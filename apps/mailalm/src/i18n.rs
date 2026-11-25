//! The interface's language (PLAN.md, section 12, decision Q11): English by default, French as an
//! option. For now only the setting and the lookup exist: the French table is filled in a later
//! pass, and a text it lacks shows in English.

use serde::{Deserialize, Serialize};

/// A language the interface can be shown in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    #[default]
    English,
    French,
}

impl Language {
    pub const ALL: [Language; 2] = [Language::English, Language::French];

    /// Its name, in itself.
    pub fn label(self) -> &'static str {
        match self {
            Language::English => "English",
            Language::French => "Français",
        }
    }
}

/// The French texts, by their English text. Empty until the translation pass.
const FRENCH: &[(&str, &str)] = &[];

/// A text of the interface in `language`: its translation, or the English text when the table has
/// none.
pub fn tr(language: Language, english: &'static str) -> &'static str {
    match language {
        Language::English => english,
        Language::French => FRENCH.iter().find(|(en, _)| *en == english).map_or(english, |(_, fr)| fr),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_translation_shows_in_english() {
        assert_eq!(tr(Language::English, "Settings"), "Settings");
        assert_eq!(tr(Language::French, "Settings"), "Settings");
        assert_eq!(Language::default(), Language::English);
        let json = serde_json::to_string(&Language::French).unwrap_or_default();
        assert_eq!(serde_json::from_str::<Language>(&json).ok(), Some(Language::French));
    }
}
