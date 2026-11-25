//! MailALM's rich-text editor, written for MailALM on egui (decision of 2026-11-25: not
//! WordCraft's engine).
//!
//! - The model ([`Document`]): paragraphs (normal, heading 1-3, bulleted or numbered list item
//!   with a nesting level, quoted any number of times, part of the signature or not) of runs of
//!   text with marks (bold, italic, underline, strike), a palette colour and a link
//!   (`https`, `http` or `mailto` only); pictures (PNG, JPEG, GIF, checked by their content and
//!   capped) in paragraphs of their own.
//! - The edits ([`Editor`]): typing, deleting, Enter (lists go on, an empty item ends them),
//!   marks, kinds, indenting, links, pictures, copy and paste, word moves, undo and redo. All
//!   pure, so they are tested without a window.
//! - The widget ([`RichEditor`]) and its [`toolbar`].
//! - What is sent: [`to_html`] (a small, safe HTML), [`plain_text`] (its alternative) and
//!   [`html_body`] (both, with the pictures, for `mailalm_mime::Outgoing::html`).
//! - What is quoted or pasted: [`import_html`], from untrusted HTML to the model, fetching
//!   nothing.
//! - What a corrector needs: [`TextMap`] and [`Editor::replace_text`], over the document's text
//!   in UTF-8 byte offsets (Antidote's `Texteur` is built on them by the compose window).
#![cfg_attr(test, allow(clippy::unwrap_used))]

mod editor;
mod html_in;
mod html_out;
mod model;
mod picture;
mod text;
mod widget;

pub use editor::{Editor, ListKind, MAX_PARAGRAPHS, MAX_TEXT, MAX_UNDO};
pub use html_in::{MAX_IMPORT_HTML, import_html};
pub use html_out::{html_body, plain_text, to_html};
pub use model::{Color, Document, Format, Kind, Link, MAX_LEVEL, MAX_QUOTE, Mark, OBJECT, Paragraph, Pos, Run, Selection};
pub use picture::{ImageError, InlineImage, MAX_IMAGE_BYTES, MAX_IMAGE_PIXELS, MAX_IMAGE_SIDE, MAX_IMAGES, MAX_IMAGES_BYTES, is_valid_content_id, sniff};
pub use text::TextMap;
pub use widget::{RichEditor, ToolbarRequest, ToolbarState, toolbar};

#[cfg(test)]
mod tests;
