//! A recipient field that offers addresses while typing (K1): every account's contacts first,
//! then the people seen in mail. A suggestion is taken by a click, or the first one by Enter; the
//! field stays plain text, read and checked when the message or the event is saved.

use egui::{Frame, Id, Key, Margin, Order, Rect, RichText, Stroke, TextEdit, Ui, vec2};
use mailalm_mime::format_person;
use mailalm_model::Person;

use crate::theme as t;

/// Where the part being typed starts (after the last comma or semicolon), and that part.
fn last_part(text: &str) -> (usize, &str) {
    let at = text.rfind([',', ';']).map_or(0, |i| i + 1);
    (at, text.get(at..).unwrap_or("").trim_start())
}

/// The field with the part being typed replaced by `p`, and a comma for the next one.
fn complete(text: &str, p: &Person) -> String {
    let (at, _) = last_part(text);
    let head = text.get(..at).unwrap_or("").trim_end();
    let sep = if head.is_empty() { "" } else { " " };
    format!("{head}{sep}{}, ", format_person(p))
}

/// A recipient field (`id` names it in its window) with the addresses `suggest` offers for what
/// is being typed, under it.
pub fn field(ui: &mut Ui, id: Id, text: &mut String, width: f32, hint: &str, suggest: &dyn Fn(&str) -> Vec<Person>) -> egui::Response {
    let response = ui.add(TextEdit::singleline(text).id(id).desired_width(width).hint_text(hint.to_owned()));
    let ctx = ui.ctx().clone();
    let rect_id = id.with("mailalm-suggestions-rect");
    // The list stays while the field has the focus, and while the pointer is on it (a click on
    // a suggestion takes the focus from the field first).
    let last: Option<Rect> = ctx.data(|d| d.get_temp(rect_id));
    let pointer_on_list = last.is_some_and(|r| ctx.input(|i| i.pointer.latest_pos().is_some_and(|p| r.contains(p))));
    let enter = response.lost_focus() && ctx.input(|i| i.key_pressed(Key::Enter));
    let (_, typed) = last_part(text);
    let list = if (response.has_focus() || response.lost_focus() || pointer_on_list) && !typed.is_empty() && !typed.contains('<') { suggest(typed) } else { Vec::new() };
    if list.is_empty() {
        ctx.data_mut(|d| d.remove::<Rect>(rect_id));
        return response;
    }
    let mut chosen: Option<usize> = if enter { Some(0) } else { None };
    let shown = egui::Area::new(id.with("mailalm-suggestions")).order(Order::Foreground).fixed_pos(response.rect.left_bottom() + vec2(0.0, 2.0)).show(&ctx, |ui| {
        Frame::new().fill(t::RAISED).stroke(Stroke::new(1.0, t::LINE)).corner_radius(6).inner_margin(Margin::same(4)).show(ui, |ui| {
            ui.set_width(response.rect.width().max(280.0));
            for (n, p) in list.iter().enumerate() {
                let label = if p.name.is_empty() { RichText::new(&p.address) } else { RichText::new(format!("{}  <{}>", p.name, p.address)) };
                if ui.add(egui::Button::new(label).frame_when_inactive(false)).clicked() {
                    chosen = Some(n);
                }
            }
        });
    });
    ctx.data_mut(|d| d.insert_temp(rect_id, shown.response.rect));
    if let Some(p) = chosen.and_then(|n| list.get(n)) {
        *text = complete(text, p);
        ctx.data_mut(|d| d.remove::<Rect>(rect_id));
        response.request_focus();
        // The cursor at the end, for the next address.
        if let Some(mut state) = egui::text_edit::TextEditState::load(&ctx, id) {
            let end = egui::text::CCursor::new(text.chars().count());
            state.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
            state.store(&ctx, id);
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_part_typed_is_replaced() {
        let zoe = Person { name: "Zoe Park".into(), address: "zoe@studio.example".into() };
        assert_eq!(last_part("a@b.example, zo"), (12, "zo"));
        assert_eq!(complete("a@b.example, zo", &zoe), "a@b.example, Zoe Park <zoe@studio.example>, ");
        assert_eq!(complete("zo", &zoe), "Zoe Park <zoe@studio.example>, ");
        assert_eq!(last_part("x; y"), (2, "y"));
    }
}
