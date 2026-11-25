//! The document as one plain text, for a corrector (Antidote): paragraphs joined by line
//! feeds, pictures left out, and the way back from a byte offset in that text to a position.
//! The compose window builds its `mailalm_antidote::Texteur` on this.

use std::ops::Range;

use crate::editor::Editor;
use crate::model::{Document, Format, Pos, Selection, byte_of_char};

/// The document's text, and where each paragraph starts in it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextMap {
    /// The paragraphs joined by `\n`; a picture is an empty line.
    pub text: String,
    /// Each paragraph's byte range in `text`.
    paragraphs: Vec<Range<usize>>,
}

impl TextMap {
    /// The map of `doc`.
    pub fn new(doc: &Document) -> TextMap {
        let mut text = String::new();
        let mut paragraphs = Vec::with_capacity(doc.paragraphs.len());
        for (i, p) in doc.paragraphs.iter().enumerate() {
            if i > 0 {
                text.push('\n');
            }
            let start = text.len();
            if !p.kind.is_image() {
                for run in &p.runs {
                    text.push_str(&run.text);
                }
            }
            paragraphs.push(start..text.len());
        }
        TextMap { text, paragraphs }
    }

    /// The position of byte offset `byte`; `None` past the end or inside a character. The line
    /// feed between two paragraphs is the end of the first.
    pub fn pos(&self, byte: usize) -> Option<Pos> {
        let index = self.paragraphs.partition_point(|r| r.start <= byte).checked_sub(1)?;
        let range = self.paragraphs.get(index)?;
        if byte > range.end {
            return None;
        }
        let para_text = self.text.get(range.start..range.end)?;
        let local = byte - range.start;
        let head = para_text.get(..local)?;
        Some(Pos::new(index, head.chars().count()))
    }

    /// The byte offset of `pos` (a picture's both sides are its line's start); `None` outside.
    pub fn byte(&self, pos: Pos) -> Option<usize> {
        let range = self.paragraphs.get(pos.para)?;
        let para_text = self.text.get(range.start..range.end)?;
        if para_text.is_empty() {
            return (pos.offset <= 1).then_some(range.start);
        }
        if pos.offset > para_text.chars().count() {
            return None;
        }
        Some(range.start + byte_of_char(para_text, pos.offset))
    }

    /// The byte range of the selection `sel`.
    pub fn range_of(&self, sel: Selection) -> Option<Range<usize>> {
        let (start, end) = sel.range();
        Some(self.byte(start)?..self.byte(end)?)
    }

    /// The byte ranges of the text's runs, with their formats (to tell a corrector how the text
    /// looks).
    pub fn formats(&self, doc: &Document) -> Vec<(Range<usize>, Format)> {
        let mut out = Vec::new();
        for (p, range) in doc.paragraphs.iter().zip(&self.paragraphs) {
            if p.kind.is_image() {
                continue;
            }
            let mut at = range.start;
            for run in &p.runs {
                out.push((at..at + run.text.len(), run.format.clone()));
                at += run.text.len();
            }
        }
        out
    }
}

impl Editor {
    /// The document's text and its map.
    pub fn text_map(&self) -> TextMap {
        TextMap::new(self.document())
    }

    /// The selection as a byte range of [`Editor::text_map`]'s text.
    pub fn selected_text_range(&self) -> Option<Range<usize>> {
        self.text_map().range_of(self.selection())
    }

    /// Replaces the byte range `range` of [`Editor::text_map`]'s text with `text`, keeping the
    /// format of the text replaced (one undo step); whether it was done. A range that does not
    /// fall on characters of the current text is refused.
    pub fn replace_text(&mut self, range: Range<usize>, text: &str) -> bool {
        let map = self.text_map();
        let (Some(start), Some(end)) = (map.pos(range.start), map.pos(range.end)) else { return false };
        if start > end {
            return false;
        }
        self.replace_range(start, end, text);
        true
    }

    /// Selects the byte range `range` of [`Editor::text_map`]'s text; whether it was valid.
    pub fn select_text(&mut self, range: Range<usize>) -> bool {
        let map = self.text_map();
        let (Some(start), Some(end)) = (map.pos(range.start), map.pos(range.end)) else { return false };
        if start > end {
            return false;
        }
        self.set_selection(Selection::new(start, end));
        true
    }
}
