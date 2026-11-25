//! Settings → Appearance and language: the theme (dark, light, or as Windows is set) and the
//! interface's language (English; French is the setting and its lookup for now, its texts come
//! with the translation pass).

use egui::{Frame, Margin, RichText, Stroke, Ui};

use super::MailApp;
use crate::i18n::{Language, tr};
use crate::theme::{self as t, ThemeChoice};

impl MailApp {
    pub(super) fn appearance_settings(&mut self, ui: &mut Ui) {
        let lang = self.settings.language;
        ui.add_space(20.0);
        ui.label(RichText::new(tr(lang, "APPEARANCE AND LANGUAGE")).size(11.0).strong().color(t::faint()));
        ui.add_space(6.0);
        let mut theme: Option<ThemeChoice> = None;
        let mut language: Option<Language> = None;
        Frame::new().fill(t::panel2()).stroke(Stroke::new(1.0, t::line())).corner_radius(8).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
            ui.set_width(ui.available_width().min(760.0));
            ui.horizontal(|ui| {
                ui.label(tr(lang, "Theme"));
                for choice in ThemeChoice::ALL {
                    if ui.radio(self.settings.theme == choice, tr(lang, choice.label())).clicked() && self.settings.theme != choice {
                        theme = Some(choice);
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label(tr(lang, "Language"));
                for choice in Language::ALL {
                    if ui.radio(self.settings.language == choice, choice.label()).clicked() && self.settings.language != choice {
                        language = Some(choice);
                    }
                }
            });
            if self.settings.language == Language::French {
                ui.label(RichText::new("The French texts come with the next version: MailALM shows English until then.").size(11.5).color(t::faint()));
            }
        });
        let changed = theme.is_some() || language.is_some();
        if let Some(choice) = theme {
            self.settings.theme = choice;
            t::apply(ui.ctx(), choice);
        }
        if let Some(choice) = language {
            self.settings.language = choice;
        }
        if changed && let Err(e) = self.settings.save() {
            self.warn(e);
        }
    }
}
