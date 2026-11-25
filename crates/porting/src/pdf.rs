//! A message as PDF: its subject, its header (from, to, copies, date, attachments' names) and its
//! text on A4 pages, through `mailalm-print`. The font embedded is egui's own Ubuntu Light, which
//! is in MailALM already: no system font file is read. Characters it lacks (Chinese, Japanese…)
//! print as its empty glyph.

use std::io;
use std::path::Path;

use mailalm_mime::Parsed;
use mailalm_print::{Doc, Fonts};

/// The PDF of a message read by `mailalm_mime::parse`. `date` is its date as the app shows it
/// (in the user's time zone); empty when it has none.
pub fn render(p: &Parsed, date: &str) -> Vec<u8> {
    let list = |people: &[mailalm_model::Person]| people.iter().map(mailalm_mime::format_person).collect::<Vec<_>>().join(", ");
    let mut header = vec![("From".to_owned(), mailalm_mime::format_person(&p.from))];
    if !p.to.is_empty() {
        header.push(("To".to_owned(), list(&p.to)));
    }
    if !p.cc.is_empty() {
        header.push(("Cc".to_owned(), list(&p.cc)));
    }
    if !date.is_empty() {
        header.push(("Date".to_owned(), date.to_owned()));
    }
    if !p.attachments.is_empty() {
        let names = p.attachments.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ");
        header.push(("Attachments".to_owned(), names));
    }
    let doc = Doc { title: p.subject.clone(), header, body: p.text.clone() };
    mailalm_print::pdf(&doc, Some(Fonts { regular: epaint_default_fonts::UBUNTU_LIGHT, bold: None }))
}

/// Writes the PDF of the message `raw` to the file `to` (written beside, then renamed over it).
pub fn export(raw: &[u8], date: &str, to: &Path) -> io::Result<()> {
    let parsed = mailalm_mime::parse(raw).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "This message could not be read."))?;
    mailalm_store::write_atomic(to, &render(&parsed, date))
}
