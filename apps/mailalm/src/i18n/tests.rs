use std::collections::{BTreeSet, HashSet};
use std::fmt::Display;
use std::path::{Path, PathBuf};

use super::*;

/// The `.rs` files under `dir`.
fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir).expect("source folder").map(|e| e.expect("entry").path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// A string literal's text, from just after its opening quote: its value and where it ends.
fn literal(src: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut chars = src.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Ok(out),
            '\\' => match chars.next() {
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('\'') => out.push('\''),
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('u') => {
                    let rest: String = chars.by_ref().take_while(|c| *c != '}').collect();
                    let hex = rest.strip_prefix('{').ok_or("bad \\u escape")?;
                    let code = u32::from_str_radix(hex, 16).map_err(|e| e.to_string())?;
                    out.push(char::from_u32(code).ok_or("bad \\u value")?);
                }
                other => return Err(format!("escape {other:?} is not handled: write the key without it")),
            },
            c => out.push(c),
        }
    }
    Err("unterminated literal".to_owned())
}

/// Every key the code uses: the literal in `tr("…")`, `trf!("…"` and `tr_key("…")`, with the
/// file it is in.
fn keys_in_code() -> Vec<(String, String)> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    sources(&manifest.join("src"), &mut files);
    // The rich-text editor's toolbar uses this table through `mailalm_compose::set_translator`.
    files.push(manifest.join("../../crates/compose/src/widget.rs"));
    let mut keys = Vec::new();
    for file in files {
        // This module's own texts are examples, not keys.
        if file.components().any(|c| c.as_os_str() == "i18n") || file.file_name().is_some_and(|n| n == "i18n.rs") {
            continue;
        }
        let src = std::fs::read_to_string(&file).expect("source file");
        for call in ["tr(\"", "trf!(\"", "tr_key(\""] {
            let mut from = 0;
            while let Some(at) = src[from..].find(call) {
                let start = from + at;
                from = start + call.len();
                let before = src[..start].chars().next_back();
                if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                    continue;
                }
                let key = literal(&src[from..]).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
                keys.push((key, file.display().to_string()));
            }
        }
    }
    keys
}

/// The `{names}` of a text.
fn placeholders(text: &str) -> BTreeSet<&str> {
    let mut names = BTreeSet::new();
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) if after[..close].chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && close > 0 => {
                names.insert(&after[..close]);
                rest = &after[close + 1..];
            }
            _ => rest = after,
        }
    }
    names
}

#[test]
fn every_key_of_the_code_has_its_french() {
    let keys = keys_in_code();
    assert!(keys.len() > 500, "the scan found only {} keys", keys.len());
    let missing: Vec<String> = keys.iter().filter(|(k, _)| french(k).is_none()).map(|(k, f)| format!("{f}: {k:?}")).collect();
    assert!(missing.is_empty(), "keys without French:\n{}", missing.join("\n"));
}

#[test]
fn the_table_has_each_key_once_and_only_keys_the_code_uses() {
    let mut seen = HashSet::new();
    let doubles: Vec<&str> = table_keys().filter(|k| !seen.insert(*k)).collect();
    assert!(doubles.is_empty(), "keys twice in the table: {doubles:?}");
    let used: HashSet<String> = keys_in_code().into_iter().map(|(k, _)| k).collect();
    let unused: Vec<&str> = table_keys().filter(|k| !used.contains(*k)).collect();
    assert!(unused.is_empty(), "keys the code no longer uses: {unused:?}");
}

fn table_keys() -> impl Iterator<Item = &'static str> {
    french::FRENCH.iter().map(|(en, _)| *en)
}

#[test]
fn the_french_has_the_same_placeholders_and_outer_spaces() {
    for (en, fr) in french::FRENCH {
        assert_eq!(placeholders(en), placeholders(fr), "placeholders of {en:?} → {fr:?}");
        assert!(!fr.trim().is_empty(), "empty French for {en:?}");
        assert_eq!(en.starts_with(' '), fr.starts_with(' '), "leading space of {en:?} → {fr:?}");
        assert_eq!(en.ends_with(' '), fr.ends_with(' '), "trailing space of {en:?} → {fr:?}");
        // A space before : ; ? ! is a no-break one (a time like 14:30 or a URL has none).
        for mark in [" :", " ;", " ?", " !"] {
            assert!(!fr.contains(mark), "ordinary space before {mark:?} in {fr:?}: write \\u{{a0}}");
        }
    }
}

#[test]
fn the_language_chooses_the_text() {
    // The global stays English in tests (other tests compare English texts): the French is read
    // through the table directly.
    assert_eq!(language(), Language::English);
    assert_eq!(tr("Settings"), "Settings");
    assert_eq!(french("Settings"), Some("Paramètres"));
    assert_eq!(french("No such text in MailALM"), None);
    assert_eq!(Language::default(), Language::English);
    let json = serde_json::to_string(&Language::French).unwrap_or_default();
    assert_eq!(serde_json::from_str::<Language>(&json).ok(), Some(Language::French));
}

#[test]
fn values_fill_their_placeholders() {
    let name = String::from("Zoé {count}");
    assert_eq!(fill("Delete {name}?", &[("name", &name as &dyn Display)]), "Delete Zoé {count}?", "a value is not filled again");
    assert_eq!(fill("{count} of {total}", &[("total", &7 as &dyn Display), ("count", &3 as &dyn Display)]), "3 of 7");
    assert_eq!(fill("{missing} and {", &[]), "{missing} and {");
    assert_eq!(fill("{count}", &[("count", &1 as &dyn Display)]), "1");
    assert_eq!(trf!("{done} of {total} messages", done = 2, total = 5), "2 of 5 messages");
}
