//! The editor: a [`Document`], the selection, and every edit, with undo and redo. Everything
//! here is pure (no egui): the widget turns keys and clicks into these calls, and the tests call
//! them directly.

use crate::model::{
    CharClass, Color, Document, Format, Kind, Link, MAX_LEVEL, MAX_QUOTE, Mark, Paragraph, Pos, Run, Selection, char_class, clean,
    next_boundary, prev_boundary,
};
use crate::picture::{ImageError, InlineImage, MAX_IMAGES, MAX_IMAGES_BYTES};
use crate::widget::ViewState;

/// The most undo steps kept.
pub const MAX_UNDO: usize = 200;
/// The most text a document holds, in bytes: what is typed or pasted beyond is dropped.
pub const MAX_TEXT: usize = 4 << 20;
/// The most paragraphs a document holds.
pub const MAX_PARAGRAPHS: usize = 50_000;

/// What an edit was, to group typing into one undo step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditKind {
    Typing,
    Other,
}

#[derive(Clone, Debug)]
struct Snapshot {
    doc: Document,
    sel: Selection,
}

/// A list kind, for [`Editor::toggle_list`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListKind {
    Bullets,
    Numbers,
}

/// A rich-text message being written.
#[derive(Clone, Debug)]
pub struct Editor {
    doc: Document,
    sel: Selection,
    /// The format the next typed text takes, when it was chosen with nothing selected (Ctrl+B
    /// then typing). Forgotten when the caret moves.
    pending: Option<Format>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    last_edit: Option<EditKind>,
    revision: u64,
    /// The last copy: its text, and the same with its formatting, pasted back when the text
    /// pasted is that one.
    clipboard: Option<(String, Document)>,
    /// Ctrl+K was pressed: the toolbar asks for a link.
    link_requested: bool,
    /// What the widget keeps between frames.
    pub(crate) view: ViewState,
}

impl Default for Editor {
    fn default() -> Self {
        Editor::new()
    }
}

impl Editor {
    /// An empty message.
    pub fn new() -> Editor {
        Editor::from_document(Document::new())
    }

    /// An editor on `doc`, the caret at its start.
    pub fn from_document(mut doc: Document) -> Editor {
        doc.normalize();
        Editor {
            doc,
            sel: Selection::default(),
            pending: None,
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit: None,
            revision: 0,
            clipboard: None,
            link_requested: false,
            view: ViewState::default(),
        }
    }

    pub fn document(&self) -> &Document {
        &self.doc
    }

    /// Replaces the whole document (undo history cleared), the caret at its start.
    pub fn set_document(&mut self, mut doc: Document) {
        doc.normalize();
        self.doc = doc;
        self.sel = Selection::default();
        self.pending = None;
        self.undo.clear();
        self.redo.clear();
        self.last_edit = None;
        self.revision = self.revision.wrapping_add(1);
    }

    /// A number that changes with every change of the document.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn selection(&self) -> Selection {
        self.sel
    }

    /// Sets the selection (moved inside the document).
    pub fn set_selection(&mut self, sel: Selection) {
        self.sel = Selection { anchor: self.doc.clamp(sel.anchor), head: self.doc.clamp(sel.head) };
        self.moved();
    }

    /// Puts the caret at `pos`, or extends the selection to it.
    pub fn move_to(&mut self, pos: Pos, extend: bool) {
        let pos = self.doc.clamp(pos);
        self.sel = if extend { Selection { anchor: self.sel.anchor, head: pos } } else { Selection::caret(pos) };
        self.moved();
    }

    fn moved(&mut self) {
        self.pending = None;
        self.last_edit = None;
    }

    pub fn select_all(&mut self) {
        self.set_selection(Selection::new(Pos::default(), self.doc.end()));
    }

    /// Selects the word (or the run of spaces or of punctuation) at `pos`.
    pub fn select_word_at(&mut self, pos: Pos) {
        let pos = self.doc.clamp(pos);
        let chars = self.chars(pos.para);
        let len = chars.len();
        if len == 0 {
            return self.set_selection(Selection::caret(pos));
        }
        let probe = if pos.offset >= len { len - 1 } else { pos.offset };
        let class = chars.get(probe).map_or(CharClass::Space, |c| char_class(*c));
        let mut start = probe;
        while start > 0 && chars.get(start - 1).is_some_and(|c| char_class(*c) == class) {
            start -= 1;
        }
        let mut end = probe + 1;
        while end < len && chars.get(end).is_some_and(|c| char_class(*c) == class) {
            end += 1;
        }
        self.set_selection(Selection::new(Pos::new(pos.para, start), Pos::new(pos.para, end)));
    }

    /// Selects the whole paragraph at `pos`.
    pub fn select_paragraph(&mut self, pos: Pos) {
        let pos = self.doc.clamp(pos);
        self.set_selection(Selection::new(Pos::new(pos.para, 0), Pos::new(pos.para, self.doc.paragraph_len(pos.para))));
    }

    fn chars(&self, para: usize) -> Vec<char> {
        self.doc.paragraph(para).map(Paragraph::chars).unwrap_or_default()
    }

    // ---- Caret movement ----

    /// One character left (to the end of the previous paragraph at a paragraph's start). Without
    /// `extend`, a selection collapses to its start.
    pub fn move_left(&mut self, extend: bool) {
        if !extend && !self.sel.is_empty() {
            let (start, _) = self.sel.range();
            return self.move_to(start, false);
        }
        let target = self.before(self.sel.head);
        self.move_to(target, extend);
    }

    /// One character right.
    pub fn move_right(&mut self, extend: bool) {
        if !extend && !self.sel.is_empty() {
            let (_, end) = self.sel.range();
            return self.move_to(end, false);
        }
        let target = self.after(self.sel.head);
        self.move_to(target, extend);
    }

    /// To the start of the word on the left.
    pub fn move_word_left(&mut self, extend: bool) {
        let target = self.word_left(self.sel.head);
        self.move_to(target, extend);
    }

    /// To the end of the word on the right.
    pub fn move_word_right(&mut self, extend: bool) {
        let target = self.word_right(self.sel.head);
        self.move_to(target, extend);
    }

    pub fn move_to_start(&mut self, extend: bool) {
        self.move_to(Pos::default(), extend);
    }

    pub fn move_to_end(&mut self, extend: bool) {
        let end = self.doc.end();
        self.move_to(end, extend);
    }

    /// The position one character before `pos`.
    fn before(&self, pos: Pos) -> Pos {
        if pos.offset == 0 {
            return match pos.para.checked_sub(1) {
                Some(p) => Pos::new(p, self.doc.paragraph_len(p)),
                None => pos,
            };
        }
        Pos::new(pos.para, prev_boundary(&self.chars(pos.para), pos.offset))
    }

    /// The position one character after `pos`.
    fn after(&self, pos: Pos) -> Pos {
        let len = self.doc.paragraph_len(pos.para);
        if pos.offset >= len {
            return if pos.para + 1 < self.doc.paragraphs.len() { Pos::new(pos.para + 1, 0) } else { pos };
        }
        Pos::new(pos.para, next_boundary(&self.chars(pos.para), pos.offset))
    }

    /// The start of the word before `pos` (spaces skipped first).
    pub fn word_left(&self, pos: Pos) -> Pos {
        let pos = self.doc.clamp(pos);
        if pos.offset == 0 {
            return self.before(pos);
        }
        let chars = self.chars(pos.para);
        let mut i = pos.offset.min(chars.len());
        while i > 0 && chars.get(i - 1).is_some_and(|c| char_class(*c) == CharClass::Space) {
            i -= 1;
        }
        if let Some(class) = i.checked_sub(1).and_then(|j| chars.get(j)).map(|c| char_class(*c)) {
            while i > 0 && chars.get(i - 1).is_some_and(|c| char_class(*c) == class) {
                i -= 1;
            }
        }
        Pos::new(pos.para, i)
    }

    /// The end of the word after `pos` (spaces after it skipped too).
    pub fn word_right(&self, pos: Pos) -> Pos {
        let pos = self.doc.clamp(pos);
        let chars = self.chars(pos.para);
        let len = chars.len();
        if pos.offset >= len {
            return self.after(pos);
        }
        let mut i = pos.offset;
        if let Some(class) = chars.get(i).map(|c| char_class(*c))
            && class != CharClass::Space
        {
            while i < len && chars.get(i).is_some_and(|c| char_class(*c) == class) {
                i += 1;
            }
        }
        while i < len && chars.get(i).is_some_and(|c| char_class(*c) == CharClass::Space) {
            i += 1;
        }
        Pos::new(pos.para, i)
    }

    // ---- Undo ----

    /// Runs one edit: the document before it is kept for undo unless nothing changed (typing is
    /// grouped into one step until the caret moves or something else is done).
    fn edit<R>(&mut self, kind: EditKind, f: impl FnOnce(&mut Editor) -> R) -> R {
        let grouped = kind == EditKind::Typing && self.last_edit == Some(EditKind::Typing);
        let before = (!grouped).then(|| Snapshot { doc: self.doc.clone(), sel: self.sel });
        let result = f(self);
        self.doc.normalize();
        self.sel = Selection { anchor: self.doc.clamp(self.sel.anchor), head: self.doc.clamp(self.sel.head) };
        let changed = match &before {
            Some(snapshot) => snapshot.doc != self.doc,
            None => true,
        };
        if changed {
            if let Some(snapshot) = before {
                self.undo.push(snapshot);
                if self.undo.len() > MAX_UNDO {
                    let extra = self.undo.len() - MAX_UNDO;
                    let _ = self.undo.drain(..extra);
                }
            }
            self.redo.clear();
            self.last_edit = Some(kind);
            self.revision = self.revision.wrapping_add(1);
        }
        result
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Undoes the last edit; whether there was one.
    pub fn undo(&mut self) -> bool {
        let Some(snapshot) = self.undo.pop() else { return false };
        self.redo.push(Snapshot { doc: std::mem::replace(&mut self.doc, snapshot.doc), sel: self.sel });
        self.sel = snapshot.sel;
        self.after_history();
        true
    }

    /// Redoes the last undone edit; whether there was one.
    pub fn redo(&mut self) -> bool {
        let Some(snapshot) = self.redo.pop() else { return false };
        self.undo.push(Snapshot { doc: std::mem::replace(&mut self.doc, snapshot.doc), sel: self.sel });
        self.sel = snapshot.sel;
        self.after_history();
        true
    }

    fn after_history(&mut self) {
        self.sel = Selection { anchor: self.doc.clamp(self.sel.anchor), head: self.doc.clamp(self.sel.head) };
        self.moved();
        self.revision = self.revision.wrapping_add(1);
    }

    // ---- Formats ----

    /// The format typed text takes at the caret.
    pub fn typing_format(&self) -> Format {
        if let Some(f) = &self.pending {
            return f.clone();
        }
        let head = self.sel.head;
        self.doc.paragraph(head.para).map(|p| p.format_at(head.offset)).unwrap_or_default()
    }

    /// The format the toolbar shows: the selection's first character's, or the typing format.
    pub fn current_format(&self) -> Format {
        if self.sel.is_empty() {
            return self.typing_format();
        }
        let (start, end) = self.sel.range();
        for para in start.para..=end.para {
            let Some(p) = self.doc.paragraph(para) else { break };
            if p.kind.is_image() {
                continue;
            }
            let from = if para == start.para { start.offset } else { 0 };
            let to = if para == end.para { end.offset } else { p.len() };
            if let Some(f) = p.formats_in(from, to).first() {
                return (*f).clone();
            }
        }
        self.typing_format()
    }

    /// Whether the selected text (or the typing format) has `mark` everywhere.
    pub fn mark_active(&self, mark: Mark) -> bool {
        if self.sel.is_empty() {
            return self.typing_format().has(mark);
        }
        let mut any = false;
        let all = self.selected_formats(|f| {
            any = true;
            f.has(mark)
        });
        all && any
    }

    /// Calls `test` on the format of every selected text run; whether it held for all.
    fn selected_formats(&self, mut test: impl FnMut(&Format) -> bool) -> bool {
        let (start, end) = self.sel.range();
        for para in start.para..=end.para {
            let Some(p) = self.doc.paragraph(para) else { break };
            if p.kind.is_image() {
                continue;
            }
            let from = if para == start.para { start.offset } else { 0 };
            let to = if para == end.para { end.offset } else { p.len() };
            if !p.formats_in(from, to).into_iter().all(&mut test) {
                return false;
            }
        }
        true
    }

    /// Changes the format of the selected text.
    fn map_selection(&mut self, f: impl Fn(&mut Format)) {
        let (start, end) = self.sel.range();
        self.edit(EditKind::Other, |ed| {
            for para in start.para..=end.para {
                let from = if para == start.para { start.offset } else { 0 };
                let Some(p) = ed.doc.paragraphs.get_mut(para) else { break };
                let to = if para == end.para { end.offset } else { p.len() };
                p.map_formats(from, to, &f);
            }
        });
    }

    /// Bold, italic, underline or strike on the selection: on unless all of it has it already.
    /// With nothing selected, for the text typed next.
    pub fn toggle_mark(&mut self, mark: Mark) {
        let on = !self.mark_active(mark);
        if self.sel.is_empty() {
            let mut f = self.typing_format();
            f.set(mark, on);
            self.pending = Some(f);
            return;
        }
        self.map_selection(|f| f.set(mark, on));
    }

    /// The colour of the selection (or of the text typed next); `None` for the text's own.
    pub fn set_color(&mut self, color: Option<Color>) {
        if self.sel.is_empty() {
            let mut f = self.typing_format();
            f.color = color;
            self.pending = Some(f);
            return;
        }
        self.map_selection(|f| f.color = color);
    }

    /// Removes marks and colour from the selection (links stay), or from the text typed next.
    pub fn clear_formatting(&mut self) {
        if self.sel.is_empty() {
            self.pending = Some(Format::default());
            return;
        }
        self.map_selection(|f| *f = Format { link: f.link.clone(), ..Format::default() });
    }

    /// The link at the caret (or on the selection's first character).
    pub fn link_at_caret(&self) -> Option<Link> {
        if !self.sel.is_empty() {
            return self.current_format().link;
        }
        let head = self.sel.head;
        let p = self.doc.paragraph(head.para)?;
        let before = head.offset.checked_sub(1).and_then(|b| p.format_of_char(b)).and_then(|f| f.link.clone());
        before.or_else(|| p.format_of_char(head.offset).and_then(|f| f.link.clone()))
    }

    /// The characters around the caret that carry `link` (the whole link).
    fn link_extent(&self, link: &Link) -> Option<(Pos, Pos)> {
        let head = self.sel.head;
        let p = self.doc.paragraph(head.para)?;
        let len = p.len();
        let has = |i: usize| p.format_of_char(i).is_some_and(|f| f.link.as_ref() == Some(link));
        let mut start = head.offset.min(len);
        while start > 0 && has(start - 1) {
            start -= 1;
        }
        let mut end = head.offset.min(len);
        while end < len && has(end) {
            end += 1;
        }
        (start < end).then_some((Pos::new(head.para, start), Pos::new(head.para, end)))
    }

    /// Makes the selection a link to `link` (with nothing selected, changes the link the caret is
    /// in); whether anything changed.
    pub fn set_link(&mut self, link: Link) -> bool {
        if self.sel.is_empty() {
            let Some(current) = self.link_at_caret() else { return false };
            let Some((start, end)) = self.link_extent(&current) else { return false };
            let saved = self.sel;
            self.sel = Selection::new(start, end);
            self.map_selection(|f| f.link = Some(link.clone()));
            self.sel = saved;
            return true;
        }
        self.map_selection(|f| f.link = Some(link.clone()));
        true
    }

    /// Inserts `text` as a link to `link` at the caret (replacing the selection).
    pub fn insert_link(&mut self, text: &str, link: Link) {
        let text = clean(text, false);
        let text = if text.trim().is_empty() { link.as_str().to_owned() } else { text };
        let mut format = self.typing_format();
        format.link = Some(link);
        self.edit(EditKind::Other, |ed| ed.insert_with_format(&text, format));
        self.pending = None;
    }

    /// Removes the links from the selection, or the whole link the caret is in.
    pub fn remove_link(&mut self) {
        if self.sel.is_empty() {
            let Some(current) = self.link_at_caret() else { return };
            let Some((start, end)) = self.link_extent(&current) else { return };
            let saved = self.sel;
            self.sel = Selection::new(start, end);
            self.map_selection(|f| f.link = None);
            self.sel = saved;
            return;
        }
        self.map_selection(|f| f.link = None);
    }

    /// Asks the toolbar for a link prompt (Ctrl+K).
    pub fn request_link(&mut self) {
        self.link_requested = true;
    }

    /// Whether a link prompt was asked for since the last call.
    pub fn take_link_request(&mut self) -> bool {
        std::mem::take(&mut self.link_requested)
    }

    // ---- Paragraph kinds ----

    /// The paragraphs the selection touches.
    fn selected_paragraphs(&self) -> std::ops::RangeInclusive<usize> {
        let (start, end) = self.sel.range();
        start.para..=end.para.min(self.doc.paragraphs.len().saturating_sub(1))
    }

    /// The kind of the paragraph at the caret.
    pub fn current_kind(&self) -> Kind {
        self.doc.paragraph(self.sel.head.para).map(|p| p.kind.clone()).unwrap_or_default()
    }

    /// Changes the selected paragraphs (pictures are left alone).
    fn map_paragraphs(&mut self, f: impl Fn(&mut Paragraph)) {
        let range = self.selected_paragraphs();
        self.edit(EditKind::Other, |ed| {
            for p in ed.doc.paragraphs.iter_mut().take(range.end() + 1).skip(*range.start()) {
                if !p.kind.is_image() {
                    f(p);
                }
            }
        });
    }

    /// Whether every selected text paragraph satisfies `test` (and there is one).
    fn all_paragraphs(&self, test: impl Fn(&Paragraph) -> bool) -> bool {
        let range = self.selected_paragraphs();
        let mut any = false;
        for p in self.doc.paragraphs.iter().take(range.end() + 1).skip(*range.start()) {
            if p.kind.is_image() {
                continue;
            }
            any = true;
            if !test(p) {
                return false;
            }
        }
        any
    }

    /// Makes the selected paragraphs a bulleted or numbered list, or plain paragraphs if they all
    /// are already that list.
    pub fn toggle_list(&mut self, list: ListKind) {
        let kind = match list {
            ListKind::Bullets => Kind::Bullet,
            ListKind::Numbers => Kind::Numbered,
        };
        let off = self.all_paragraphs(|p| p.kind == kind);
        self.map_paragraphs(|p| {
            if off {
                p.kind = Kind::Normal;
                p.level = 0;
            } else {
                if !p.kind.is_list() {
                    p.level = 0;
                }
                p.kind = kind.clone();
            }
        });
    }

    /// Makes the selected paragraphs headings of `level` (1 to 3), or plain paragraphs if they all
    /// are already.
    pub fn toggle_heading(&mut self, level: u8) {
        let level = level.clamp(1, 3);
        let off = self.all_paragraphs(|p| p.kind == Kind::Heading(level));
        self.map_paragraphs(|p| {
            p.kind = if off { Kind::Normal } else { Kind::Heading(level) };
            p.level = 0;
        });
    }

    /// Makes the selected paragraphs plain paragraphs (not lists or headings).
    pub fn set_normal(&mut self) {
        self.map_paragraphs(|p| {
            p.kind = Kind::Normal;
            p.level = 0;
        });
    }

    /// Quotes the selected paragraphs, or takes one quote off if they all are quoted.
    pub fn toggle_quote(&mut self) {
        let range = self.selected_paragraphs();
        let all = self.doc.paragraphs.iter().take(range.end() + 1).skip(*range.start()).all(|p| p.quote > 0);
        self.edit(EditKind::Other, |ed| {
            for p in ed.doc.paragraphs.iter_mut().take(range.end() + 1).skip(*range.start()) {
                if all {
                    p.quote -= 1;
                } else if p.quote == 0 {
                    p.quote = 1;
                }
            }
        });
    }

    /// Nests the selected list items one level deeper; whether the selection is in a list.
    pub fn indent(&mut self) -> bool {
        if !self.all_paragraphs(|p| p.kind.is_list()) {
            return false;
        }
        self.map_paragraphs(|p| p.level = (p.level + 1).min(MAX_LEVEL));
        true
    }

    /// Takes the selected list items one level out (out of the list from the first level);
    /// whether the selection is in a list.
    pub fn outdent(&mut self) -> bool {
        if !self.all_paragraphs(|p| p.kind.is_list()) {
            return false;
        }
        self.map_paragraphs(|p| {
            if p.level == 0 {
                p.kind = Kind::Normal;
            } else {
                p.level -= 1;
            }
        });
        true
    }

    // ---- Text ----

    /// Removes `start..end` (in document order); returns where the caret goes. Two pictures, or
    /// a picture and text, are never merged into one paragraph: the paragraphs between are
    /// removed and both ends stay.
    fn delete_range(&mut self, start: Pos, end: Pos) -> Pos {
        let start = self.doc.clamp(start);
        let end = self.doc.clamp(end);
        if start >= end {
            return start;
        }
        if start.para == end.para {
            if let Some(p) = self.doc.paragraphs.get_mut(start.para) {
                if p.kind.is_image() {
                    // The picture itself is removed.
                    p.kind = Kind::Normal;
                    p.runs.clear();
                    return Pos::new(start.para, 0);
                }
                p.remove(start.offset, end.offset);
            }
            return start;
        }
        let (Some(first), Some(last)) = (self.doc.paragraphs.get(start.para), self.doc.paragraphs.get(end.para)) else { return start };
        let head_image = first.kind.is_image() && start.offset > 0;
        let tail_image = last.kind.is_image() && end.offset == 0;
        let mut head = first.clone();
        if first.kind.is_image() {
            if !head_image {
                // The picture is in the deleted range: what stays of this paragraph is empty.
                head = Paragraph { kind: Kind::Normal, ..first.sibling() };
            }
        } else {
            let _ = head.split_off(start.offset);
        }
        let mut tail = last.clone();
        if last.kind.is_image() {
            if !tail_image {
                tail = Paragraph { kind: Kind::Normal, ..last.sibling() };
            }
        } else {
            tail.runs = tail.split_off(end.offset);
        }
        let replacement: Vec<Paragraph> = if head_image || tail_image {
            vec![head, tail]
        } else {
            if first.kind.is_image() && !last.kind.is_image() {
                // Nothing of the first paragraph is left: the result is the last one's kind.
                head.kind = last.kind.clone();
                head.level = last.level;
            }
            head.runs.extend(tail.runs);
            vec![head]
        };
        self.doc.paragraphs.splice(start.para..=end.para, replacement);
        start
    }

    /// Deletes the selection; whether there was one.
    pub fn delete_selection(&mut self) -> bool {
        if self.sel.is_empty() {
            return false;
        }
        let (start, end) = self.sel.range();
        self.edit(EditKind::Other, |ed| {
            let at = ed.delete_range(start, end);
            ed.sel = Selection::caret(at);
        });
        true
    }

    /// The selection removed (no undo step of its own), for an edit that replaces it.
    fn take_selection(&mut self) {
        if !self.sel.is_empty() {
            let (start, end) = self.sel.range();
            let at = self.delete_range(start, end);
            self.sel = Selection::caret(at);
        }
    }

    /// The size of the document's text, in bytes.
    fn text_size(&self) -> usize {
        self.doc.paragraphs.iter().map(|p| p.runs.iter().map(|r| r.text.len()).sum::<usize>() + 1).sum()
    }

    /// Types `text` at the caret, replacing the selection; line breaks start new paragraphs.
    pub fn insert_text(&mut self, text: &str) {
        let text = clean(text, true);
        if text.is_empty() {
            return;
        }
        let kind = if !text.contains('\n') && text.chars().count() <= 4 && self.sel.is_empty() { EditKind::Typing } else { EditKind::Other };
        // A new word starts a new undo step.
        if kind == EditKind::Typing && text.starts_with(char::is_whitespace) {
            self.last_edit = None;
        }
        self.edit(kind, |ed| {
            ed.take_selection();
            let format = ed.typing_format();
            ed.insert_with_format(&text, format);
        });
        self.pending = None;
    }

    /// Inserts `text` in `format` at the caret (the selection already removed); line feeds start
    /// paragraphs. Text past [`MAX_TEXT`] is dropped.
    fn insert_with_format(&mut self, text: &str, format: Format) {
        let room = MAX_TEXT.saturating_sub(self.text_size());
        let mut end = text.len().min(room);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let text = text.get(..end).unwrap_or("");
        for (n, line) in text.split('\n').enumerate() {
            if n > 0 {
                if self.doc.paragraphs.len() >= MAX_PARAGRAPHS {
                    break;
                }
                self.split_paragraph(false);
            }
            if line.is_empty() {
                continue;
            }
            self.text_paragraph_at_caret();
            let at = self.sel.head;
            if let Some(p) = self.doc.paragraphs.get_mut(at.para) {
                p.insert_runs(at.offset, vec![Run::new(line, format.clone())]);
                self.sel = Selection::caret(Pos::new(at.para, at.offset + line.chars().count()));
            }
        }
    }

    /// When the caret is at a picture, moves it into a new empty paragraph before (caret before
    /// the picture) or after it, so that text can be typed there.
    fn text_paragraph_at_caret(&mut self) {
        let at = self.sel.head;
        let Some(p) = self.doc.paragraphs.get(at.para) else { return };
        if !p.kind.is_image() {
            return;
        }
        let new = p.sibling();
        let index = if at.offset == 0 { at.para } else { at.para + 1 };
        self.doc.paragraphs.insert(index, new);
        self.sel = Selection::caret(Pos::new(index, 0));
    }

    /// Splits the paragraph at the caret; the caret goes to the start of the second part.
    /// `enter`: as the Enter key does (a heading is followed by a plain paragraph).
    fn split_paragraph(&mut self, enter: bool) {
        let at = self.sel.head;
        let Some(p) = self.doc.paragraphs.get_mut(at.para) else { return };
        if p.kind.is_image() {
            let new = p.sibling();
            if at.offset == 0 {
                self.doc.paragraphs.insert(at.para, new);
                self.sel = Selection::caret(Pos::new(at.para + 1, 0));
            } else {
                self.doc.paragraphs.insert(at.para + 1, new);
                self.sel = Selection::caret(Pos::new(at.para + 1, 0));
            }
            return;
        }
        let tail_runs = p.split_off(at.offset);
        let mut next = Paragraph { kind: p.kind.clone(), level: p.level, quote: p.quote, signature: p.signature, runs: tail_runs };
        if enter && matches!(p.kind, Kind::Heading(_)) && next.is_empty() {
            next.kind = Kind::Normal;
        }
        self.doc.paragraphs.insert(at.para + 1, next);
        self.sel = Selection::caret(Pos::new(at.para + 1, 0));
    }

    /// The Enter key: a new paragraph (a list goes on; Enter on an empty item ends the list, or
    /// takes it one level out; Enter on an empty quoted paragraph takes one quote off).
    pub fn enter(&mut self) {
        if self.doc.paragraphs.len() >= MAX_PARAGRAPHS {
            return;
        }
        let format = self.typing_format().without_link();
        self.edit(EditKind::Other, |ed| {
            ed.take_selection();
            let at = ed.sel.head;
            if let Some(p) = ed.doc.paragraphs.get_mut(at.para)
                && p.is_empty()
                && !p.kind.is_image()
            {
                if p.kind.is_list() {
                    if p.level > 0 {
                        p.level -= 1;
                    } else {
                        p.kind = Kind::Normal;
                    }
                    return;
                }
                if p.quote > 0 && !matches!(p.kind, Kind::Heading(_)) {
                    p.quote -= 1;
                    return;
                }
            }
            ed.split_paragraph(true);
        });
        // What is typed next keeps the look of the text before the break.
        self.pending = Some(format);
    }

    /// The Backspace key.
    pub fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        let at = self.sel.head;
        let Some(p) = self.doc.paragraph(at.para) else { return };
        if at.offset > 0 {
            let start = self.before(at);
            self.edit(EditKind::Other, |ed| {
                let pos = ed.delete_range(start, at);
                ed.sel = Selection::caret(pos);
            });
            return;
        }
        // At the start of a paragraph: a list item becomes a paragraph, a quote loses a level,
        // and only then is the paragraph joined to the one before.
        if p.kind.is_list() {
            return self.edit(EditKind::Other, |ed| {
                if let Some(p) = ed.doc.paragraphs.get_mut(at.para) {
                    p.kind = Kind::Normal;
                    p.level = 0;
                }
            });
        }
        let prev_quote = at.para.checked_sub(1).and_then(|i| self.doc.paragraph(i)).map_or(0, |q| q.quote);
        if p.quote > prev_quote && !p.kind.is_image() {
            return self.edit(EditKind::Other, |ed| {
                if let Some(p) = ed.doc.paragraphs.get_mut(at.para) {
                    p.quote -= 1;
                }
            });
        }
        let Some(prev_index) = at.para.checked_sub(1) else {
            if matches!(p.kind, Kind::Heading(_)) {
                self.set_normal();
            }
            return;
        };
        let Some(prev) = self.doc.paragraph(prev_index) else { return };
        let (here_image, prev_image, here_empty, prev_empty) = (p.kind.is_image(), prev.kind.is_image(), p.is_empty(), prev.is_empty());
        self.edit(EditKind::Other, |ed| {
            if prev_image {
                if here_empty {
                    // An empty paragraph after a picture goes; the caret is after the picture.
                    ed.doc.paragraphs.remove(at.para);
                    ed.sel = Selection::caret(Pos::new(prev_index, 1));
                } else {
                    // The picture before goes.
                    ed.doc.paragraphs.remove(prev_index);
                    ed.sel = Selection::caret(Pos::new(prev_index, 0));
                }
            } else if here_image {
                if prev_empty {
                    ed.doc.paragraphs.remove(prev_index);
                    ed.sel = Selection::caret(Pos::new(prev_index, 0));
                } else {
                    ed.sel = Selection::caret(Pos::new(prev_index, ed.doc.paragraph_len(prev_index)));
                }
            } else {
                let end = Pos::new(prev_index, ed.doc.paragraph_len(prev_index));
                let pos = ed.delete_range(end, at);
                ed.sel = Selection::caret(pos);
            }
        });
    }

    /// The Delete key.
    pub fn delete(&mut self) {
        if self.delete_selection() {
            return;
        }
        let at = self.sel.head;
        let Some(p) = self.doc.paragraph(at.para) else { return };
        let len = p.len();
        if at.offset < len {
            let end = self.after(at);
            self.edit(EditKind::Other, |ed| {
                let pos = ed.delete_range(at, end);
                ed.sel = Selection::caret(pos);
            });
            return;
        }
        let next_index = at.para + 1;
        let Some(next) = self.doc.paragraph(next_index) else { return };
        let (here_image, next_image, here_empty, next_empty) = (p.kind.is_image(), next.kind.is_image(), p.is_empty(), next.is_empty());
        self.edit(EditKind::Other, |ed| {
            if next_image {
                if here_empty {
                    ed.doc.paragraphs.remove(at.para);
                    ed.sel = Selection::caret(Pos::new(at.para, 0));
                } else {
                    ed.doc.paragraphs.remove(next_index);
                }
            } else if here_image {
                if next_empty {
                    ed.doc.paragraphs.remove(next_index);
                } else {
                    ed.sel = Selection::caret(Pos::new(next_index, 0));
                }
            } else {
                let pos = ed.delete_range(at, Pos::new(next_index, 0));
                ed.sel = Selection::caret(pos);
            }
        });
    }

    /// Ctrl+Backspace: deletes back to the start of the word.
    pub fn delete_word_backward(&mut self) {
        if self.delete_selection() {
            return;
        }
        let at = self.sel.head;
        if at.offset == 0 {
            return self.backspace();
        }
        let start = self.word_left(at);
        self.edit(EditKind::Other, |ed| {
            let pos = ed.delete_range(start, at);
            ed.sel = Selection::caret(pos);
        });
    }

    /// Ctrl+Delete: deletes to the end of the word.
    pub fn delete_word_forward(&mut self) {
        if self.delete_selection() {
            return;
        }
        let at = self.sel.head;
        if at.offset >= self.doc.paragraph_len(at.para) {
            return self.delete();
        }
        let end = self.word_right(at);
        self.edit(EditKind::Other, |ed| {
            let pos = ed.delete_range(at, end);
            ed.sel = Selection::caret(pos);
        });
    }

    // ---- Pictures ----

    /// Puts a picture at the caret, in a paragraph of its own (the paragraph at the caret is split
    /// there); the caret goes after it.
    pub fn insert_image(&mut self, image: InlineImage) -> Result<(), ImageError> {
        let already = self.doc.images.iter().any(|i| i.content_id == image.content_id);
        let total = self.doc.images.iter().map(|i| i.bytes.len()).sum::<usize>() + image.bytes.len();
        if !already && (self.doc.images.len() >= MAX_IMAGES || total > MAX_IMAGES_BYTES) {
            return Err(ImageError::TooMany);
        }
        let id = image.content_id.clone();
        self.edit(EditKind::Other, |ed| {
            ed.take_selection();
            if !already {
                ed.doc.images.push(image);
            }
            let at = ed.sel.head;
            let Some(p) = ed.doc.paragraphs.get(at.para) else { return };
            let mut picture = Paragraph::image(&id);
            picture.quote = p.quote;
            picture.signature = p.signature;
            let index = if p.kind.is_image() {
                if at.offset == 0 { at.para } else { at.para + 1 }
            } else if at.offset == 0 && p.is_empty() {
                // An empty paragraph becomes the picture.
                ed.doc.paragraphs.remove(at.para);
                at.para
            } else if at.offset == 0 {
                at.para
            } else if at.offset >= p.len() {
                at.para + 1
            } else {
                ed.split_paragraph(false);
                at.para + 1
            };
            ed.doc.paragraphs.insert(index, picture);
            // The caret after the picture: at the start of what follows, or in a new paragraph.
            if ed.doc.paragraphs.get(index + 1).is_none_or(|n| n.kind.is_image()) {
                let new = ed.doc.paragraphs.get(index).map(Paragraph::sibling).unwrap_or_default();
                ed.doc.paragraphs.insert(index + 1, new);
            }
            ed.sel = Selection::caret(Pos::new(index + 1, 0));
        });
        Ok(())
    }

    // ---- Fragments, copy and paste ----

    /// The selection as a document of its own.
    pub fn selected_document(&self) -> Document {
        let (start, end) = self.sel.range();
        let mut paragraphs = Vec::new();
        for para in start.para..=end.para {
            let Some(p) = self.doc.paragraph(para) else { break };
            let from = if para == start.para { start.offset } else { 0 };
            let to = if para == end.para { end.offset } else { p.len() };
            if p.kind.is_image() {
                if from < to {
                    paragraphs.push(p.clone());
                }
                continue;
            }
            paragraphs.push(Paragraph { runs: p.slice(from, to), signature: false, ..p.clone() });
        }
        let mut doc = Document { paragraphs, images: self.doc.images.clone() };
        doc.normalize();
        doc
    }

    /// Copies the selection: returns its text for the system clipboard, and keeps it with its
    /// formatting for a paste in this editor.
    pub fn copy(&mut self) -> Option<String> {
        if self.sel.is_empty() {
            return None;
        }
        let fragment = self.selected_document();
        let text = crate::html_out::plain_text(&fragment);
        self.clipboard = Some((text.clone(), fragment));
        Some(text)
    }

    /// Cuts the selection (see [`Editor::copy`]).
    pub fn cut(&mut self) -> Option<String> {
        let text = self.copy()?;
        self.delete_selection();
        Some(text)
    }

    /// Pastes text from the clipboard: with its formatting when it is what this editor copied
    /// last, else as plain text.
    pub fn paste_text(&mut self, text: &str) {
        let same = self.clipboard.as_ref().is_some_and(|(copied, _)| clean(copied, true) == clean(text, true));
        if same && let Some((_, fragment)) = self.clipboard.clone() {
            return self.insert_document(&fragment);
        }
        self.insert_text(text);
    }

    /// Pastes HTML: imported (see [`crate::import_html`]) and inserted at the caret. `images`
    /// are the pictures its `cid:` addresses may name.
    pub fn paste_html(&mut self, html: &str, images: &[InlineImage]) {
        let fragment = crate::html_in::import_html(html, images);
        self.insert_document(&fragment);
    }

    /// Inserts a document at the caret, replacing the selection, with its formatting. Its first
    /// paragraph continues the one at the caret, and its last is followed by the rest of it.
    pub fn insert_document(&mut self, fragment: &Document) {
        if fragment.is_empty() {
            return;
        }
        let room_paragraphs = MAX_PARAGRAPHS.saturating_sub(self.doc.paragraphs.len());
        let room_text = MAX_TEXT.saturating_sub(self.text_size());
        let mut size = 0usize;
        let mut incoming: Vec<Paragraph> = Vec::new();
        for p in &fragment.paragraphs {
            size += p.runs.iter().map(|r| r.text.len()).sum::<usize>() + 1;
            if incoming.len() >= room_paragraphs || size > room_text {
                break;
            }
            incoming.push(p.clone());
        }
        let mut images: Vec<InlineImage> = Vec::new();
        let mut total = self.doc.images.iter().map(|i| i.bytes.len()).sum::<usize>();
        for p in &incoming {
            if let Kind::Image(id) = &p.kind
                && let Some(image) = fragment.image(id)
                && !self.doc.images.iter().chain(&images).any(|i| i.content_id == image.content_id)
                && self.doc.images.len() + images.len() < MAX_IMAGES
                && total + image.bytes.len() <= MAX_IMAGES_BYTES
            {
                total += image.bytes.len();
                images.push(image.clone());
            }
        }
        self.edit(EditKind::Other, |ed| {
            ed.take_selection();
            ed.doc.images.extend(images);
            let at = ed.sel.head;
            let Some(current) = ed.doc.paragraphs.get(at.para).cloned() else { return };
            let base_quote = current.quote;
            let signature = current.signature;
            let mut incoming = incoming;
            for p in &mut incoming {
                p.quote = p.quote.saturating_add(base_quote).min(MAX_QUOTE);
                p.signature = signature;
            }
            if current.kind.is_image() {
                // Before or after the picture, as paragraphs of their own.
                let index = if at.offset == 0 { at.para } else { at.para + 1 };
                let count = incoming.len();
                let last_len = incoming.last().map_or(0, Paragraph::len);
                ed.doc.paragraphs.splice(index..index, incoming);
                ed.sel = Selection::caret(Pos::new(index + count.saturating_sub(1), last_len));
                return;
            }
            let mut head = current.clone();
            let tail_runs = head.split_off(at.offset);
            let mut out: Vec<Paragraph> = vec![head];
            for (n, p) in incoming.into_iter().enumerate() {
                let joinable = n == 0 && !p.kind.is_image();
                match out.last_mut() {
                    Some(last) if joinable => {
                        if last.is_empty() {
                            // An empty paragraph takes the pasted one's kind.
                            last.kind = p.kind.clone();
                            last.level = p.level;
                            last.quote = p.quote;
                        }
                        last.runs.extend(p.runs);
                    }
                    _ => out.push(p),
                }
            }
            let last_index = out.len() - 1;
            let caret = match out.last_mut() {
                Some(last) if !last.kind.is_image() => {
                    let len = last.len();
                    last.runs.extend(tail_runs);
                    Pos::new(at.para + last_index, len)
                }
                _ => {
                    let rest = Paragraph { kind: current.kind.clone(), level: current.level, quote: current.quote, signature: current.signature, runs: tail_runs };
                    out.push(rest);
                    Pos::new(at.para + last_index + 1, 0)
                }
            };
            ed.doc.paragraphs.splice(at.para..=at.para, out);
            ed.sel = Selection::caret(caret);
        });
        self.pending = None;
    }

    // ---- Signature ----

    /// Replaces the signature (the paragraphs marked as such) with `signature`, where the old one
    /// was, else where [`signature_slot`] says (above a reply's quoted text, or at the end);
    /// `None` removes it.
    pub fn set_signature(&mut self, signature: Option<&Document>) {
        let incoming: Vec<Paragraph> = signature
            .map(|s| s.paragraphs.iter().cloned().map(|mut p| {
                p.signature = true;
                p
            }).collect())
            .unwrap_or_default();
        let images: Vec<InlineImage> = signature.map(|s| s.images.clone()).unwrap_or_default();
        self.edit(EditKind::Other, |ed| {
            let first = ed.doc.paragraphs.iter().position(|p| p.signature);
            ed.doc.paragraphs.retain(|p| !p.signature);
            if ed.doc.paragraphs.is_empty() {
                ed.doc.paragraphs.push(Paragraph::default());
            }
            let at = first.unwrap_or_else(|| signature_slot(&ed.doc)).min(ed.doc.paragraphs.len());
            for image in images {
                if !ed.doc.images.iter().any(|i| i.content_id == image.content_id) {
                    ed.doc.images.push(image);
                }
            }
            ed.doc.paragraphs.splice(at..at, incoming);
            ed.sel = Selection { anchor: ed.doc.clamp(ed.sel.anchor), head: ed.doc.clamp(ed.sel.head) };
        });
    }

    /// The link on the character at `pos` (its `offset` counts characters), and the text of the
    /// characters around it that carry the same link: what a reader sees of the link.
    pub fn link_at(&self, pos: Pos) -> Option<(Link, String)> {
        let p = self.doc.paragraph(pos.para)?;
        let link = p.format_of_char(pos.offset)?.link.clone()?;
        let mut chars: Vec<(char, bool)> = Vec::with_capacity(p.len());
        for run in &p.runs {
            let same = run.format.link.as_ref() == Some(&link);
            chars.extend(run.text.chars().map(|c| (c, same)));
        }
        let linked = |i: usize| chars.get(i).is_some_and(|(_, same)| *same);
        let mut start = pos.offset;
        while start > 0 && linked(start - 1) {
            start -= 1;
        }
        let mut end = pos.offset;
        while linked(end) {
            end += 1;
        }
        let text: String = chars.get(start..end)?.iter().map(|(c, _)| *c).collect();
        Some((link, text))
    }

    // ---- For the text access ----

    /// Replaces `start..end` with `text`, in the format of the first character replaced (or of
    /// the text at `start`); the caret goes after it.
    pub(crate) fn replace_range(&mut self, start: Pos, end: Pos, text: &str) {
        let text = clean(text, true);
        let format = self
            .doc
            .paragraph(start.para)
            .map(|p| if start < end && start.offset < p.len() { p.format_of_char(start.offset).cloned().unwrap_or_default() } else { p.format_at(start.offset) })
            .unwrap_or_default();
        self.edit(EditKind::Other, |ed| {
            let at = ed.delete_range(start, end);
            ed.sel = Selection::caret(at);
            ed.insert_with_format(&text, format);
        });
        self.pending = None;
    }

    /// The document and the widget's state, borrowed together.
    pub(crate) fn parts(&mut self) -> (&Document, &mut ViewState) {
        (&self.doc, &mut self.view)
    }

    /// Checks the invariants (tests only).
    #[cfg(test)]
    pub(crate) fn check(&self) {
        assert!(!self.doc.paragraphs.is_empty(), "no paragraph");
        for (i, p) in self.doc.paragraphs.iter().enumerate() {
            assert!(p.runs.iter().all(|r| !r.text.is_empty()), "empty run in {i}: {p:?}");
            assert!(p.runs.windows(2).all(|w| w[0].format != w[1].format), "unmerged runs in {i}: {p:?}");
            if let Kind::Image(id) = &p.kind {
                assert_eq!(p.len(), 1, "picture {i}");
                assert!(self.doc.image(id).is_some(), "picture {i} without its image");
            } else {
                assert!(!p.text().contains(crate::model::OBJECT), "object character in text {i}");
            }
            assert!(p.kind.is_list() || p.level == 0);
        }
        for pos in [self.sel.anchor, self.sel.head] {
            assert_eq!(self.doc.clamp(pos), pos, "selection out of the document: {pos:?}");
        }
    }
}

/// Where a new signature goes in `doc` (one without a signature): above the quoted text of a
/// reply or a forward (the first quoted paragraph), and above its attribution line ("On …, …
/// wrote:", a line ending with a colon just before it) and the empty lines over that, the first
/// two lines staying above (one to write on, one empty); at the end when nothing is quoted.
pub fn signature_slot(doc: &Document) -> usize {
    let Some(first_quoted) = doc.paragraphs.iter().position(|p| p.quote > 0) else { return doc.paragraphs.len() };
    let mut at = first_quoted;
    let attribution = |p: &Paragraph| p.quote == 0 && !p.kind.is_image() && !p.is_empty() && p.text().trim_end().ends_with(':');
    if at > 0 && doc.paragraphs.get(at - 1).is_some_and(attribution) {
        at -= 1;
    }
    while at > 2 && doc.paragraphs.get(at - 1).is_some_and(|p| p.quote == 0 && p.is_empty() && !p.kind.is_image()) {
        at -= 1;
    }
    at
}
