//! HTML from strangers (a message replied to or forwarded, a paste) into the document, keeping
//! only what the model has: text, paragraphs, headings, quotes, lists, bold, italic, underline,
//! strike, palette colours, and links to `http`, `https` or `mailto` addresses.
//!
//! html5ever's tokenizer (already built for html2text) reads the markup and decodes the
//! character references; the tree is rebuilt here, tolerantly, with caps on size and depth.
//! Nothing is fetched: a picture is kept only when it is a `cid:` one the caller provides, and
//! scripts, styles, frames, forms and embedded objects are dropped with their content.

use std::cell::RefCell;

use html5ever::tendril::StrTendril;
use html5ever::tokenizer::states::RawKind;
use html5ever::tokenizer::{BufferQueue, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts};

use crate::model::{Color, Document, Format, Kind, Link, MAX_QUOTE, OBJECT, Paragraph, Run, is_hidden};
use crate::picture::InlineImage;

/// The most HTML read: the rest is dropped.
pub const MAX_IMPORT_HTML: usize = 2 << 20;
/// The most text kept, in bytes.
const MAX_IMPORT_TEXT: usize = 1 << 20;
/// The most paragraphs kept.
const MAX_IMPORT_PARAGRAPHS: usize = 20_000;
/// The deepest nesting of elements followed; deeper ones only break lines.
const MAX_DEPTH: usize = 64;
/// The most tokens read.
const MAX_TOKENS: usize = 400_000;
/// The longest attribute value kept.
const MAX_ATTR: usize = 4096;

/// A token, as the builder needs it.
enum Tok {
    Start { name: String, attrs: Vec<(String, String)>, self_closing: bool },
    End(String),
    Text(String),
}

/// Collects the tokenizer's tokens.
struct Sink {
    tokens: RefCell<Vec<Tok>>,
}

/// The attributes the builder reads.
const KEPT_ATTRS: [&str; 5] = ["href", "src", "style", "color", "alt"];

impl TokenSink for Sink {
    type Handle = ();

    fn process_token(&self, token: Token, _line_number: u64) -> TokenSinkResult<()> {
        let Ok(mut tokens) = self.tokens.try_borrow_mut() else { return TokenSinkResult::Continue };
        if tokens.len() >= MAX_TOKENS {
            return TokenSinkResult::Continue;
        }
        match token {
            Token::TagToken(tag) => {
                let name = tag.name.to_string().to_ascii_lowercase();
                match tag.kind {
                    TagKind::StartTag => {
                        let attrs = tag
                            .attrs
                            .iter()
                            .filter_map(|a| {
                                let key = a.name.local.to_string().to_ascii_lowercase();
                                KEPT_ATTRS.contains(&key.as_str()).then(|| (key, a.value.chars().take(MAX_ATTR).collect::<String>()))
                            })
                            .collect();
                        let raw = match name.as_str() {
                            _ if tag.self_closing => None,
                            "script" => Some(RawKind::ScriptData),
                            "style" | "xmp" | "iframe" | "noembed" | "noframes" => Some(RawKind::Rawtext),
                            "textarea" | "title" => Some(RawKind::Rcdata),
                            _ => None,
                        };
                        tokens.push(Tok::Start { name, attrs, self_closing: tag.self_closing });
                        if let Some(kind) = raw {
                            // Their content is text to the tokenizer, never markup.
                            return TokenSinkResult::RawData(kind);
                        }
                    }
                    TagKind::EndTag => tokens.push(Tok::End(name)),
                }
            }
            Token::CharacterTokens(text) => match tokens.last_mut() {
                Some(Tok::Text(last)) => last.push_str(&text),
                _ => tokens.push(Tok::Text(String::from(&*text))),
            },
            _ => {}
        }
        TokenSinkResult::Continue
    }
}

/// Elements dropped with all they hold.
fn is_skipped(name: &str) -> bool {
    matches!(
        name,
        "script" | "style" | "head" | "title" | "template" | "noscript" | "iframe" | "object" | "embed" | "applet" | "svg" | "math" | "select"
            | "textarea" | "button" | "xmp" | "noembed" | "noframes" | "frameset" | "frame" | "canvas" | "video" | "audio" | "map" | "datalist"
    )
}

/// Elements without content (no end tag).
fn is_void(name: &str) -> bool {
    matches!(name, "br" | "img" | "hr" | "meta" | "link" | "input" | "area" | "base" | "col" | "embed" | "param" | "source" | "track" | "wbr" | "keygen")
}

/// What an element does to the paragraphs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Block {
    /// A paragraph-like block (`p`, `div`, `pre`, table rows…).
    Para,
    Heading(u8),
    Quote,
    /// A list (`true`: numbered).
    List(bool),
    Item,
    Cell,
}

fn block_of(name: &str) -> Option<Block> {
    Some(match name {
        "h1" => Block::Heading(1),
        "h2" => Block::Heading(2),
        "h3" | "h4" | "h5" | "h6" => Block::Heading(3),
        "blockquote" => Block::Quote,
        "ul" | "dir" | "menu" => Block::List(false),
        "ol" => Block::List(true),
        "li" => Block::Item,
        "td" | "th" => Block::Cell,
        "p" | "div" | "pre" | "address" | "article" | "aside" | "center" | "dd" | "dl" | "dt" | "figure" | "figcaption" | "footer" | "header" | "main"
        | "nav" | "section" | "table" | "tbody" | "thead" | "tfoot" | "tr" | "caption" | "form" | "fieldset" | "legend" | "details" | "summary" | "body"
        | "html" => Block::Para,
        _ => return None,
    })
}

/// An open element.
struct Frame {
    name: String,
    block: Option<Block>,
    /// The format of text inside.
    format: Format,
    pre: bool,
}

/// Builds the document from the tokens.
struct Builder<'a> {
    stack: Vec<Frame>,
    /// Names of the skipped elements open (their content is dropped while it is not empty).
    skipping: Vec<String>,
    paragraphs: Vec<Paragraph>,
    /// The paragraph being filled, once it has started.
    current: Option<Paragraph>,
    /// A space is due before the next character (collapsed white space), in the format of the
    /// white space it stands for.
    space: Option<Format>,
    text_size: usize,
    provided: &'a [InlineImage],
    images: Vec<InlineImage>,
    full: bool,
}

/// `html` (untrusted) as a document. `provided` are the pictures its `cid:` addresses may name
/// (the related parts of the message); any other picture is dropped, and nothing is fetched.
pub fn import_html(html: &str, provided: &[InlineImage]) -> Document {
    let mut end = html.len().min(MAX_IMPORT_HTML);
    while !html.is_char_boundary(end) {
        end -= 1;
    }
    let html = html.get(..end).unwrap_or("");
    let tokenizer = Tokenizer::new(Sink { tokens: RefCell::new(Vec::new()) }, TokenizerOpts::default());
    let input = BufferQueue::default();
    input.push_back(StrTendril::from_slice(html));
    let _ = tokenizer.feed(&input);
    tokenizer.end();
    let tokens = tokenizer.sink.tokens.take();

    let mut b = Builder {
        stack: Vec::new(),
        skipping: Vec::new(),
        paragraphs: Vec::new(),
        current: None,
        space: None,
        text_size: 0,
        provided,
        images: Vec::new(),
        full: false,
    };
    for token in tokens {
        if b.full {
            break;
        }
        match token {
            Tok::Start { name, attrs, self_closing } => b.start(name, &attrs, self_closing),
            Tok::End(name) => b.end(&name),
            Tok::Text(text) => b.text(&text),
        }
    }
    b.finish(false);
    // Blank lines at the end say nothing.
    while b.paragraphs.len() > 1 && b.paragraphs.last().is_some_and(|p| p.is_empty() && !p.kind.is_image()) {
        b.paragraphs.pop();
    }
    Document::from_parts(b.paragraphs, b.images)
}

impl Builder<'_> {
    fn format(&self) -> Format {
        self.stack.last().map(|f| f.format.clone()).unwrap_or_default()
    }

    fn pre(&self) -> bool {
        self.stack.last().is_some_and(|f| f.pre)
    }

    /// A new paragraph with the block attributes of the open elements.
    fn new_paragraph(&self) -> Paragraph {
        let mut p = Paragraph::default();
        let mut lists = 0u8;
        let mut ordered = false;
        let mut item: Option<(bool, u8)> = None;
        let mut heading: Option<u8> = None;
        for f in &self.stack {
            match f.block {
                Some(Block::Quote) => p.quote = p.quote.saturating_add(1).min(MAX_QUOTE),
                Some(Block::List(o)) => {
                    lists = lists.saturating_add(1);
                    ordered = o;
                }
                Some(Block::Item) => item = Some((ordered, lists.saturating_sub(1))),
                Some(Block::Heading(h)) => heading = Some(h),
                _ => {}
            }
        }
        if let Some((ordered, level)) = item {
            p.kind = if ordered { Kind::Numbered } else { Kind::Bullet };
            p.level = level;
        } else if let Some(h) = heading {
            p.kind = Kind::Heading(h);
        }
        p
    }

    /// Ends the paragraph being filled; with `force`, an empty one is kept too (a line break).
    fn finish(&mut self, force: bool) {
        let p = match self.current.take() {
            Some(p) => p,
            None if force => self.new_paragraph(),
            None => return,
        };
        self.space = None;
        if self.paragraphs.len() >= MAX_IMPORT_PARAGRAPHS {
            self.full = true;
            return;
        }
        self.paragraphs.push(p);
    }

    fn push_char(&mut self, c: char, format: &Format) {
        if self.text_size >= MAX_IMPORT_TEXT {
            self.full = true;
            return;
        }
        if self.current.is_none() {
            self.current = Some(self.new_paragraph());
        }
        let Some(p) = self.current.as_mut() else { return };
        match p.runs.last_mut() {
            Some(last) if &last.format == format => last.text.push(c),
            _ => p.runs.push(Run::new(c.to_string(), format.clone())),
        }
        self.text_size += c.len_utf8();
    }

    fn text(&mut self, text: &str) {
        if !self.skipping.is_empty() {
            return;
        }
        let format = self.format();
        let pre = self.pre();
        for c in text.chars() {
            if self.full {
                return;
            }
            if pre {
                match c {
                    '\n' => self.finish(true),
                    '\r' => {}
                    '\t' => self.push_char('\t', &format),
                    c if c.is_control() || is_hidden(c) || c == OBJECT => {}
                    c => self.push_char(c, &format),
                }
                continue;
            }
            if matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c') {
                if self.space.is_none() && self.current.as_ref().is_some_and(|p| !p.is_empty()) {
                    self.space = Some(format.clone());
                }
                continue;
            }
            if c.is_control() || is_hidden(c) || c == OBJECT {
                continue;
            }
            if let Some(space_format) = self.space.take() {
                self.push_char(' ', &space_format);
            }
            self.push_char(c, &format);
        }
    }

    fn start(&mut self, name: String, attrs: &[(String, String)], self_closing: bool) {
        let attr = |key: &str| attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
        if !self.skipping.is_empty() {
            if !is_void(&name) && !self_closing && self.skipping.len() < MAX_DEPTH {
                self.skipping.push(name);
            }
            return;
        }
        if is_skipped(&name) {
            if !is_void(&name) && !self_closing {
                self.skipping.push(name);
            }
            return;
        }
        match name.as_str() {
            "br" => return self.finish(true),
            "hr" => {
                self.finish(false);
                return self.finish(true);
            }
            "img" => return self.image(attr("src").unwrap_or(""), attr("alt").unwrap_or("")),
            "td" | "th" => {
                // A new cell closes the one open in the same row.
                self.close_open("td", &["tr", "table"]);
                self.close_open("th", &["tr", "table"]);
            }
            "tr" => self.close_open("tr", &["table"]),
            "li" => self.close_open("li", &["ul", "ol", "dir", "menu"]),
            "p" => self.close_open("p", &["div", "li", "td", "th", "blockquote", "ul", "ol", "table"]),
            _ => {}
        }
        let block = block_of(&name);
        if let Some(b) = block {
            if b == Block::Cell {
                // Cells of a row stay on one line, apart.
                if self.space.is_none() && self.current.as_ref().is_some_and(|p| !p.is_empty()) {
                    self.space = Some(Format::default());
                }
            } else {
                self.finish(false);
            }
        }
        if is_void(&name) || self_closing || self.stack.len() >= MAX_DEPTH {
            return;
        }
        let mut format = self.format();
        match name.as_str() {
            "b" | "strong" => format.bold = true,
            "i" | "em" | "cite" | "var" | "dfn" => format.italic = true,
            "u" | "ins" => format.underline = true,
            "s" | "strike" | "del" => format.strike = true,
            "a" => format.link = attr("href").and_then(Link::parse),
            "font" => {
                if let Some(c) = attr("color").and_then(parse_color) {
                    format.color = Color::nearest(c);
                }
            }
            _ => {}
        }
        if let Some(style) = attr("style") {
            apply_style(style, &mut format);
        }
        let pre = name == "pre" || self.pre();
        self.stack.push(Frame { name, block, format, pre });
    }

    /// Closes an open `name` if it is more recent than any of `until` (an implied end tag).
    fn close_open(&mut self, name: &str, until: &[&str]) {
        let mut found = None;
        for (i, f) in self.stack.iter().enumerate().rev() {
            if f.name == name {
                found = Some(i);
                break;
            }
            if until.contains(&f.name.as_str()) {
                break;
            }
        }
        if let Some(i) = found {
            self.pop_to(i);
        }
    }

    /// Closes the elements from index `i` up.
    fn pop_to(&mut self, i: usize) {
        let blocks = self.stack.iter().skip(i).any(|f| f.block.is_some_and(|b| b != Block::Cell));
        if blocks {
            self.finish(false);
        }
        self.stack.truncate(i);
    }

    fn end(&mut self, name: &str) {
        if !self.skipping.is_empty() {
            if let Some(i) = self.skipping.iter().rposition(|n| n == name) {
                self.skipping.truncate(i);
            }
            return;
        }
        match name {
            "br" => return self.finish(true),
            "p" if !self.stack.iter().any(|f| f.name == "p") => return self.finish(false),
            _ => {}
        }
        if let Some(i) = self.stack.iter().rposition(|f| f.name == name) {
            self.pop_to(i);
        }
    }

    /// A picture: kept only for a `cid:` the caller provides; else its alternative text.
    fn image(&mut self, src: &str, alt: &str) {
        let provided = self.provided;
        let src = src.trim();
        let cid = src.get(..4).filter(|s| s.eq_ignore_ascii_case("cid:")).and_then(|_| src.get(4..)).map(percent_decode);
        if let Some(cid) = cid
            && let Some(image) = provided.iter().find(|i| i.content_id == cid)
        {
            self.finish(false);
            let mut p = Paragraph::image(&image.content_id);
            p.quote = self.new_paragraph().quote;
            if self.paragraphs.len() >= MAX_IMPORT_PARAGRAPHS {
                self.full = true;
                return;
            }
            self.paragraphs.push(p);
            if !self.images.iter().any(|i| i.content_id == image.content_id) {
                self.images.push(image.clone());
            }
            return;
        }
        let alt = alt.trim();
        if !alt.is_empty() && alt.chars().count() <= 100 {
            self.text(&format!("[{alt}]"));
        }
    }
}

/// `%XX` sequences decoded (a `cid:` address may be percent-encoded, RFC 2392).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b == b'%'
            && let (Some(h), Some(l)) = (bytes.get(i + 1).and_then(|c| (*c as char).to_digit(16)), bytes.get(i + 2).and_then(|c| (*c as char).to_digit(16)))
        {
            out.push(u8::try_from(h * 16 + l).unwrap_or(b'?'));
            i += 3;
            continue;
        }
        out.push(b);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What an inline `style` attribute says about the model's formats.
fn apply_style(style: &str, format: &mut Format) {
    for decl in style.split(';').take(64) {
        let Some((key, value)) = decl.split_once(':') else { continue };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim().trim_end_matches("!important").trim().to_ascii_lowercase();
        match key.as_str() {
            "font-weight" => {
                format.bold = matches!(value.as_str(), "bold" | "bolder") || value.parse::<u32>().is_ok_and(|w| w >= 600);
            }
            "font-style" => format.italic = matches!(value.as_str(), "italic" | "oblique"),
            "text-decoration" | "text-decoration-line" => {
                if value.contains("underline") {
                    format.underline = true;
                }
                if value.contains("line-through") {
                    format.strike = true;
                }
                if value == "none" {
                    format.underline = false;
                    format.strike = false;
                }
            }
            "color" => {
                if let Some(c) = parse_color(&value) {
                    format.color = Color::nearest(c);
                }
            }
            _ => {}
        }
    }
}

/// A CSS or HTML colour: `#rgb`, `#rrggbb`, `rgb(r, g, b)` or a few names.
fn parse_color(value: &str) -> Option<[u8; 3]> {
    let v = value.trim().to_ascii_lowercase();
    if let Some(hex) = v.strip_prefix('#') {
        let digits: Vec<u8> = hex.chars().map(|c| c.to_digit(16).and_then(|d| u8::try_from(d).ok())).collect::<Option<Vec<u8>>>()?;
        return match digits.as_slice() {
            [r, g, b] => Some([r * 17, g * 17, b * 17]),
            [r1, r2, g1, g2, b1, b2] => Some([r1 * 16 + r2, g1 * 16 + g2, b1 * 16 + b2]),
            _ => None,
        };
    }
    if let Some(inner) = v.strip_prefix("rgb(").or_else(|| v.strip_prefix("rgba(")).and_then(|r| r.strip_suffix(')')) {
        let parts: Vec<u8> = inner.split(',').take(3).map(|p| p.trim().parse::<f32>().ok().map(|n| n.clamp(0.0, 255.0) as u8)).collect::<Option<Vec<u8>>>()?;
        return match parts.as_slice() {
            [r, g, b] => Some([*r, *g, *b]),
            _ => None,
        };
    }
    Some(match v.as_str() {
        "red" => [255, 0, 0],
        "orange" => [255, 165, 0],
        "green" => [0, 128, 0],
        "blue" => [0, 0, 255],
        "navy" => [0, 0, 128],
        "purple" => [128, 0, 128],
        "gray" | "grey" => [128, 128, 128],
        "black" => [0, 0, 0],
        "white" => [255, 255, 255],
        _ => return None,
    })
}
