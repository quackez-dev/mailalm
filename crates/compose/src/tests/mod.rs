//! Tests of the model, the edits, the HTML both ways, the text access and the MIME round trip.
//! The widget itself (egui) is not run here.

mod edit;
mod html;
mod text;

use crate::*;

/// A document of plain lines.
fn doc(lines: &[&str]) -> Document {
    Document::from_text(&lines.join("\n"))
}

fn editor(lines: &[&str]) -> Editor {
    Editor::from_document(doc(lines))
}

/// The paragraphs' texts.
fn texts(ed: &Editor) -> Vec<String> {
    ed.document().paragraphs.iter().map(Paragraph::text).collect()
}

fn at(para: usize, offset: usize) -> Pos {
    Pos::new(para, offset)
}

fn caret(ed: &Editor) -> Pos {
    let sel = ed.selection();
    assert!(sel.is_empty(), "a caret, not a selection: {sel:?}");
    sel.head
}

/// A PNG header of `w` x `h` (enough for the checks, not for decoding).
fn png(w: u32, h: u32) -> Vec<u8> {
    let mut v = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    v.extend(w.to_be_bytes());
    v.extend(h.to_be_bytes());
    v.extend([8, 6, 0, 0, 0]);
    v
}

/// A GIF header of `w` x `h`.
fn gif(w: u16, h: u16) -> Vec<u8> {
    let mut v = b"GIF89a".to_vec();
    v.extend(w.to_le_bytes());
    v.extend(h.to_le_bytes());
    v.extend([0, 0, 0]);
    v
}

/// A JPEG start (SOI, an APP0 segment, then a SOF0 frame header) of `w` x `h`.
fn jpeg(w: u16, h: u16) -> Vec<u8> {
    let mut v = vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10];
    v.extend(b"JFIF\0");
    v.extend([1, 1, 0, 0, 1, 0, 1, 0, 0]);
    v.extend([0xff, 0xc0, 0x00, 0x11, 0x08]);
    v.extend(h.to_be_bytes());
    v.extend(w.to_be_bytes());
    v.extend([3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
    v
}

fn bold() -> Format {
    Format { bold: true, ..Format::default() }
}
