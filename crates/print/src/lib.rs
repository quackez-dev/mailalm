// Adapted from MailCraft (https://github.com/ZIDAN44/mailcraft), commit c3510d13, crates/print/src/lib.rs
// and crates/print/src/layout.rs. MIT OR Apache-2.0.
// Changed: plain text only: the formatted text, the laid-out blocks (tables, boxes), the pictures and
// the links (`rich.rs`, `blocks.rs`, `sheet.rs`, `objects.rs`, `picture.rs`, and with them the `image`
// dependency) are left out; `layout.rs` is folded in, for the subject, the header and the text alone;
// without a bold font the bold text is drawn with the regular one, stroked, instead of Helvetica-Bold;
// the text laid out is capped (`MAX_BODY`); the producer is MailALM; the tests embed egui's font
// instead of reading the system's.

//! A message on paper: laid out on A4 pages and written as a PDF (ISO 32000).
//!
//! The font given is embedded when it can be (a TrueType font, as a CID font with identity
//! encoding and a Unicode map, so the text can be searched and copied); without one the standard
//! Helvetica is used and characters outside Windows-1252 print as `?`. Pages carry the subject, the
//! header, the text and page numbers. Pure Rust, no I/O: the caller writes the bytes.

use std::collections::BTreeMap;
use std::fmt::Write as _;

/// What goes on the pages.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Doc {
    /// Printed large at the top (the subject).
    pub title: String,
    /// `From`, `To`, `Date`… lines under it.
    pub header: Vec<(String, String)>,
    /// The text.
    pub body: String,
}

/// Font files to embed (TrueType, or OpenType with TrueType outlines). Without `bold`, bold text
/// is drawn with `regular`, stroked.
#[derive(Clone, Copy, Debug)]
pub struct Fonts<'a> {
    pub regular: &'a [u8],
    pub bold: Option<&'a [u8]>,
}

const PAGE_W: f32 = 595.28;
const PAGE_H: f32 = 841.89;
const MARGIN: f32 = 56.0;
/// The lowest a line of text goes on a page (under it: the page's number).
const BOTTOM: f32 = MARGIN + 20.0;
/// The text's size and the distance between its lines.
const SIZE: f32 = 11.0;
const LEAD: f32 = 14.5;
/// Most pages written (a very long message is cut, with a note).
pub const MAX_PAGES: usize = 500;
/// The most of the text laid out, in bytes: far more than 500 pages hold, and a bound on the work
/// done for a hostile message before the pages run out.
const MAX_BODY: usize = 4 << 20;
/// The most of the subject and of each header line laid out, in characters.
const MAX_LINE: usize = 4096;
/// Header lines printed at most.
const MAX_HEADER_LINES: usize = 64;

/// A font as the layout and the writer see it.
enum Face<'a> {
    Embedded { data: &'a [u8], font: Box<Font<'a>>, used: BTreeMap<u16, char> },
    Helvetica { bold: bool },
}

/// What the layout and the writer read from a font file to embed (skrifa, in font units).
struct Font<'a> {
    charmap: skrifa::charmap::Charmap<'a>,
    advances: skrifa::metrics::GlyphMetrics<'a>,
    metrics: skrifa::metrics::Metrics,
}

impl Font<'_> {
    /// The glyph drawing `c` (0, the missing glyph, when the font has none: TrueType ids fit 16 bits).
    fn glyph(&self, c: char) -> u16 {
        let of = |c: char| self.charmap.map(c).and_then(|g| u16::try_from(g.to_u32()).ok()).unwrap_or(0);
        match of(c) {
            // The lines a table is drawn with in text, which few text fonts have: the nearest
            // character every font has (they printed as empty boxes).
            0 if ('\u{2500}'..='\u{257F}').contains(&c) => of(match c {
                '\u{2500}' | '\u{2501}' | '\u{2504}' | '\u{2505}' | '\u{2508}' | '\u{2509}' | '\u{254C}' | '\u{254D}' | '\u{2550}' => '-',
                '\u{2502}' | '\u{2503}' | '\u{2506}' | '\u{2507}' | '\u{250A}' | '\u{250B}' | '\u{254E}' | '\u{254F}' | '\u{2551}' => '|',
                _ => '+',
            }),
            glyph => glyph,
        }
    }

    /// How far a glyph moves the pen, in font units.
    fn advance(&self, gid: u16) -> f32 {
        self.advances.advance_width(skrifa::GlyphId::new(u32::from(gid))).unwrap_or(0.0)
    }

    fn units_per_em(&self) -> f32 {
        f32::from(self.metrics.units_per_em.max(1))
    }
}

impl<'a> Face<'a> {
    fn parse(data: &'a [u8]) -> Option<Face<'a>> {
        use skrifa::MetadataProvider as _;
        use skrifa::raw::TableProvider as _;
        let face = skrifa::FontRef::from_index(data, 0).ok()?;
        // CFF outlines can't go in a FontFile2 stream.
        if face.glyf().is_err() {
            return None;
        }
        let (size, at) = (skrifa::instance::Size::unscaled(), skrifa::instance::LocationRef::default());
        let font = Font { charmap: face.charmap(), advances: face.glyph_metrics(size, at), metrics: face.metrics(size, at) };
        Some(Face::Embedded { data, font: Box::new(font), used: BTreeMap::new() })
    }

    /// Width of a character in thousandths of the font size.
    fn width(&self, c: char) -> f32 {
        match self {
            Face::Embedded { font, .. } => font.advance(font.glyph(c)) * 1000.0 / font.units_per_em(),
            Face::Helvetica { bold } => helvetica_width(c, *bold),
        }
    }

    fn text_width(&self, s: &str, size: f32) -> f32 {
        s.chars().map(|c| self.width(c)).sum::<f32>() * size / 1000.0
    }

    /// The bytes of a PDF string drawing `s` with this font.
    fn encode(&mut self, s: &str) -> String {
        match self {
            Face::Embedded { font, used, .. } => {
                let mut hex = String::with_capacity(s.len() * 4 + 2);
                hex.push('<');
                for c in s.chars() {
                    let gid = font.glyph(c);
                    used.entry(gid).or_insert(c);
                    let _ = write!(hex, "{gid:04X}");
                }
                hex.push('>');
                hex
            }
            Face::Helvetica { .. } => {
                let mut lit = String::from("(");
                for c in s.chars() {
                    match win_ansi(c) {
                        b'(' | b')' | b'\\' => {
                            lit.push('\\');
                            lit.push(char::from(win_ansi(c)));
                        }
                        b if b >= 0x80 => {
                            let _ = write!(lit, "\\{b:03o}");
                        }
                        b => lit.push(char::from(b)),
                    }
                }
                lit.push(')');
                lit
            }
        }
    }
}

/// The regular face, and the bold one when there is one of its own (else bold is the regular
/// face, stroked).
struct Faces<'a> {
    regular: Face<'a>,
    bold: Option<Face<'a>>,
}

impl<'a> Faces<'a> {
    /// The fonts given when the regular one can be embedded, else Helvetica and Helvetica-Bold.
    fn of(fonts: Option<Fonts<'a>>) -> Faces<'a> {
        match fonts.and_then(|f| Face::parse(f.regular).map(|r| (r, f.bold))) {
            Some((regular, bold)) => Faces { regular, bold: bold.and_then(Face::parse) },
            None => Faces { regular: Face::Helvetica { bold: false }, bold: Some(Face::Helvetica { bold: true }) },
        }
    }

    fn get(&self, bold: bool) -> &Face<'a> {
        match (&self.bold, bold) {
            (Some(b), true) => b,
            _ => &self.regular,
        }
    }

    fn get_mut(&mut self, bold: bool) -> &mut Face<'a> {
        match (&mut self.bold, bold) {
            (Some(b), true) => b,
            _ => &mut self.regular,
        }
    }

    /// Bold text is the regular font, stroked.
    fn stroked_bold(&self) -> bool {
        self.bold.is_none()
    }
}

/// Windows-1252 code of a character (`?` when it has none).
fn win_ansi(c: char) -> u8 {
    match c {
        ' '..='~' => c as u8,
        '\u{A0}'..='\u{FF}' => u8::try_from(u32::from(c)).unwrap_or(b'?'),
        '€' => 0x80,
        '‚' => 0x82,
        '„' => 0x84,
        '…' => 0x85,
        '‘' => 0x91,
        '’' => 0x92,
        '“' => 0x93,
        '”' => 0x94,
        '•' => 0x95,
        '–' => 0x96,
        '—' => 0x97,
        '™' => 0x99,
        _ => b'?',
    }
}

/// Helvetica's widths (the standard metrics of the PDF base fonts) for printable ASCII; other
/// characters get the width of `n`.
fn helvetica_width(c: char, bold: bool) -> f32 {
    const REGULAR: [u16; 95] = [
        278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 278, 278,
        584, 584, 584, 556, 1015, 667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944,
        667, 667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556, 556, 556, 333, 500,
        278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
    ];
    const BOLD: [u16; 95] = [
        278, 333, 474, 556, 556, 889, 722, 238, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 333, 333,
        584, 584, 584, 611, 975, 722, 722, 722, 722, 667, 611, 778, 722, 278, 556, 722, 611, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944,
        667, 667, 611, 333, 278, 333, 584, 556, 333, 556, 611, 556, 611, 556, 333, 611, 611, 278, 278, 556, 278, 889, 611, 611, 611, 611, 389, 556,
        333, 611, 556, 778, 556, 556, 500, 389, 280, 389, 584,
    ];
    let table = if bold { &BOLD } else { &REGULAR };
    let i = (c as usize).wrapping_sub(32);
    f32::from(table.get(i).copied().unwrap_or(556))
}

/// A line of text to place: bold or not, size, position.
#[derive(Debug, Default)]
struct Placed {
    bold: bool,
    size: f32,
    x: f32,
    y: f32,
    text: String,
}

/// Split `text` into lines no wider than `width` (words, then characters for long words).
fn wrap(face: &Face<'_>, text: &str, size: f32, width: f32) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.replace("\r\n", "\n").split('\n') {
        let para = para.replace('\t', "    ");
        let mut line = String::new();
        // Keep leading spaces (quoting, indentation).
        let lead: String = para.chars().take_while(|c| *c == ' ').collect();
        let words: Vec<&str> = para.split(' ').filter(|w| !w.is_empty()).collect();
        if words.is_empty() {
            out.push(String::new());
            continue;
        }
        line.push_str(&lead);
        for w in words {
            let candidate = if line.trim().is_empty() { format!("{line}{w}") } else { format!("{line} {w}") };
            if face.text_width(&candidate, size) <= width {
                line = candidate;
                continue;
            }
            if !line.trim().is_empty() {
                out.push(std::mem::take(&mut line));
            }
            // A word wider than the line is cut by characters.
            let cut = cut_word(
                w,
                size,
                width,
                |c| face.width(c),
                |done, _| {
                    out.push(done);
                    true
                },
            );
            line = cut.map(|(rest, _)| rest).unwrap_or_default();
        }
        out.push(line);
    }
    out
}

/// Cut `word`, wider than `width`, by characters, `advance` giving each one's width in
/// thousandths of `size`: `full` is given each full piece and its width, and says whether to go
/// on. What is left of the word and its width; `None` when `full` said to stop.
///
/// The width is added up as the piece grows, not measured again at each character: characters
/// that take no room (a long run of accents) never fill a piece, which made that quadratic.
fn cut_word(word: &str, size: f32, width: f32, advance: impl Fn(char) -> f32, mut full: impl FnMut(String, f32) -> bool) -> Option<(String, f32)> {
    let mut piece = String::new();
    // In thousandths of the size, added in the order [`Face::text_width`] adds them (from -0).
    let mut units = -0.0f32;
    for c in word.chars() {
        let w = advance(c);
        let grown = units + w;
        if grown * size / 1000.0 > width && !piece.is_empty() {
            if !full(std::mem::take(&mut piece), units * size / 1000.0) {
                return None;
            }
            piece.push(c);
            units = w;
        } else {
            piece.push(c);
            units = grown;
        }
    }
    Some((piece, units * size / 1000.0))
}

/// The first `max` characters of `s`.
fn capped(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// The first [`MAX_BODY`] bytes of `s`, cut on a character's boundary, and whether it was cut.
fn capped_body(s: &str) -> (&str, bool) {
    if s.len() <= MAX_BODY {
        return (s, false);
    }
    let mut end = MAX_BODY;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    (s.get(..end).unwrap_or(""), true)
}

/// The pages, and the line under the header: its page, and how high on it.
struct Laid {
    pages: Vec<Vec<Placed>>,
    rule: (usize, f32),
}

/// Lay `doc` out under `title`.
fn layout(doc: &Doc, title: &str, faces: &Faces<'_>) -> Laid {
    let width = PAGE_W - 2.0 * MARGIN;
    let mut pages: Vec<Vec<Placed>> = vec![Vec::new()];
    let mut y = PAGE_H - MARGIN;
    for line in wrap(faces.get(true), &capped(title, MAX_LINE), 16.0, width) {
        place(&mut pages, &mut y, true, 16.0, 21.0, MARGIN, line);
    }
    y -= 6.0;
    let header = doc.header.get(..MAX_HEADER_LINES).unwrap_or(doc.header.as_slice());
    let label_w = header.iter().map(|(k, _)| faces.get(true).text_width(&format!("{}:", capped(k, 64)), 9.5)).fold(0.0f32, f32::max) + 6.0;
    for (k, v) in header {
        let lines = wrap(faces.get(false), &capped(v, MAX_LINE), 9.5, width - label_w);
        for (i, line) in lines.into_iter().enumerate() {
            if i == 0 {
                let ok = place(&mut pages, &mut y, true, 9.5, 13.0, MARGIN, format!("{}:", capped(k, 64)));
                if ok && let Some(p) = pages.last_mut() {
                    p.push(Placed { bold: false, size: 9.5, x: MARGIN + label_w, y, text: line });
                }
            } else {
                place(&mut pages, &mut y, false, 9.5, 13.0, MARGIN + label_w, line);
            }
        }
    }
    y -= 8.0;
    let rule = (pages.len().saturating_sub(1), y);
    y -= 6.0;
    let (body, cut) = capped_body(&doc.body);
    // (The lines after one with no page left aren't placed.)
    let truncated = wrap(faces.get(false), body, SIZE, width).into_iter().any(|line| !place(&mut pages, &mut y, false, SIZE, LEAD, MARGIN, line));
    if truncated || cut {
        place(&mut pages, &mut y, true, 11.0, 18.0, MARGIN, "(The rest of the message didn't fit in this document.)".into());
    }
    Laid { pages, rule }
}

/// Room on the page under `*y` for a line `lead` tall: a new page when it doesn't fit above the
/// bottom of this one (false when there are as many pages as are written).
fn room_for(pages: &mut Vec<Vec<Placed>>, y: &mut f32, lead: f32) -> bool {
    if *y - lead < BOTTOM {
        if pages.len() >= MAX_PAGES {
            return false;
        }
        pages.push(Vec::new());
        *y = PAGE_H - MARGIN;
    }
    true
}

/// A line of `text` placed under `*y` (which moves to it), on a new page when it doesn't fit
/// on this one; false when there's no page for it.
fn place(pages: &mut Vec<Vec<Placed>>, y: &mut f32, bold: bool, size: f32, lead: f32, x: f32, text: String) -> bool {
    if !room_for(pages, y, lead) {
        return false;
    }
    *y -= lead;
    if let Some(p) = pages.last_mut() {
        p.push(Placed { bold, size, x, y: *y, text });
    }
    true
}

fn compress(data: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec_zlib(data, 6)
}

/// PDF text-string literal of `s` (for the document info), escaped; non-ASCII as `?`.
fn info_string(s: &str) -> String {
    let mut out = String::from("(");
    for c in s.chars().take(200) {
        match c {
            '(' | ')' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            ' '..='~' => out.push(c),
            _ => out.push('?'),
        }
    }
    out.push(')');
    out
}

/// A stream object of `data` compressed; `extra`: more of its dictionary, after its length.
fn flate_stream(extra: &str, data: &[u8]) -> Vec<u8> {
    let packed = compress(data);
    let mut s = format!("<< /Length {}{extra} /Filter /FlateDecode >>\nstream\n", packed.len()).into_bytes();
    s.extend_from_slice(&packed);
    s.extend_from_slice(b"\nendstream");
    s
}

/// Add an object; its number.
fn add(objects: &mut Vec<Vec<u8>>, body: Vec<u8>) -> usize {
    objects.push(body);
    objects.len()
}

/// The document as PDF bytes.
pub fn pdf(doc: &Doc, fonts: Option<Fonts<'_>>) -> Vec<u8> {
    let mut faces = Faces::of(fonts);
    let title = if doc.title.trim().is_empty() { "(no subject)".to_string() } else { doc.title.clone() };
    let laid = layout(doc, &title, &faces);
    let n = laid.pages.len();
    // Written before the fonts: a page's text names the glyphs a font embeds.
    let contents: Vec<String> =
        laid.pages.iter().enumerate().map(|(i, page)| content_stream(&mut faces, page, (i == laid.rule.0).then_some(laid.rule.1), i + 1, n)).collect();

    // Objects: 1 catalog, 2 page tree, 3 info, then fonts, and page + content pairs.
    let info = format!("<< /Producer (MailALM) /Title {} >>", info_string(&title)).into_bytes();
    let mut objects: Vec<Vec<u8>> = vec![b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(), Vec::new(), info];
    let mut fonts_dict = format!("/F1 {} 0 R", font_objects(&faces.regular, 0, &mut objects));
    if let Some(bold) = &faces.bold {
        let _ = write!(fonts_dict, " /F2 {} 0 R", font_objects(bold, 1, &mut objects));
    }
    let mut kids = Vec::new();
    for c in &contents {
        let content_ref = add(&mut objects, flate_stream("", c.as_bytes()));
        let page = format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PAGE_W:.2} {PAGE_H:.2}] /Resources << /Font << {fonts_dict} >> >> /Contents {content_ref} 0 R >>"
        );
        kids.push(add(&mut objects, page.into_bytes()));
    }
    if let Some(o) = objects.get_mut(1) {
        let k: String = kids.iter().map(|r| format!("{r} 0 R")).collect::<Vec<_>>().join(" ");
        *o = format!("<< /Type /Pages /Kids [{k}] /Count {} >>", kids.len()).into_bytes();
    }
    write_file(&objects)
}

/// Page `number` of `of`'s content stream: the line under the header when it's on it (`rule`,
/// how high), the text, and the page's number.
fn content_stream(faces: &mut Faces<'_>, page: &[Placed], rule: Option<f32>, number: usize, of: usize) -> String {
    let mut c = String::new();
    if let Some(rule_y) = rule {
        let _ = writeln!(c, "0.75 G 0.6 w {MARGIN:.2} {rule_y:.2} m {:.2} {rule_y:.2} l S 0 G", PAGE_W - MARGIN);
    }
    let stroked = faces.stroked_bold();
    c.push_str("BT\n");
    for p in page {
        let enc = faces.get_mut(p.bold).encode(&p.text);
        let font = if p.bold && !stroked { 2 } else { 1 };
        if p.bold && stroked {
            // Bold from the regular font: filled and outlined.
            let _ = write!(c, "2 Tr {:.2} w ", p.size * 0.03);
        }
        let _ = write!(c, "/F{font} {:.1} Tf 1 0 0 1 {:.2} {:.2} Tm {enc} Tj", p.size, p.x, p.y);
        c.push_str(if p.bold && stroked { " 0 Tr\n" } else { "\n" });
    }
    let footer = format!("{number} / {of}");
    let fw = faces.regular.text_width(&footer, 8.0);
    let enc = faces.regular.encode(&footer);
    let _ = writeln!(c, "0.45 g /F1 8.0 Tf 1 0 0 1 {:.2} {:.2} Tm {enc} Tj 0 g", (PAGE_W - fw) / 2.0, MARGIN / 2.0);
    c.push_str("ET\n");
    c
}

/// Write the `i`th font's objects into `objects`; the number of the one a page names. An
/// embedded font is its file, its descriptor, a CID font of the glyphs used, and a map from
/// them back to the characters they draw (so the text can be searched and copied).
fn font_objects(face: &Face<'_>, i: usize, objects: &mut Vec<Vec<u8>>) -> usize {
    let (data, font, used) = match face {
        Face::Helvetica { bold } => {
            let name = if *bold { "Helvetica-Bold" } else { "Helvetica" };
            return add(objects, format!("<< /Type /Font /Subtype /Type1 /BaseFont /{name} /Encoding /WinAnsiEncoding >>").into_bytes());
        }
        Face::Embedded { data, font, used } => (data, font, used),
    };
    let upem = font.units_per_em();
    let scale = |v: f32| (v * 1000.0 / upem).round() as i32;
    let file_ref = add(objects, flate_stream(&format!(" /Length1 {}", data.len()), data));
    let m = &font.metrics;
    let bbox = m.bounds.unwrap_or(skrifa::metrics::BoundingBox { x_min: 0.0, y_min: 0.0, x_max: 0.0, y_max: 0.0 });
    let descriptor = format!(
        "<< /Type /FontDescriptor /FontName /MailALMFont{i} /Flags 32 /FontBBox [{} {} {} {}] /ItalicAngle 0 /Ascent {} /Descent {} /CapHeight {} /StemV {} /FontFile2 {file_ref} 0 R >>",
        scale(bbox.x_min),
        scale(bbox.y_min),
        scale(bbox.x_max),
        scale(bbox.y_max),
        scale(m.ascent),
        scale(m.descent),
        scale(m.cap_height.unwrap_or(m.ascent)),
        if i == 1 { 140 } else { 80 }
    );
    let desc_ref = add(objects, descriptor.into_bytes());
    let mut w = String::from("[");
    for gid in used.keys() {
        let _ = write!(w, " {gid} [{}]", scale(font.advance(*gid)));
    }
    w.push_str(" ]");
    let cid = format!(
        "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /MailALMFont{i} /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor {desc_ref} 0 R /DW 1000 /W {w} /CIDToGIDMap /Identity >>"
    );
    let cid_ref = add(objects, cid.into_bytes());
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def /CMapName /Adobe-Identity-UCS def /CMapType 2 def 1 begincodespacerange <0000> <FFFF> endcodespacerange\n",
    );
    let pairs: Vec<(&u16, &char)> = used.iter().collect();
    for chunk in pairs.chunks(100) {
        let _ = writeln!(cmap, "{} beginbfchar", chunk.len());
        for (gid, c) in chunk {
            let mut units = [0u16; 2];
            let hex: String = c.encode_utf16(&mut units).iter().map(|u| format!("{u:04X}")).collect();
            let _ = writeln!(cmap, "<{gid:04X}> <{hex}>");
        }
        cmap.push_str("endbfchar\n");
    }
    cmap.push_str("endcmap CMapName currentdict /CMap defineresource pop end end");
    let tu_ref = add(objects, flate_stream("", cmap.as_bytes()));
    add(
        objects,
        format!("<< /Type /Font /Subtype /Type0 /BaseFont /MailALMFont{i} /Encoding /Identity-H /DescendantFonts [{cid_ref} 0 R] /ToUnicode {tu_ref} 0 R >>")
            .into_bytes(),
    )
}

/// The file: header, `objects` (numbered from 1), cross-reference table, trailer.
fn write_file(objects: &[Vec<u8>]) -> Vec<u8> {
    let mut out: Vec<u8> = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R /Info 3 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    out
}

#[cfg(test)]
mod tests;
