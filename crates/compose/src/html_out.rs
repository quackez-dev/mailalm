//! The document as it is sent: a small, safe HTML (only the elements the model has, every text
//! escaped, attributes written from known values only), and its plain-text alternative.

use crate::model::{Document, Kind, Link, Paragraph, Run};

/// The widest a picture is written, in pixels (mail readers scale it down further if needed).
const MAX_IMAGE_WIDTH: u32 = 640;

/// The message as HTML: a whole document with `p`, `h1`-`h3`, `blockquote`, `ul`/`ol`/`li`,
/// `b`/`i`/`u`/`s`, `span` with a palette colour, `a href` (http, https, mailto) and
/// `img src="cid:..."`.
pub fn to_html(doc: &Document) -> String {
    let mut out = String::from(
        "<!DOCTYPE html>\n<html><head><meta http-equiv=\"Content-Type\" content=\"text/html; charset=utf-8\"></head>\n<body style=\"font-family:'Segoe UI',Arial,sans-serif;font-size:11pt\">\n",
    );
    let mut stack: Vec<Open> = Vec::new();
    for p in &doc.paragraphs {
        let want = wants(p);
        let mut keep = 0;
        while let (Some(open), Some(w)) = (stack.get(keep), want.get(keep)) {
            if !open.matches(*w) {
                break;
            }
            keep += 1;
        }
        while stack.len() > keep {
            if let Some(open) = stack.pop() {
                open.close(&mut out);
            }
        }
        let continues = p.kind.is_list() && stack.len() == want.len();
        if continues {
            // The next item of the same list.
            out.push_str("</li>\n");
        }
        for w in want.iter().skip(stack.len()) {
            if let Some(Open::List { li_open: false, .. }) = stack.last() {
                // A list nested deeper than the item before: inside an item of its own.
                out.push_str("<li style=\"list-style:none\">");
                if let Some(Open::List { li_open, .. }) = stack.last_mut() {
                    *li_open = true;
                }
            }
            stack.push(Open::open(*w, &mut out));
        }
        if p.kind.is_list() {
            out.push_str("<li>");
            if let Some(Open::List { li_open, .. }) = stack.last_mut() {
                *li_open = true;
            }
            inline(&mut out, &p.runs);
            continue;
        }
        block(&mut out, p, doc);
    }
    while let Some(open) = stack.pop() {
        open.close(&mut out);
    }
    out.push_str("</body></html>\n");
    out
}

/// What a paragraph needs around it, from the outside in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Want {
    Signature,
    Quote,
    /// A list (`true`: numbered); the innermost must be of the item's kind.
    List { ordered: bool, exact: bool },
}

/// An element left open.
enum Open {
    Signature,
    Quote,
    List { ordered: bool, li_open: bool },
}

impl Open {
    fn matches(&self, want: Want) -> bool {
        match (self, want) {
            (Open::Signature, Want::Signature) | (Open::Quote, Want::Quote) => true,
            (Open::List { ordered, .. }, Want::List { ordered: o, exact }) => !exact || *ordered == o,
            _ => false,
        }
    }

    fn open(want: Want, out: &mut String) -> Open {
        match want {
            Want::Signature => {
                out.push_str("<div class=\"mailalm-signature\">\n");
                Open::Signature
            }
            Want::Quote => {
                out.push_str("<blockquote style=\"margin:0 0 0 0.8ex;border-left:2px solid #cccccc;padding-left:1ex\">\n");
                Open::Quote
            }
            Want::List { ordered, .. } => {
                out.push_str(if ordered { "<ol>\n" } else { "<ul>\n" });
                Open::List { ordered, li_open: false }
            }
        }
    }

    fn close(self, out: &mut String) {
        match self {
            Open::Signature => out.push_str("</div>\n"),
            Open::Quote => out.push_str("</blockquote>\n"),
            Open::List { ordered, li_open } => {
                if li_open {
                    out.push_str("</li>\n");
                }
                out.push_str(if ordered { "</ol>\n" } else { "</ul>\n" });
            }
        }
    }
}

fn wants(p: &Paragraph) -> Vec<Want> {
    let mut want = Vec::new();
    if p.signature {
        want.push(Want::Signature);
    }
    for _ in 0..p.quote {
        want.push(Want::Quote);
    }
    if p.kind.is_list() {
        let ordered = p.kind == Kind::Numbered;
        for _ in 0..p.level {
            want.push(Want::List { ordered, exact: false });
        }
        want.push(Want::List { ordered, exact: true });
    }
    want
}

/// A paragraph that is not a list item.
fn block(out: &mut String, p: &Paragraph, doc: &Document) {
    match &p.kind {
        Kind::Image(id) => {
            let Some(image) = doc.image(id) else { return };
            let width = image.width.min(MAX_IMAGE_WIDTH).max(1);
            let height = (u64::from(image.height) * u64::from(width) / u64::from(image.width.max(1))).max(1);
            out.push_str(&format!(
                "<p><img src=\"cid:{}\" alt=\"\" width=\"{width}\" height=\"{height}\" style=\"max-width:100%;height:auto\"></p>\n",
                escape_attr(&image.content_id)
            ));
        }
        Kind::Heading(level) => {
            let level = (*level).clamp(1, 3);
            out.push_str(&format!("<h{level}>"));
            inline(out, &p.runs);
            out.push_str(&format!("</h{level}>\n"));
        }
        Kind::Normal | Kind::Bullet | Kind::Numbered => {
            out.push_str("<p style=\"margin:0\">");
            inline(out, &p.runs);
            out.push_str("</p>\n");
        }
    }
}

/// The runs of a paragraph, links grouped; `<br>` for an empty one (so that it keeps its line).
fn inline(out: &mut String, runs: &[Run]) {
    if runs.iter().all(|r| r.text.is_empty()) {
        out.push_str("<br>");
        return;
    }
    let mut space_before = true;
    let mut current: Option<&Link> = None;
    let mut open = false;
    for run in runs {
        let link = run.format.link.as_ref();
        if open && link != current {
            out.push_str("</a>");
            open = false;
        }
        if !open && let Some(l) = link {
            out.push_str(&format!("<a href=\"{}\">", escape_attr(l.as_str())));
            open = true;
        }
        current = link;
        let f = &run.format;
        let tags: Vec<(String, &str)> = [
            (f.bold, "<b>".to_owned(), "</b>"),
            (f.italic, "<i>".to_owned(), "</i>"),
            (f.underline, "<u>".to_owned(), "</u>"),
            (f.strike, "<s>".to_owned(), "</s>"),
        ]
        .into_iter()
        .filter(|(on, _, _)| *on)
        .map(|(_, o, c)| (o, c))
        .chain(f.color.map(|c| (format!("<span style=\"color:{}\">", c.hex()), "</span>")))
        .collect();
        for (o, _) in &tags {
            out.push_str(o);
        }
        escape_text(out, &run.text, &mut space_before);
        for (_, c) in tags.iter().rev() {
            out.push_str(c);
        }
    }
    if open {
        out.push_str("</a>");
    }
}

/// `text` escaped for HTML; spaces that HTML would collapse (at the start, after another one)
/// and tabs written as no-break spaces.
fn escape_text(out: &mut String, text: &str, space_before: &mut bool) {
    for c in text.chars() {
        let space = matches!(c, ' ' | '\t' | '\u{a0}');
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            '\t' => out.push_str("&nbsp;&nbsp;&nbsp;&nbsp;"),
            '\u{a0}' => out.push_str("&nbsp;"),
            ' ' if *space_before => out.push_str("&nbsp;"),
            c => out.push(c),
        }
        *space_before = space;
    }
}

/// `value` escaped for a double-quoted attribute.
pub(crate) fn escape_attr(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// The plain-text alternative: one line per paragraph; quotes with `> `, list items with `- ` or
/// their number (indented by level), links as `text <url>`, pictures as `[image]`, and the
/// signature after a `-- ` line.
pub fn plain_text(doc: &Document) -> String {
    let numbers = doc.list_numbers();
    let mut lines: Vec<String> = Vec::with_capacity(doc.paragraphs.len());
    let mut in_signature = false;
    for (i, p) in doc.paragraphs.iter().enumerate() {
        if p.signature && !in_signature {
            lines.push("-- ".to_owned());
        }
        in_signature = p.signature;
        let mut line = String::new();
        if p.quote > 0 {
            line.push_str(&">".repeat(usize::from(p.quote)));
            line.push(' ');
        }
        match &p.kind {
            Kind::Bullet => {
                line.push_str(&"  ".repeat(usize::from(p.level)));
                line.push_str("- ");
            }
            Kind::Numbered => {
                line.push_str(&"  ".repeat(usize::from(p.level)));
                let n = numbers.get(i).copied().flatten().unwrap_or(1);
                line.push_str(&format!("{n}. "));
            }
            Kind::Image(_) => line.push_str("[image]"),
            Kind::Normal | Kind::Heading(_) => {}
        }
        if !p.kind.is_image() {
            line.push_str(&runs_text(&p.runs));
        }
        let trimmed = line.trim_end();
        lines.push(trimmed.to_owned());
    }
    lines.join("\n")
}

/// The text of runs, each link followed by its address unless the text already is it.
fn runs_text(runs: &[Run]) -> String {
    let mut out = String::new();
    let mut current: Option<&Link> = None;
    let mut link_text = String::new();
    for run in runs {
        let link = run.format.link.as_ref();
        if link != current {
            flush_link(&mut out, current, &mut link_text);
            current = link;
        }
        out.push_str(&run.text);
        if link.is_some() {
            link_text.push_str(&run.text);
        }
    }
    flush_link(&mut out, current, &mut link_text);
    out
}

/// After a link's text: its address, unless the text already is it.
fn flush_link(out: &mut String, link: Option<&Link>, link_text: &mut String) {
    if let Some(l) = link {
        let url = l.as_str();
        let shown = link_text.trim();
        let same = shown == url || url.strip_prefix("mailto:").is_some_and(|a| a == shown) || url.strip_suffix('/').is_some_and(|u| u == shown);
        if !same {
            out.push_str(&format!(" <{url}>"));
        }
    }
    link_text.clear();
}

/// The HTML version for the MIME builder, with the pictures it shows (each once, in order).
pub fn html_body(doc: &Document) -> mailalm_mime::HtmlBody {
    let mut images: Vec<mailalm_mime::InlineImage> = Vec::new();
    for p in &doc.paragraphs {
        if let Kind::Image(id) = &p.kind
            && !images.iter().any(|i| &i.content_id == id)
            && let Some(image) = doc.image(id)
        {
            images.push(image.to_mime());
        }
    }
    mailalm_mime::HtmlBody { html: to_html(doc), images }
}
