//! The document: paragraphs of runs of formatted text, block pictures, positions and selections.
//!
//! A position is a paragraph and a character (`char`) offset in it. A picture is a paragraph of
//! its own holding one character ([`OBJECT`]), so that the caret can stand before or after it and
//! a selection can hold it like any other character.

use std::fmt;
use std::sync::Arc;

use crate::picture::InlineImage;

/// The character a picture paragraph holds.
pub const OBJECT: char = '\u{FFFC}';
/// The deepest list nesting (0 is the outermost list).
pub const MAX_LEVEL: u8 = 7;
/// The deepest quote nesting.
pub const MAX_QUOTE: u8 = 8;

/// The colours text can take: a small palette, so that the HTML written holds only known values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Color {
    Red,
    Orange,
    Green,
    Blue,
    Purple,
    Gray,
}

impl Color {
    pub const ALL: [Color; 6] = [Color::Red, Color::Orange, Color::Green, Color::Blue, Color::Purple, Color::Gray];

    /// Its red, green and blue values.
    pub fn rgb(self) -> [u8; 3] {
        match self {
            Color::Red => [0xcc, 0x00, 0x00],
            Color::Orange => [0xe6, 0x7e, 0x00],
            Color::Green => [0x2e, 0x7d, 0x32],
            Color::Blue => [0x15, 0x65, 0xc0],
            Color::Purple => [0x6a, 0x1b, 0x9a],
            Color::Gray => [0x75, 0x75, 0x75],
        }
    }

    /// `#rrggbb`, as the HTML writes it.
    pub fn hex(self) -> String {
        let [r, g, b] = self.rgb();
        format!("#{r:02x}{g:02x}{b:02x}")
    }

    /// Its name, for the toolbar.
    pub fn name(self) -> &'static str {
        match self {
            Color::Red => "Red",
            Color::Orange => "Orange",
            Color::Green => "Green",
            Color::Blue => "Blue",
            Color::Purple => "Purple",
            Color::Gray => "Gray",
        }
    }

    /// The palette colour closest to `rgb`, or `None` for black, white and the greys close to the
    /// text's own colour (most colours in received mail are those, and are not kept).
    pub(crate) fn nearest(rgb: [u8; 3]) -> Option<Color> {
        let [r, g, b] = rgb.map(i32::from);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        if max - min < 24 {
            // A grey: only a mid grey is kept, as Gray.
            return (64..=176).contains(&max).then_some(Color::Gray);
        }
        Color::ALL.into_iter().filter(|c| *c != Color::Gray).min_by_key(|c| {
            let [cr, cg, cb] = c.rgb().map(i32::from);
            (r - cr).pow(2) + (g - cg).pow(2) + (b - cb).pow(2)
        })
    }
}

/// A link's target: `https:`, `http:` or `mailto:` only.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Link(Arc<str>);

impl Link {
    /// The longest target kept.
    pub const MAX_LEN: usize = 2048;

    /// A link to `url`, if it is an `https:`, `http:` or `mailto:` address without spaces or
    /// control characters; the scheme is written in lower case. A bare `www.` address becomes
    /// `https://www.…`.
    pub fn parse(url: &str) -> Option<Link> {
        let url = url.trim();
        if url.is_empty() || url.len() > Self::MAX_LEN || url.chars().any(|c| c.is_whitespace() || c.is_control() || is_hidden(c)) {
            return None;
        }
        let lower = url.to_ascii_lowercase();
        if lower.starts_with("www.") && url.len() > 4 {
            return Some(Link(format!("https://{url}").into()));
        }
        let (scheme, rest) = url.split_once(':')?;
        let scheme = scheme.to_ascii_lowercase();
        let ok = match scheme.as_str() {
            "http" | "https" => rest.strip_prefix("//").is_some_and(|host| host.split(['/', '?', '#']).next().is_some_and(|h| !h.is_empty())),
            "mailto" => !rest.is_empty(),
            _ => false,
        };
        ok.then(|| Link(format!("{scheme}:{rest}").into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Link {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Link({:?})", &*self.0)
    }
}

/// Direction overrides and isolates, which can make text read otherwise than it is: never kept
/// in the document.
pub(crate) fn is_hidden(c: char) -> bool {
    matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200e}' | '\u{200f}' | '\u{061c}')
}

/// Text as the document keeps it: control characters (other than tab and, when `lines`, line
/// feeds) and direction overrides removed, line ends unified, the object character removed.
pub(crate) fn clean(text: &str, lines: bool) -> String {
    let unified = text.replace("\r\n", "\n");
    unified
        .chars()
        .filter_map(|c| match c {
            '\r' | '\n' | '\u{2028}' | '\u{2029}' => lines.then_some('\n'),
            '\t' => Some('\t'),
            OBJECT => None,
            c if c.is_control() || is_hidden(c) => None,
            c => Some(c),
        })
        .collect()
}

/// The character marks a run can have.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mark {
    Bold,
    Italic,
    Underline,
    Strike,
}

/// How a run of text looks.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Format {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub color: Option<Color>,
    pub link: Option<Link>,
}

impl Format {
    pub fn has(&self, mark: Mark) -> bool {
        match mark {
            Mark::Bold => self.bold,
            Mark::Italic => self.italic,
            Mark::Underline => self.underline,
            Mark::Strike => self.strike,
        }
    }

    pub fn set(&mut self, mark: Mark, on: bool) {
        match mark {
            Mark::Bold => self.bold = on,
            Mark::Italic => self.italic = on,
            Mark::Underline => self.underline = on,
            Mark::Strike => self.strike = on,
        }
    }

    /// The same look without the link.
    pub fn without_link(&self) -> Format {
        Format { link: None, ..self.clone() }
    }
}

/// Text in one format.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    pub format: Format,
}

impl Run {
    pub fn new(text: impl Into<String>, format: Format) -> Run {
        Run { text: text.into(), format }
    }

    pub fn plain(text: impl Into<String>) -> Run {
        Run { text: text.into(), format: Format::default() }
    }
}

/// What a paragraph is.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Kind {
    #[default]
    Normal,
    /// A heading, level 1 to 3.
    Heading(u8),
    /// An item of a bulleted list.
    Bullet,
    /// An item of a numbered list.
    Numbered,
    /// A picture, by its content id in [`Document::images`].
    Image(String),
}

impl Kind {
    pub fn is_list(&self) -> bool {
        matches!(self, Kind::Bullet | Kind::Numbered)
    }

    pub fn is_image(&self) -> bool {
        matches!(self, Kind::Image(_))
    }
}

/// A paragraph: one line of the message until the next Enter, wrapped on screen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Paragraph {
    pub kind: Kind,
    /// The list nesting of an item, from 0; 0 for any other paragraph.
    pub level: u8,
    /// How many quotes it is in (a reply's quoted text is 1).
    pub quote: u8,
    /// It belongs to the signature.
    pub signature: bool,
    pub runs: Vec<Run>,
}

impl Paragraph {
    pub fn new(runs: Vec<Run>) -> Paragraph {
        let mut p = Paragraph { runs, ..Paragraph::default() };
        p.normalize();
        p
    }

    /// An empty paragraph with the block attributes (quote, signature) of `self`.
    pub(crate) fn sibling(&self) -> Paragraph {
        Paragraph { quote: self.quote, signature: self.signature, ..Paragraph::default() }
    }

    /// A picture paragraph.
    pub fn image(content_id: &str) -> Paragraph {
        let mut p = Paragraph { kind: Kind::Image(content_id.to_owned()), ..Paragraph::default() };
        p.normalize();
        p
    }

    /// Its text (the object character for a picture).
    pub fn text(&self) -> String {
        self.runs.iter().map(|r| r.text.as_str()).collect()
    }

    /// Its length in characters (1 for a picture).
    pub fn len(&self) -> usize {
        self.runs.iter().map(|r| r.text.chars().count()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.runs.iter().all(|r| r.text.is_empty())
    }

    /// Its characters.
    pub(crate) fn chars(&self) -> Vec<char> {
        self.runs.iter().flat_map(|r| r.text.chars()).collect()
    }

    /// Merges adjacent runs of the same format and removes empty ones; a picture holds exactly
    /// the object character, and only list items have a level.
    pub(crate) fn normalize(&mut self) {
        if self.kind.is_image() {
            self.runs = vec![Run::plain(OBJECT.to_string())];
            self.level = 0;
            return;
        }
        let mut out: Vec<Run> = Vec::with_capacity(self.runs.len());
        for run in self.runs.drain(..) {
            if run.text.is_empty() {
                continue;
            }
            match out.last_mut() {
                Some(last) if last.format == run.format => last.text.push_str(&run.text),
                _ => out.push(run),
            }
        }
        self.runs = out;
        if let Kind::Heading(h) = self.kind {
            self.kind = Kind::Heading(h.clamp(1, 3));
        }
        self.level = if self.kind.is_list() { self.level.min(MAX_LEVEL) } else { 0 };
        self.quote = self.quote.min(MAX_QUOTE);
    }

    /// Splits the run holding character `at` so that a run starts there; returns that run's
    /// index (the number of runs when `at` is the end).
    pub(crate) fn split_runs_at(&mut self, at: usize) -> usize {
        let mut seen = 0usize;
        let mut index = 0usize;
        while let Some(run) = self.runs.get_mut(index) {
            let n = run.text.chars().count();
            if at == seen {
                return index;
            }
            if at < seen + n {
                let byte = byte_of_char(&run.text, at - seen);
                let tail = run.text.split_off(byte);
                let format = run.format.clone();
                self.runs.insert(index + 1, Run { text: tail, format });
                return index + 1;
            }
            seen += n;
            index += 1;
        }
        self.runs.len()
    }

    /// Removes and returns the runs from character `at` on.
    pub(crate) fn split_off(&mut self, at: usize) -> Vec<Run> {
        let i = self.split_runs_at(at);
        if i >= self.runs.len() {
            return Vec::new();
        }
        self.runs.split_off(i)
    }

    /// Inserts `runs` at character `at`.
    pub(crate) fn insert_runs(&mut self, at: usize, runs: Vec<Run>) {
        let i = self.split_runs_at(at);
        let tail = if i < self.runs.len() { self.runs.split_off(i) } else { Vec::new() };
        self.runs.extend(runs);
        self.runs.extend(tail);
        self.normalize();
    }

    /// Removes the characters `start..end`.
    pub(crate) fn remove(&mut self, start: usize, end: usize) {
        if start >= end {
            return;
        }
        let tail = self.split_off(end);
        let _ = self.split_off(start);
        self.runs.extend(tail);
        self.normalize();
    }

    /// The runs of the characters `start..end`.
    pub(crate) fn slice(&self, start: usize, end: usize) -> Vec<Run> {
        let mut copy = self.clone();
        let _ = copy.split_off(end);
        copy.split_off(start)
    }

    /// Changes the format of the characters `start..end`.
    pub(crate) fn map_formats(&mut self, start: usize, end: usize, mut f: impl FnMut(&mut Format)) {
        if start >= end || self.kind.is_image() {
            return;
        }
        let first = self.split_runs_at(start);
        let last = self.split_runs_at(end);
        for run in self.runs.iter_mut().take(last).skip(first) {
            f(&mut run.format);
        }
        self.normalize();
    }

    /// The formats of the characters `start..end`, one per run they touch.
    pub(crate) fn formats_in(&self, start: usize, end: usize) -> Vec<&Format> {
        let mut out = Vec::new();
        let mut seen = 0usize;
        for run in &self.runs {
            let n = run.text.chars().count();
            if seen < end && seen + n > start {
                out.push(&run.format);
            }
            seen += n;
        }
        out
    }

    /// The format of character `at` (the last one's at the end), if there is one.
    pub(crate) fn format_of_char(&self, at: usize) -> Option<&Format> {
        let mut seen = 0usize;
        for run in &self.runs {
            let n = run.text.chars().count();
            if at < seen + n {
                return Some(&run.format);
            }
            seen += n;
        }
        None
    }

    /// The format text typed at `at` takes: the character before's (the one after at the
    /// start), without a link unless the caret is inside one.
    pub(crate) fn format_at(&self, at: usize) -> Format {
        if self.kind.is_image() {
            return Format::default();
        }
        let before = at.checked_sub(1).and_then(|b| self.format_of_char(b));
        let after = self.format_of_char(at);
        let mut f = before.or(after).cloned().unwrap_or_default();
        let inside = matches!((before, after), (Some(b), Some(a)) if b.link.is_some() && b.link == a.link);
        if !inside {
            f.link = None;
        }
        f
    }
}

/// The byte offset of character `n` of `s` (its length past the end).
pub(crate) fn byte_of_char(s: &str, n: usize) -> usize {
    s.char_indices().nth(n).map_or(s.len(), |(b, _)| b)
}

/// A whole message being written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Document {
    /// Never empty.
    pub paragraphs: Vec<Paragraph>,
    /// The pictures the paragraphs show, by content id.
    pub images: Vec<InlineImage>,
}

impl Default for Document {
    fn default() -> Self {
        Document::new()
    }
}

impl Document {
    /// One empty paragraph.
    pub fn new() -> Document {
        Document { paragraphs: vec![Paragraph::default()], images: Vec::new() }
    }

    /// A document of plain text, one paragraph per line.
    pub fn from_text(text: &str) -> Document {
        let text = clean(text, true);
        let mut doc = Document {
            paragraphs: text.split('\n').map(|line| Paragraph::new(vec![Run::plain(line)])).collect(),
            images: Vec::new(),
        };
        doc.normalize();
        doc
    }

    /// A document of the paragraphs, with the pictures they name.
    pub fn from_parts(paragraphs: Vec<Paragraph>, images: Vec<InlineImage>) -> Document {
        let mut doc = Document { paragraphs, images };
        doc.normalize();
        doc
    }

    /// The same paragraphs one quote deeper and out of any signature (the text of a message
    /// replied to).
    pub fn quoted(mut self) -> Document {
        for p in &mut self.paragraphs {
            p.quote = p.quote.saturating_add(1).min(MAX_QUOTE);
            p.signature = false;
        }
        self
    }

    /// Whether it holds no text and no picture.
    pub fn is_empty(&self) -> bool {
        self.paragraphs.iter().all(|p| p.is_empty() && !p.kind.is_image())
    }

    pub fn image(&self, content_id: &str) -> Option<&InlineImage> {
        self.images.iter().find(|i| i.content_id == content_id)
    }

    pub fn paragraph(&self, index: usize) -> Option<&Paragraph> {
        self.paragraphs.get(index)
    }

    /// The length of paragraph `index` in characters (0 past the end).
    pub fn paragraph_len(&self, index: usize) -> usize {
        self.paragraphs.get(index).map_or(0, Paragraph::len)
    }

    /// Restores the invariants: at least one paragraph, each normalized, a picture paragraph
    /// only for a picture the document has, and only the pictures a paragraph shows.
    pub fn normalize(&mut self) {
        if self.paragraphs.is_empty() {
            self.paragraphs.push(Paragraph::default());
        }
        let images = &self.images;
        for p in &mut self.paragraphs {
            if let Kind::Image(id) = &p.kind
                && !images.iter().any(|i| &i.content_id == id)
            {
                p.kind = Kind::Normal;
                p.runs.clear();
            }
            p.normalize();
        }
        let used: Vec<&str> = self.paragraphs.iter().filter_map(|p| if let Kind::Image(id) = &p.kind { Some(id.as_str()) } else { None }).collect();
        let mut kept: Vec<InlineImage> = Vec::with_capacity(self.images.len());
        for image in self.images.drain(..) {
            if used.contains(&image.content_id.as_str()) && !kept.iter().any(|k| k.content_id == image.content_id) {
                kept.push(image);
            }
        }
        self.images = kept;
    }

    /// The last position.
    pub fn end(&self) -> Pos {
        let para = self.paragraphs.len().saturating_sub(1);
        Pos { para, offset: self.paragraph_len(para) }
    }

    /// `pos` moved inside the document.
    pub fn clamp(&self, pos: Pos) -> Pos {
        if pos.para >= self.paragraphs.len() {
            return self.end();
        }
        Pos { para: pos.para, offset: pos.offset.min(self.paragraph_len(pos.para)) }
    }

    /// The number each numbered item shows (`None` for every other paragraph): counted per list
    /// level, restarted after anything that is not a list item, and when the kind of item changes
    /// at a level.
    pub fn list_numbers(&self) -> Vec<Option<usize>> {
        let levels = usize::from(MAX_LEVEL) + 1;
        let mut counters = vec![0usize; levels];
        let mut kinds: Vec<Option<Kind>> = vec![None; levels];
        let mut quote = 0u8;
        let mut out = Vec::with_capacity(self.paragraphs.len());
        for p in &self.paragraphs {
            if !p.kind.is_list() || p.quote != quote {
                counters.iter_mut().for_each(|c| *c = 0);
                kinds.iter_mut().for_each(|k| *k = None);
                quote = p.quote;
            }
            if !p.kind.is_list() {
                out.push(None);
                continue;
            }
            let level = usize::from(p.level);
            for deeper in level + 1..levels {
                if let Some(c) = counters.get_mut(deeper) {
                    *c = 0;
                }
                if let Some(k) = kinds.get_mut(deeper) {
                    *k = None;
                }
            }
            let same = kinds.get(level).is_some_and(|k| k.as_ref() == Some(&p.kind));
            let counter = counters.get_mut(level);
            let n = match counter {
                Some(c) => {
                    if !same {
                        *c = 0;
                    }
                    *c += 1;
                    *c
                }
                None => 1,
            };
            if let Some(k) = kinds.get_mut(level) {
                *k = Some(p.kind.clone());
            }
            out.push((p.kind == Kind::Numbered).then_some(n));
        }
        out
    }
}

/// A place between two characters: paragraph `para`, before its character `offset`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pos {
    pub para: usize,
    pub offset: usize,
}

impl Pos {
    pub fn new(para: usize, offset: usize) -> Pos {
        Pos { para, offset }
    }
}

/// The selection: from `anchor` (where it started) to `head` (where the caret is). Empty when
/// they are the same: then it is only the caret.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub anchor: Pos,
    pub head: Pos,
}

impl Selection {
    pub fn caret(pos: Pos) -> Selection {
        Selection { anchor: pos, head: pos }
    }

    pub fn new(anchor: Pos, head: Pos) -> Selection {
        Selection { anchor, head }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// Its start and end, in document order.
    pub fn range(&self) -> (Pos, Pos) {
        if self.anchor <= self.head { (self.anchor, self.head) } else { (self.head, self.anchor) }
    }
}

/// What kind of character this is, for word boundaries.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CharClass {
    Space,
    Word,
    Other,
}

pub(crate) fn char_class(c: char) -> CharClass {
    if c.is_whitespace() {
        CharClass::Space
    } else if c.is_alphanumeric() || c == '_' || c == '\'' || c == '’' || is_extending(c) {
        CharClass::Word
    } else {
        CharClass::Other
    }
}

/// A character that belongs with the one before it (accents, variation selectors, skin tones,
/// joiners): the caret does not stop before it.
pub(crate) fn is_extending(c: char) -> bool {
    matches!(c,
        '\u{0300}'..='\u{036f}'
        | '\u{1ab0}'..='\u{1aff}'
        | '\u{1dc0}'..='\u{1dff}'
        | '\u{20d0}'..='\u{20ff}'
        | '\u{fe00}'..='\u{fe0f}'
        | '\u{fe20}'..='\u{fe2f}'
        | '\u{200c}' | '\u{200d}'
        | '\u{1f3fb}'..='\u{1f3ff}'
        | '\u{e0020}'..='\u{e007f}'
        | '\u{e0100}'..='\u{e01ef}')
}

/// The offset one character (with what extends it) before `at` in `chars`.
pub(crate) fn prev_boundary(chars: &[char], at: usize) -> usize {
    let mut i = at.min(chars.len());
    if i == 0 {
        return 0;
    }
    i -= 1;
    // Back over the marks that extend a base, and over a joined sequence (a ZWJ emoji).
    while i > 0 {
        let here = chars.get(i).copied().unwrap_or(' ');
        let before = chars.get(i - 1).copied().unwrap_or(' ');
        if is_extending(here) || before == '\u{200d}' {
            i -= 1;
        } else {
            break;
        }
    }
    i
}

/// The offset one character (with what extends it) after `at` in `chars`.
pub(crate) fn next_boundary(chars: &[char], at: usize) -> usize {
    let len = chars.len();
    let mut i = at.min(len);
    if i >= len {
        return len;
    }
    i += 1;
    while i < len {
        let here = chars.get(i).copied().unwrap_or(' ');
        let before = chars.get(i - 1).copied().unwrap_or(' ');
        if is_extending(here) || before == '\u{200d}' {
            i += 1;
        } else {
            break;
        }
    }
    i
}
