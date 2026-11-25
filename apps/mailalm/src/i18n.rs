//! The interface's language (PLAN.md, section 12, decision Q11): English by default, French as an
//! option.
//!
//! Every text of the interface is written in English in the code, inside [`tr`] (a fixed text),
//! [`trf!`] (a text with values, named `{like_this}`) or [`tr_key`] (a text kept, or put in a
//! constant, and translated with [`tr`] when it is shown). The English text is the key of the
//! French table (`i18n/french.rs`); a text the table lacks shows in English. The language is a
//! global, set as MailALM starts and when the setting changes, so the texts follow it at the next
//! frame.

use std::collections::HashMap;
use std::fmt::{Display, Write as _};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

mod french;

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

/// The language shown now: 0 English, 1 French.
static LANGUAGE: AtomicU8 = AtomicU8::new(0);

/// Shows the interface in `language` from the next frame on.
pub fn set_language(language: Language) {
    LANGUAGE.store(
        match language {
            Language::English => 0,
            Language::French => 1,
        },
        Ordering::Relaxed,
    );
}

/// The language shown now.
pub fn language() -> Language {
    match LANGUAGE.load(Ordering::Relaxed) {
        1 => Language::French,
        _ => Language::English,
    }
}

/// The French table, by English text.
fn french_table() -> &'static HashMap<&'static str, &'static str> {
    static TABLE: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    TABLE.get_or_init(|| french::FRENCH.iter().copied().collect())
}

/// The French text of `english`, if the table has it.
fn french(english: &str) -> Option<&'static str> {
    french_table().get(english).copied()
}

/// A text of the interface, in the language shown: its translation, or the English text when the
/// table has none.
pub fn tr(english: &'static str) -> &'static str {
    match language() {
        Language::English => english,
        Language::French => french(english).unwrap_or(english),
    }
}

/// An English text kept as it is (in a constant, a list, a value stored), and translated with
/// [`tr`] where it is shown. It marks the text for the table's test.
pub const fn tr_key(english: &'static str) -> &'static str {
    english
}

/// `template` with each `{name}` replaced by its value; a `{name}` without a value stays as it
/// is. Used by [`trf!`].
pub fn fill(template: &str, values: &[(&str, &dyn Display)]) -> String {
    let mut out = String::with_capacity(template.len() + 32);
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let (before, from_open) = rest.split_at(open);
        out.push_str(before);
        let after = from_open.get(1..).unwrap_or("");
        let value = after.find('}').and_then(|close| {
            let name = after.get(..close)?;
            values.iter().find(|(n, _)| *n == name).map(|(_, v)| (*v, close))
        });
        match value {
            Some((v, close)) => {
                let _ = write!(out, "{v}");
                rest = after.get(close + 1..).unwrap_or("");
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// A text of the interface with values, in the language shown: `trf!("Delete {name}?", name =
/// account.name)`. The names are the same in every language; each value is shown with `Display`.
macro_rules! trf {
    ($english:literal $(, $name:ident = $value:expr)* $(,)?) => {
        $crate::i18n::fill($crate::i18n::tr($english), &[$((stringify!($name), &$value as &dyn ::std::fmt::Display)),*])
    };
}
pub(crate) use trf;

#[cfg(test)]
mod tests;
