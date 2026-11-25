// Adapted from MailCraft (https://github.com/ZIDAN44/mailcraft), commit c3510d13,
// crates/ui-egui/src/reading/attachments.rs (`decode_pixels`) and crates/ui-egui/src/look/thumbs.rs
// (`pdf_page`, `is_bold`). MIT OR Apache-2.0.
// Changed: only on the user's request, from the approval dialog, never as a thumbnail; on a
// thread of its own with the answer sent back (a hostile file can be slow); a picture is
// refused before decoding when its input is over 20 MB or it has more than 40 megapixels, and
// its decoder's allocation is capped to that; the first five pages of a PDF drawn to the pane's
// width (MailCraft drew one page at a time); the interface's font from egui's own fonts; text
// shown as text (new); errors said rather than dropped; video frames removed.

//! Attachment previews the user asked for, drawn by MailALM itself (PLAN.md, section 10.1): a
//! picture decoded in Rust with size limits, the first pages of a PDF drawn by hayro (pure Rust,
//! nothing of the PDF runs: no JavaScript, no links), and text shown as text. No external program
//! is started.

use std::io::Cursor;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_interpret::font::{FontData, FontQuery, StandardFont};
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderCache, RenderSettings};
use mailalm_safety::Preview;

/// The largest picture decoded.
const MAX_IMAGE_INPUT: usize = 20 << 20;
/// The most pixels a picture may have (40 megapixels).
const MAX_PIXELS: u64 = 40_000_000;
/// The widest or tallest picture accepted, whatever its pixel count.
const MAX_IMAGE_SIDE: u32 = 40_000;
/// The largest PDF drawn.
const MAX_PDF: usize = 40 << 20;
/// The pages of a PDF drawn, from the first.
pub const PDF_PAGES: usize = 5;
/// How wide a PDF page is drawn, in pixels.
const PAGE_WIDTH: f32 = 900.0;
/// The most pixels a drawn page may have.
const PAGE_PIXELS: f32 = 16_000_000.0;
/// The most text shown.
const MAX_TEXT: usize = 256 << 10;
/// The longest side of a texture made here, below the graphics card's own limit.
const MAX_SIDE: u32 = 4096;

/// What the preview thread hands back.
pub enum Rendered {
    Image(egui::ColorImage),
    /// The first pages, and how many the PDF has.
    Pages(Vec<egui::ColorImage>, usize),
    Text(String),
    Failed(String),
}

/// The interface's fonts (regular, bold) for the text of a PDF that does not carry its fonts.
type Fonts = (Option<Arc<Vec<u8>>>, Option<Arc<Vec<u8>>>);

/// Starts drawing a preview of `bytes` on a thread of its own. The answer comes on the receiver
/// (and the window is woken); if the thread cannot start or stops, the sender is dropped.
pub fn start(ctx: &egui::Context, kind: Preview, bytes: Arc<Vec<u8>>) -> Receiver<Rendered> {
    let (tx, rx) = mpsc::channel();
    let side = u32::try_from(ctx.input(|i| i.max_texture_side)).unwrap_or(u32::MAX).clamp(2, MAX_SIDE);
    let fonts = if kind == Preview::Pdf { interface_fonts() } else { (None, None) };
    let ctx = ctx.clone();
    let _ = std::thread::Builder::new().name("mailalm-preview".to_owned()).spawn(move || {
        let r = render(kind, bytes, side, fonts);
        let _ = tx.send(r);
        ctx.request_repaint();
    });
    rx
}

fn render(kind: Preview, bytes: Arc<Vec<u8>>, side: u32, fonts: Fonts) -> Rendered {
    match kind {
        Preview::Image => match decode_pixels(&bytes, side) {
            Ok(img) => Rendered::Image(img),
            Err(e) => Rendered::Failed(e),
        },
        Preview::Pdf => match pdf_pages(bytes, side, fonts) {
            Ok((pages, total)) => Rendered::Pages(pages, total),
            Err(e) => Rendered::Failed(e),
        },
        Preview::Text => Rendered::Text(text(&bytes)),
    }
}

/// A picture's pixels, at most `side` pixels on a side. Size and allocation are checked before
/// decoding.
fn decode_pixels(data: &[u8], side: u32) -> Result<egui::ColorImage, String> {
    if data.len() > MAX_IMAGE_INPUT {
        return Err(format!("The picture is larger than {} MB: it is not previewed. Save it to look at it.", MAX_IMAGE_INPUT >> 20));
    }
    let unreadable = |_| "The picture could not be read.".to_owned();
    let (w, h) = image::ImageReader::new(Cursor::new(data)).with_guessed_format().map_err(unreadable)?.into_dimensions().map_err(|_| "The picture could not be read.".to_owned())?;
    if u64::from(w).saturating_mul(u64::from(h)) > MAX_PIXELS || w > MAX_IMAGE_SIDE || h > MAX_IMAGE_SIDE {
        return Err(format!("The picture is {w} x {h} pixels, more than MailALM previews (40 megapixels)."));
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_SIDE);
    limits.max_image_height = Some(MAX_IMAGE_SIDE);
    limits.max_alloc = Some(MAX_PIXELS * 4 + (64 << 20));
    let mut reader = image::ImageReader::new(Cursor::new(data)).with_guessed_format().map_err(unreadable)?;
    reader.limits(limits);
    let decoded = reader.decode().map_err(|e| format!("The picture could not be decoded: {e}"))?;
    let img = if decoded.width() > side || decoded.height() > side { decoded.thumbnail(side, side) } else { decoded }.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    Ok(egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()))
}

/// The first [`PDF_PAGES`] pages of a PDF, and how many it has. hayro runs behind a panic guard
/// (and on its own thread).
fn pdf_pages(data: Arc<Vec<u8>>, side: u32, fonts: Fonts) -> Result<(Vec<egui::ColorImage>, usize), String> {
    if data.len() > MAX_PDF {
        return Err(format!("The PDF is larger than {} MB: it is not previewed. Save it to look at it.", MAX_PDF >> 20));
    }
    if !data.windows(5).take(1024).any(|w| w == b"%PDF-") {
        return Err("This is not a PDF MailALM can draw.".to_owned());
    }
    let regular: Option<FontData> = fonts.0.map(|f| f as FontData);
    let bold: Option<FontData> = fonts.1.map(|f| f as FontData).or_else(|| regular.clone());
    let interpreter = InterpreterSettings {
        font_resolver: Arc::new(move |q: &FontQuery| {
            let f = if is_bold(q) { bold.clone() } else { regular.clone() };
            f.map(|f| (f, 0))
        }),
        ..Default::default()
    };
    let rendered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let pdf = Pdf::new(data).ok()?;
        let pages = pdf.pages();
        let total = pages.len();
        let mut out = Vec::new();
        for n in 0..total.min(PDF_PAGES) {
            let Some(page) = pages.get(n) else { break };
            let (w, h) = page.render_dimensions();
            if !(w.is_finite() && h.is_finite()) || w < 1.0 || h < 1.0 {
                continue;
            }
            // The pane's width, at most 4x, no page over 16 megapixels, no side over the
            // texture limit (a pixel short of it, as rounding may add one).
            let scale = (PAGE_WIDTH / w).min(4.0).min((PAGE_PIXELS / (w * h)).sqrt()).min((side.max(2) - 1) as f32 / w.max(h));
            let settings = PixmapSettings { x_scale: scale, y_scale: scale, bg_color: WHITE };
            let pixmap = hayro::render(page, &RenderCache::new(), &interpreter, &RenderSettings::default(), &settings);
            let (pw, ph) = (usize::from(pixmap.width()), usize::from(pixmap.height()));
            let bytes = pixmap.data_as_u8_slice();
            if pw > 0 && ph > 0 && bytes.len() == pw * ph * 4 {
                out.push(egui::ColorImage::from_rgba_premultiplied([pw, ph], bytes));
            }
        }
        Some((out, total))
    }));
    match rendered {
        Ok(Some((pages, total))) if !pages.is_empty() => Ok((pages, total)),
        Ok(_) => Err("The PDF could not be drawn.".to_owned()),
        Err(_) => Err("The PDF could not be drawn (the renderer stopped on it).".to_owned()),
    }
}

/// Whether a font a PDF asks for is drawn with the bold font: one of the standard fonts' bold
/// faces, or a bold one the PDF names and doesn't carry.
fn is_bold(q: &FontQuery) -> bool {
    use StandardFont::*;
    match q {
        FontQuery::Standard(s) => matches!(s, HelveticaBold | HelveticaBoldOblique | CourierBold | CourierBoldOblique | TimesBold | TimesBoldItalic),
        FontQuery::Fallback(f) => f.is_bold,
    }
}

/// egui's own proportional font, for the PDF's standard fonts (hayro is built without its
/// embedded ones, as MailCraft builds it).
fn interface_fonts() -> Fonts {
    let defs = egui::FontDefinitions::default();
    let regular = defs.font_data.get("Ubuntu-Light").map(|f| {
        let bytes: &[u8] = (*f.font).as_ref();
        Arc::new(bytes.to_vec())
    });
    (regular, None)
}

/// The text of a file, at most [`MAX_TEXT`] of it: UTF-8, UTF-16 with its mark, or else Latin-1;
/// control and invisible characters removed.
fn text(bytes: &[u8]) -> String {
    let sample = bytes.get(..MAX_TEXT).unwrap_or(bytes);
    let pairs = |rest: &[u8], big: bool| -> String {
        let units = rest.chunks_exact(2).map(|c| {
            let (a, b) = (c.first().copied().unwrap_or(0), c.get(1).copied().unwrap_or(0));
            if big { u16::from_be_bytes([a, b]) } else { u16::from_le_bytes([a, b]) }
        });
        char::decode_utf16(units).map(|r| r.unwrap_or('\u{FFFD}')).collect()
    };
    let s = if let Some(rest) = sample.strip_prefix(b"\xFF\xFE") {
        pairs(rest, false)
    } else if let Some(rest) = sample.strip_prefix(b"\xFE\xFF") {
        pairs(rest, true)
    } else {
        let body = sample.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(sample);
        match std::str::from_utf8(body) {
            Ok(s) => s.to_owned(),
            Err(e) if e.error_len().is_none() => String::from_utf8_lossy(body.get(..e.valid_up_to()).unwrap_or(body)).into_owned(),
            Err(_) => body.iter().map(|&c| char::from(c)).collect(),
        }
    };
    let mut t = mailalm_mime::clean_text(&s);
    if bytes.len() > MAX_TEXT {
        t.push_str(&format!("\n\n[MailALM: only the first {} KB are shown.]", MAX_TEXT >> 10));
    }
    t
}
