//! The egui side: [`RichEditor`], the widget that shows an [`Editor`] and turns the mouse, the
//! keyboard and the input method into its edits, and [`toolbar`].
//!
//! Each paragraph is laid out on its own with egui's text layout (one section per run), so the
//! caret, the selection and the clicks map to (paragraph, character) through the galleys. The
//! event handling follows egui's `TextEdit` (focus lock for Tab and the arrows, IME output), with
//! the document's own model underneath.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use egui::output::{IMEOutput, IMESurroundingText};
use egui::text::{CCursor, CCursorRange, Galley, LayoutJob, TextFormat};
use egui::text_selection::visuals::{paint_text_cursor, paint_text_selection};
use egui::{
    Align2, Button, Color32, ColorImage, CursorIcon, Event, EventFilter, FontFamily, FontId, IMEPurpose, Id, ImeChangeCause, ImeEvent, Key,
    Modifiers, Pos2, Rect, Response, RichText, Sense, Stroke, TextEdit, TextureHandle, TextureOptions, Ui, Vec2, pos2, vec2,
};

use crate::editor::{Editor, ListKind};
use crate::model::{Color, Document, Format, Kind, Link, Mark, Paragraph, Pos, Selection};
use crate::picture;

/// How far a quote level moves the text right.
const QUOTE_INDENT: f32 = 14.0;
/// How far a list level moves the text right.
const LIST_INDENT: f32 = 24.0;
/// The space between paragraphs.
const PARA_GAP: f32 = 2.0;
/// The space above a heading.
const HEADING_GAP: f32 = 6.0;
/// The tallest a picture is shown.
const MAX_IMAGE_HEIGHT: f32 = 480.0;
/// The largest texture made for a picture.
const MAX_TEXTURE_SIDE: u32 = 2048;
/// How far the faux bold copy is drawn to the right (when no bold font is given).
const FAUX_BOLD_OFFSET: f32 = 0.55;

/// What the widget keeps between frames (inside the [`Editor`]).
#[derive(Clone, Default)]
pub(crate) struct ViewState {
    /// The widget's id, for the toolbar to give it the focus back.
    id: Option<Id>,
    /// The pictures' textures, by content id (`None`: it could not be decoded).
    textures: HashMap<String, Option<TextureHandle>>,
    /// The input method's text being composed, shown at the caret.
    preedit: String,
    /// When the caret last moved, for its blinking.
    last_interaction: f64,
    /// The x the caret keeps when it goes up and down.
    h_pos: Option<f32>,
    ime_generation: u64,
    was_focused: bool,
}

impl fmt::Debug for ViewState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ViewState").field("id", &self.id).field("textures", &self.textures.len()).finish_non_exhaustive()
    }
}

/// A new IME context generation for this widget (a range of its own, apart from egui's).
fn next_ime_generation() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1 << 40);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// The look of the text.
struct Look {
    size: f32,
    family: FontFamily,
    bold_family: Option<FontFamily>,
    text: Color32,
    strong: Color32,
    weak: Color32,
    link: Color32,
    dark: bool,
}

impl Look {
    fn palette(&self, c: Color) -> Color32 {
        let [r, g, b] = c.rgb();
        if self.dark {
            // Lighter on a dark background.
            let up = |v: u8| v + ((255 - v) as f32 * 0.4) as u8;
            Color32::from_rgb(up(r), up(g), up(b))
        } else {
            Color32::from_rgb(r, g, b)
        }
    }

    fn size_of(&self, kind: &Kind) -> f32 {
        match kind {
            Kind::Heading(1) => self.size * 1.6,
            Kind::Heading(2) => self.size * 1.35,
            Kind::Heading(_) => self.size * 1.15,
            _ => self.size,
        }
    }

    /// The format of a run; `overlay`: the faux bold copy (only bold text, nothing else drawn).
    fn format(&self, f: &Format, kind: &Kind, overlay: bool) -> TextFormat {
        let bold = f.bold || matches!(kind, Kind::Heading(_));
        let family = match (&self.bold_family, bold) {
            (Some(b), true) => b.clone(),
            _ => self.family.clone(),
        };
        let mut color = match f.color {
            Some(c) => self.palette(c),
            None if bold => self.strong,
            None => self.text,
        };
        if f.link.is_some() {
            color = self.link;
        }
        let underline = if f.underline || f.link.is_some() { Stroke::new(1.0, color) } else { Stroke::NONE };
        let strike = if f.strike { Stroke::new(1.0, color) } else { Stroke::NONE };
        let mut format = TextFormat {
            font_id: FontId::new(self.size_of(kind), family),
            color,
            italics: f.italic,
            underline,
            strikethrough: strike,
            ..TextFormat::default()
        };
        if overlay {
            format.underline = Stroke::NONE;
            format.strikethrough = Stroke::NONE;
            if !bold {
                format.color = Color32::TRANSPARENT;
            }
        }
        format
    }
}

/// One paragraph laid out.
struct Block {
    /// From the widget's top.
    top: f32,
    height: f32,
    /// Where its text starts, from the widget's left.
    left: f32,
    galley: Option<Arc<Galley>>,
    /// The faux bold copy, if it has bold text and no bold font is given.
    bold_overlay: Option<Arc<Galley>>,
    /// A picture's size on screen.
    image: Option<Vec2>,
    row_height: f32,
    /// Its length in characters.
    len: usize,
}

struct Layout {
    blocks: Vec<Block>,
    height: f32,
}

/// The number of characters of a galley.
fn galley_len(g: &Galley) -> usize {
    g.rows.iter().map(|r| r.char_count_including_newline().0).sum()
}

fn layout(ui: &Ui, doc: &Document, width: f32, look: &Look) -> Layout {
    let mut blocks = Vec::with_capacity(doc.paragraphs.len());
    let mut y = 0.0f32;
    for (i, p) in doc.paragraphs.iter().enumerate() {
        if i > 0 {
            y += if matches!(p.kind, Kind::Heading(_)) { HEADING_GAP } else { PARA_GAP };
        }
        let mut left = f32::from(p.quote) * QUOTE_INDENT;
        if p.kind.is_list() {
            left += f32::from(p.level + 1) * LIST_INDENT;
        }
        let avail = (width - left).max(40.0);
        let font = FontId::new(look.size_of(&p.kind), look.family.clone());
        let row_height = ui.fonts_mut(|f| f.row_height(&font));
        let block = match &p.kind {
            Kind::Image(id) => {
                let (w, h) = doc.image(id).map_or((64.0, 64.0), |im| (im.width as f32, im.height as f32));
                let scale = (avail / w.max(1.0)).min(MAX_IMAGE_HEIGHT / h.max(1.0)).min(1.0);
                let size = vec2((w * scale).max(8.0), (h * scale).max(8.0));
                Block { top: y, height: size.y, left, galley: None, bold_overlay: None, image: Some(size), row_height, len: 1 }
            }
            _ => {
                let (galley, overlay) = layout_paragraph(ui, p, avail, look);
                let height = galley.size().y.max(row_height);
                Block { top: y, height, left, galley: Some(galley), bold_overlay: overlay, image: None, row_height, len: p.len() }
            }
        };
        y += block.height;
        blocks.push(block);
    }
    Layout { blocks, height: y }
}

fn layout_paragraph(ui: &Ui, p: &Paragraph, width: f32, look: &Look) -> (Arc<Galley>, Option<Arc<Galley>>) {
    let job = |overlay: bool| {
        let mut job = LayoutJob::default();
        job.wrap.max_width = width;
        job.keep_trailing_whitespace = true;
        if p.runs.is_empty() {
            job.append("", 0.0, look.format(&Format::default(), &p.kind, overlay));
        }
        for run in &p.runs {
            job.append(&run.text, 0.0, look.format(&run.format, &p.kind, overlay));
        }
        job
    };
    let galley = ui.fonts_mut(|f| f.layout_job(job(false)));
    let any_bold = matches!(p.kind, Kind::Heading(_)) || p.runs.iter().any(|r| r.format.bold);
    let overlay = (look.bold_family.is_none() && any_bold).then(|| ui.fonts_mut(|f| f.layout_job(job(true))));
    (galley, overlay)
}

impl Layout {
    /// The position at `point` (relative to the widget's top left).
    fn pos_at(&self, point: Vec2) -> Pos {
        let index = self
            .blocks
            .iter()
            .position(|b| point.y < b.top + b.height + PARA_GAP * 0.5)
            .unwrap_or(self.blocks.len().saturating_sub(1));
        let Some(block) = self.blocks.get(index) else { return Pos::default() };
        if let Some(size) = block.image {
            return Pos::new(index, usize::from(point.x >= block.left + size.x * 0.5));
        }
        let Some(g) = &block.galley else { return Pos::new(index, 0) };
        let c = g.cursor_from_pos(vec2(point.x - block.left, point.y - block.top));
        Pos::new(index, c.index.0.min(block.len))
    }

    /// The caret's rectangle at `pos` (relative to the widget's top left).
    fn caret_rect(&self, pos: Pos) -> Rect {
        let Some(block) = self.blocks.get(pos.para) else { return Rect::from_min_size(Pos2::ZERO, vec2(0.0, 16.0)) };
        if let Some(size) = block.image {
            let x = block.left + if pos.offset == 0 { 0.0 } else { size.x };
            return Rect::from_min_max(pos2(x, block.top), pos2(x, block.top + size.y));
        }
        let Some(g) = &block.galley else { return Rect::from_min_size(pos2(block.left, block.top), vec2(0.0, block.row_height)) };
        let offset = pos.offset.min(galley_len(g));
        let r = g.pos_from_cursor(CCursor::new(offset));
        let r = if r.height() < 1.0 { Rect::from_min_size(r.min, vec2(0.0, block.row_height)) } else { r };
        r.translate(vec2(block.left, block.top))
    }

    /// The position a row up or down from `pos`, keeping `x`.
    fn vertical(&self, pos: Pos, x: f32, down: bool) -> Pos {
        let Some(block) = self.blocks.get(pos.para) else { return pos };
        if let Some(g) = &block.galley {
            let offset = pos.offset.min(galley_len(g));
            let row = g.layout_from_cursor(CCursor::new(offset)).row;
            let within = if down { row + 1 < g.rows.len() } else { row > 0 };
            if within {
                let (c, _) = if down { g.cursor_down_one_row(&CCursor::new(offset), Some(x - block.left)) } else { g.cursor_up_one_row(&CCursor::new(offset), Some(x - block.left)) };
                return Pos::new(pos.para, c.index.0.min(block.len));
            }
        }
        let target = if down { pos.para + 1 } else { pos.para.wrapping_sub(1) };
        let Some(next) = self.blocks.get(target) else {
            return if down { Pos::new(pos.para, block.len) } else { Pos::new(pos.para, 0) };
        };
        if let Some(size) = next.image {
            return Pos::new(target, usize::from(x >= next.left + size.x * 0.5));
        }
        let Some(g) = &next.galley else { return Pos::new(target, 0) };
        let row = if down { g.rows.first() } else { g.rows.last() };
        let y = row.map_or(0.0, |r| r.rect().center().y);
        let c = g.cursor_from_pos(vec2(x - next.left, y));
        Pos::new(target, c.index.0.min(next.len))
    }

    /// The start or end of the row `pos` is on.
    fn row_edge(&self, pos: Pos, end: bool) -> Pos {
        let Some(block) = self.blocks.get(pos.para) else { return pos };
        if block.image.is_some() {
            return Pos::new(pos.para, usize::from(end));
        }
        let Some(g) = &block.galley else { return pos };
        let offset = pos.offset.min(galley_len(g));
        let row = g.layout_from_cursor(CCursor::new(offset)).row;
        let start: usize = g.rows.iter().take(row).map(|r| r.char_count_including_newline().0).sum();
        let at = if end { start + g.rows.get(row).map_or(0, |r| r.char_count_excluding_newline().0) } else { start };
        Pos::new(pos.para, at.min(block.len))
    }
}

/// The rich-text editor widget. The [`Editor`] holds the document and the selection; put the
/// widget in a `ScrollArea` for a long message (the caret is kept in view).
pub struct RichEditor<'a> {
    editor: &'a mut Editor,
    id: Option<Id>,
    min_height: f32,
    font_size: Option<f32>,
    bold_family: Option<FontFamily>,
    hint: Option<String>,
}

impl<'a> RichEditor<'a> {
    pub fn new(editor: &'a mut Editor) -> Self {
        RichEditor { editor, id: None, min_height: 120.0, font_size: None, bold_family: None, hint: None }
    }

    /// The widget's id (one per compose window).
    pub fn id(mut self, id: Id) -> Self {
        self.id = Some(id);
        self
    }

    pub fn min_height(mut self, height: f32) -> Self {
        self.min_height = height;
        self
    }

    /// The text size (the body style's by default).
    pub fn font_size(mut self, size: f32) -> Self {
        self.font_size = Some(size);
        self
    }

    /// A bold font family the application registered; without one, bold is drawn twice, a little
    /// apart.
    pub fn bold_family(mut self, family: FontFamily) -> Self {
        self.bold_family = Some(family);
        self
    }

    /// Shown, faint, while the message is empty.
    pub fn hint_text(mut self, text: impl Into<String>) -> Self {
        self.hint = Some(text.into());
        self
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        let RichEditor { editor, id, min_height, font_size, bold_family, hint } = self;
        let id = id.unwrap_or_else(|| ui.make_persistent_id("mailalm-rich-editor"));
        editor.view.id = Some(id);
        let visuals = ui.visuals().clone();
        let body = egui::TextStyle::Body.resolve(ui.style());
        let look = Look {
            size: font_size.unwrap_or(body.size),
            family: body.family.clone(),
            bold_family,
            text: visuals.text_color(),
            strong: visuals.strong_text_color(),
            weak: visuals.weak_text_color(),
            link: visuals.hyperlink_color,
            dark: visuals.dark_mode,
        };
        let width = ui.available_width().max(80.0);
        let mut lay = layout(ui, editor.document(), width, &look);
        let size = vec2(width, lay.height.max(min_height));
        let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
        let mut response = ui.interact(rect, id, Sense::click_and_drag());
        let origin = rect.min;
        let now = ui.input(|i| i.time);
        let revision = editor.revision();
        let selection = editor.selection();

        // The mouse.
        if response.hovered() {
            ui.set_cursor_icon(CursorIcon::Text);
        }
        if let Some(pointer) = response.interact_pointer_pos() {
            let pos = lay.pos_at(pointer - origin);
            let shift = ui.input(|i| i.modifiers.shift);
            if response.double_clicked() && !shift {
                editor.select_word_at(pos);
            } else if response.triple_clicked() && !shift {
                editor.select_paragraph(pos);
            } else if response.hovered() && ui.input(|i| i.pointer.any_pressed()) {
                editor.move_to(pos, shift);
                ui.memory_mut(|m| {
                    m.request_focus(id);
                    m.interrupt_ime();
                });
                editor.view.preedit.clear();
            } else if ui.is_being_dragged(id) {
                editor.move_to(pos, true);
            }
            editor.view.h_pos = None;
            editor.view.last_interaction = now;
        }

        // The keyboard and the input method.
        let focused = ui.memory(|m| m.has_focus(id));
        if focused {
            let filter = EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: false };
            ui.memory_mut(|m| m.set_focus_lock_filter(id, filter));
            let events = ui.input(|i| i.filtered_events(&filter));
            let mut stale = false;
            for event in events {
                if stale {
                    lay = layout(ui, editor.document(), width, &look);
                    stale = false;
                }
                let before = editor.revision();
                handle_event(ui, editor, &lay, event);
                if editor.revision() != before {
                    stale = true;
                }
            }
            if stale {
                lay = layout(ui, editor.document(), width, &look);
            }
        }

        let changed = editor.revision() != revision;
        if changed {
            response.mark_changed();
        }
        let moved = editor.selection() != selection;
        if changed || moved {
            editor.view.last_interaction = now;
        }
        let caret = lay.caret_rect(editor.selection().head).translate(origin.to_vec2());
        if focused && (changed || moved) {
            ui.scroll_to_rect(caret.expand(4.0), None);
        }
        if (changed || moved) && lay.height.max(min_height) != size.y {
            ui.ctx().request_repaint();
        }

        let now_selection = editor.selection();
        let (doc, view) = editor.parts();
        paint(ui, doc, now_selection, &mut view.textures, &lay, &look, rect, hint.as_deref());
        if focused {
            if ui.input(|i| i.focused) {
                paint_text_cursor(ui, ui.painter(), caret, now - editor.view.last_interaction);
            }
            paint_preedit(ui, &editor.view.preedit, caret, &look);
            if !editor.view.was_focused || editor.view.ime_generation == 0 {
                editor.view.ime_generation = next_ime_generation();
            }
            let to_global = ui.ctx().layer_transform_to_global(ui.layer_id()).unwrap_or_default();
            let generation = editor.view.ime_generation;
            ui.output_mut(|o| {
                o.ime = Some(IMEOutput {
                    owner: id,
                    context_generation: generation,
                    purpose: IMEPurpose::Normal,
                    rect: to_global * rect,
                    cursor_rect: to_global * caret,
                    should_interrupt_composition: false,
                    reset_request: None,
                    change_cause: ImeChangeCause::Other,
                    surrounding_text: IMESurroundingText::Unavailable,
                });
            });
        } else {
            editor.view.preedit.clear();
        }
        editor.view.was_focused = focused;
        response
    }
}

/// One input event.
fn handle_event(ui: &Ui, editor: &mut Editor, lay: &Layout, event: Event) {
    let vertical = matches!(event, Event::Key { key: Key::ArrowUp | Key::ArrowDown | Key::PageUp | Key::PageDown, pressed: true, .. });
    if !vertical && matches!(event, Event::Key { pressed: true, .. } | Event::Text(_) | Event::Paste(_)) {
        editor.view.h_pos = None;
    }
    match event {
        Event::Text(text) => {
            if !text.is_empty() && text != "\n" && text != "\r" {
                editor.insert_text(&text);
            }
        }
        Event::Paste(text) => editor.paste_text(&text),
        Event::Copy => {
            if let Some(text) = editor.copy() {
                ui.ctx().copy_text(text);
            }
        }
        Event::Cut => {
            if let Some(text) = editor.cut() {
                ui.ctx().copy_text(text);
            }
        }
        Event::Ime(ime) => match ime {
            ImeEvent::Preedit { text, .. } => editor.view.preedit = text,
            ImeEvent::Commit(text) => {
                editor.view.preedit.clear();
                if !text.is_empty() && text != "\n" && text != "\r" {
                    editor.insert_text(&text);
                }
            }
            _ => {}
        },
        Event::Key { key, physical_key, pressed: true, modifiers, .. } => key_pressed(editor, lay, key, physical_key, modifiers),
        _ => {}
    }
}

fn key_pressed(editor: &mut Editor, lay: &Layout, key: Key, physical_key: Option<Key>, modifiers: Modifiers) {
    // Ctrl, but not AltGr (Ctrl+Alt on Windows), which types characters.
    let command = modifiers.command && !modifiers.alt;
    let shift = modifiers.shift;
    match key {
        Key::Enter => editor.enter(),
        Key::Backspace if command => editor.delete_word_backward(),
        Key::Backspace => editor.backspace(),
        Key::Delete if command => editor.delete_word_forward(),
        Key::Delete => editor.delete(),
        Key::ArrowLeft if command => editor.move_word_left(shift),
        Key::ArrowLeft => editor.move_left(shift),
        Key::ArrowRight if command => editor.move_word_right(shift),
        Key::ArrowRight => editor.move_right(shift),
        Key::ArrowUp | Key::ArrowDown | Key::PageUp | Key::PageDown => {
            let down = matches!(key, Key::ArrowDown | Key::PageDown);
            let steps = if matches!(key, Key::PageUp | Key::PageDown) { 15 } else { 1 };
            let mut pos = editor.selection().head;
            let x = editor.view.h_pos.unwrap_or_else(|| lay.caret_rect(pos).center().x);
            for _ in 0..steps {
                pos = lay.vertical(pos, x, down);
            }
            editor.move_to(pos, shift);
            editor.view.h_pos = Some(x);
        }
        Key::Home if command => editor.move_to_start(shift),
        Key::Home => {
            let pos = lay.row_edge(editor.selection().head, false);
            editor.move_to(pos, shift);
        }
        Key::End if command => editor.move_to_end(shift),
        Key::End => {
            let pos = lay.row_edge(editor.selection().head, true);
            editor.move_to(pos, shift);
        }
        Key::Tab => {
            if shift {
                editor.outdent();
            } else if !editor.indent() {
                editor.insert_text("\t");
            }
        }
        Key::A if command => editor.select_all(),
        Key::B if command => editor.toggle_mark(Mark::Bold),
        Key::I if command => editor.toggle_mark(Mark::Italic),
        Key::U if command => editor.toggle_mark(Mark::Underline),
        Key::K if command => editor.request_link(),
        Key::Z if command && shift => {
            editor.redo();
        }
        Key::Z if command => {
            editor.undo();
        }
        Key::Y if command => {
            editor.redo();
        }
        _ => {
            // Ctrl+Shift+7 and 8 by the key's place (on a French keyboard, Shift gives the digit).
            match physical_key.unwrap_or(key) {
                Key::Num7 if command && shift => editor.toggle_list(ListKind::Numbers),
                Key::Num8 if command && shift => editor.toggle_list(ListKind::Bullets),
                _ => {}
            }
        }
    }
}

/// The bullet of a list level.
fn bullet(level: u8) -> &'static str {
    match level % 3 {
        0 => "•",
        1 => "◦",
        _ => "▪",
    }
}

#[allow(clippy::too_many_arguments)]
fn paint(
    ui: &Ui,
    doc: &Document,
    selection: Selection,
    textures: &mut HashMap<String, Option<TextureHandle>>,
    lay: &Layout,
    look: &Look,
    rect: Rect,
    hint: Option<&str>,
) {
    let painter = ui.painter();
    let visuals = ui.visuals();
    let origin = rect.min;
    let clip = ui.clip_rect();
    let numbers = doc.list_numbers();
    let (sel_start, sel_end) = selection.range();
    let has_selection = sel_start != sel_end;
    let quote_color = visuals.weak_text_color().gamma_multiply(0.6);
    if doc.is_empty()
        && let Some(hint) = hint
    {
        painter.text(origin, Align2::LEFT_TOP, hint, FontId::new(look.size, look.family.clone()), look.weak);
    }
    for (i, (p, block)) in doc.paragraphs.iter().zip(&lay.blocks).enumerate() {
        let top = origin.y + block.top;
        if top > clip.max.y || top + block.height < clip.min.y {
            continue;
        }
        for d in 0..p.quote {
            let x = origin.x + f32::from(d) * QUOTE_INDENT + 3.0;
            painter.line_segment([pos2(x, top - PARA_GAP), pos2(x, top + block.height + PARA_GAP)], Stroke::new(2.0, quote_color));
        }
        let selected = (has_selection && sel_start.para <= i && i <= sel_end.para).then(|| {
            let from = if i == sel_start.para { sel_start.offset } else { 0 };
            let to = if i == sel_end.para { sel_end.offset } else { block.len };
            (from, to)
        });
        if p.kind.is_list() {
            let marker = match numbers.get(i).copied().flatten() {
                Some(n) => format!("{n}."),
                None => bullet(p.level).to_owned(),
            };
            painter.text(pos2(origin.x + block.left - 6.0, top), Align2::RIGHT_TOP, marker, FontId::new(look.size, look.family.clone()), look.text);
        }
        let at = pos2(origin.x + block.left, top);
        if let (Some(size), Kind::Image(id)) = (block.image, &p.kind) {
            let image_rect = Rect::from_min_size(at, size);
            match doc.image(id).and_then(|image| texture(ui, textures, image)) {
                Some(texture) => {
                    painter.image(texture.id(), image_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                }
                None => {
                    painter.rect_filled(image_rect, 2.0, visuals.extreme_bg_color);
                    painter.text(image_rect.center(), Align2::CENTER_CENTER, "Picture", FontId::new(look.size, look.family.clone()), look.weak);
                }
            }
            if selected.is_some_and(|(a, b)| a < b) {
                painter.rect_filled(image_rect, 0.0, visuals.selection.bg_fill.gamma_multiply(0.5));
            }
            continue;
        }
        let Some(galley) = &block.galley else { continue };
        let mut galley = Arc::clone(galley);
        if let Some((a, b)) = selected {
            let len = galley_len(&galley);
            let (a, b) = (a.min(len), b.min(len));
            if a < b {
                paint_text_selection(&mut galley, visuals, &CCursorRange::two(CCursor::new(a), CCursor::new(b)), None);
            }
            if i < sel_end.para {
                // The paragraph's end is selected too.
                let end = lay.caret_rect(Pos::new(i, block.len)).translate(origin.to_vec2());
                painter.rect_filled(Rect::from_min_size(end.min, vec2(5.0, end.height())), 0.0, visuals.selection.bg_fill);
            }
        }
        if let Some(overlay) = &block.bold_overlay {
            painter.galley(at + vec2(FAUX_BOLD_OFFSET, 0.0), Arc::clone(overlay), look.strong);
        }
        painter.galley(at, galley, look.text);
    }
}

/// The texture of a picture, decoded once.
fn texture(ui: &Ui, textures: &mut HashMap<String, Option<TextureHandle>>, image: &picture::InlineImage) -> Option<TextureHandle> {
    if let Some(cached) = textures.get(&image.content_id) {
        return cached.clone();
    }
    let handle = picture::decode(image, MAX_TEXTURE_SIDE).map(|(size, rgba)| {
        ui.ctx().load_texture(format!("mailalm-compose-{}", image.content_id), ColorImage::from_rgba_unmultiplied(size, &rgba), TextureOptions::LINEAR)
    });
    textures.insert(image.content_id.clone(), handle.clone());
    handle
}

/// The input method's composition, drawn at the caret, underlined.
fn paint_preedit(ui: &Ui, preedit: &str, caret: Rect, look: &Look) {
    if preedit.is_empty() {
        return;
    }
    let painter = ui.painter();
    let galley = painter.layout_no_wrap(preedit.to_owned(), FontId::new(look.size, look.family.clone()), look.text);
    let r = Rect::from_min_size(caret.min, galley.size());
    painter.rect_filled(r.expand(1.0), 2.0, ui.visuals().extreme_bg_color);
    painter.galley(r.min, galley, look.text);
    painter.line_segment([r.left_bottom(), r.right_bottom()], Stroke::new(1.0, look.text));
}

/// What [`toolbar`] keeps between frames.
#[derive(Clone, Debug, Default)]
pub struct ToolbarState {
    /// The link being typed, when the link prompt is open.
    link: Option<String>,
    error: Option<String>,
}

/// What the toolbar asks the window to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolbarRequest {
    /// Pick a picture file and give it to [`Editor::insert_image`].
    InsertImage,
}

/// The formatting toolbar: bold, italic, underline, strike, lists, quote, headings, link, colour,
/// picture, clear formatting, undo and redo. Ctrl+K in the editor opens its link prompt.
pub fn toolbar(ui: &mut Ui, editor: &mut Editor, state: &mut ToolbarState) -> Option<ToolbarRequest> {
    let mut request = None;
    let mut acted = false;
    if editor.take_link_request() {
        state.link = Some(editor.link_at_caret().map(|l| l.as_str().to_owned()).unwrap_or_default());
        state.error = None;
    }
    let format = editor.current_format();
    let kind = editor.current_kind();
    let quoted = editor.document().paragraph(editor.selection().head.para).is_some_and(|p| p.quote > 0);
    ui.horizontal_wrapped(|ui| {
        let marks = [
            (Mark::Bold, RichText::new("B").strong(), "Bold (Ctrl+B)"),
            (Mark::Italic, RichText::new("I").italics(), "Italic (Ctrl+I)"),
            (Mark::Underline, RichText::new("U").underline(), "Underline (Ctrl+U)"),
            (Mark::Strike, RichText::new("S").strikethrough(), "Strikethrough"),
        ];
        for (mark, label, tip) in marks {
            if ui.add(Button::selectable(format.has(mark), label)).on_hover_text(tip).clicked() {
                editor.toggle_mark(mark);
                acted = true;
            }
        }
        ui.separator();
        if ui.add(Button::selectable(kind == Kind::Bullet, "• List")).on_hover_text("Bulleted list (Ctrl+Shift+8)").clicked() {
            editor.toggle_list(ListKind::Bullets);
            acted = true;
        }
        if ui.add(Button::selectable(kind == Kind::Numbered, "1. List")).on_hover_text("Numbered list (Ctrl+Shift+7)").clicked() {
            editor.toggle_list(ListKind::Numbers);
            acted = true;
        }
        if ui.add(Button::selectable(quoted, "Quote")).clicked() {
            editor.toggle_quote();
            acted = true;
        }
        let _ = ui.menu_button("Heading", |ui| {
            for level in 1..=3u8 {
                if ui.add(Button::selectable(kind == Kind::Heading(level), format!("Heading {level}"))).clicked() {
                    editor.toggle_heading(level);
                    acted = true;
                    ui.close();
                }
            }
            if ui.button("Normal text").clicked() {
                editor.set_normal();
                acted = true;
                ui.close();
            }
        });
        ui.separator();
        if ui.add(Button::selectable(format.link.is_some(), "Link…")).on_hover_text("Link (Ctrl+K)").clicked() {
            state.link = Some(editor.link_at_caret().map(|l| l.as_str().to_owned()).unwrap_or_default());
            state.error = None;
        }
        let _ = ui.menu_button("Colour", |ui| {
            if ui.button("Automatic").clicked() {
                editor.set_color(None);
                acted = true;
                ui.close();
            }
            for c in Color::ALL {
                let [r, g, b] = c.rgb();
                if ui.button(RichText::new(format!("■ {}", c.name())).color(Color32::from_rgb(r, g, b))).clicked() {
                    editor.set_color(Some(c));
                    acted = true;
                    ui.close();
                }
            }
        });
        if ui.button("Picture…").clicked() {
            request = Some(ToolbarRequest::InsertImage);
        }
        if ui.button("Clear formatting").clicked() {
            editor.clear_formatting();
            acted = true;
        }
        ui.separator();
        if ui.add_enabled(editor.can_undo(), Button::new("Undo")).on_hover_text("Undo (Ctrl+Z)").clicked() {
            editor.undo();
            acted = true;
        }
        if ui.add_enabled(editor.can_redo(), Button::new("Redo")).on_hover_text("Redo (Ctrl+Y)").clicked() {
            editor.redo();
            acted = true;
        }
    });

    let (mut apply, mut remove, mut close) = (false, false, false);
    if let Some(url) = state.link.as_mut() {
        ui.horizontal(|ui| {
            ui.label("Link:");
            let field = ui.add(TextEdit::singleline(url).hint_text("https://…").desired_width(300.0));
            if field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                apply = true;
            }
            if ui.button("Apply").clicked() {
                apply = true;
            }
            if editor.link_at_caret().is_some() && ui.button("Remove").clicked() {
                remove = true;
            }
            if ui.button("Cancel").clicked() {
                close = true;
            }
        });
        if let Some(error) = &state.error {
            ui.label(RichText::new(error).color(ui.visuals().error_fg_color));
        }
    }
    if apply {
        let typed = state.link.clone().unwrap_or_default();
        match Link::parse(&typed) {
            Some(link) => {
                if editor.selection().is_empty() && editor.link_at_caret().is_none() {
                    editor.insert_link(typed.trim(), link);
                } else {
                    editor.set_link(link);
                }
                close = true;
                acted = true;
            }
            None => state.error = Some("Only https, http and mailto links can be put in a message.".to_owned()),
        }
    }
    if remove {
        editor.remove_link();
        close = true;
        acted = true;
    }
    if close {
        state.link = None;
        state.error = None;
    }
    if acted && let Some(id) = editor.view.id {
        ui.memory_mut(|m| m.request_focus(id));
    }
    request
}
