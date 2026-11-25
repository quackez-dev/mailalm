//! What each account writes with (C4, PLAN.md section 8): its signature, its templates and its
//! quick replies, kept in the settings, and where a signature goes in a message. A signature and
//! a template are kept as the small HTML MailALM's editor writes (`mailalm_compose::to_html`),
//! read back through the editor's import like any HTML; a quick reply is plain text.

use mailalm_compose::{Document, import_html, plain_text};
use mailalm_model::AccountId;
use serde::{Deserialize, Serialize};

/// The longest signature, template or quick reply kept, in bytes.
pub const MAX_SAVED: usize = 256 << 10;

/// One account's signature, templates and quick replies.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Writing {
    pub account: AccountId,
    /// The signature's HTML; empty for none.
    #[serde(default)]
    pub signature: String,
    #[serde(default)]
    pub templates: Vec<Template>,
    #[serde(default)]
    pub quick_replies: Vec<QuickReply>,
}

impl Writing {
    pub fn new(account: AccountId) -> Writing {
        Writing { account, signature: String::new(), templates: Vec::new(), quick_replies: Vec::new() }
    }
}

/// A message to start from: its subject and its text (HTML).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Template {
    pub name: String,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub html: String,
}

/// A short text put in at the caret.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickReply {
    pub name: String,
    #[serde(default)]
    pub text: String,
}

/// The signature as the editor puts it in a message (where `mailalm_compose::signature_slot`
/// says); `None` for none.
pub fn signature_document(html: &str) -> Option<Document> {
    let doc = import_html(html, &[]);
    (!doc.is_empty()).then_some(doc)
}

/// The signature as plain text (`-- ` then its lines); `None` for none.
pub fn signature_text(html: &str) -> Option<String> {
    let doc = import_html(html, &[]);
    (!doc.is_empty()).then(|| format!("-- \n{}", plain_text(&doc)))
}

/// A plain-text body with its signature changed: `old` (the one put in before) replaced by
/// `new` where it is; else `new` put above a reply's or a forward's quoted text, or at the end.
pub fn place_plain(body: &str, old: Option<&str>, new: Option<&str>) -> String {
    if let Some(old) = old.filter(|o| !o.is_empty())
        && let Some(at) = body.find(old)
    {
        let before = body.get(..at).unwrap_or("");
        let after = body.get(at + old.len()..).unwrap_or("");
        return match new {
            Some(new) => format!("{before}{new}{after}"),
            None => format!("{}{after}", before.trim_end_matches('\n')),
        };
    }
    let Some(new) = new.filter(|n| !n.is_empty()) else { return body.to_owned() };
    let lines: Vec<&str> = body.split('\n').collect();
    match plain_slot(&lines) {
        Some(i) => {
            let head = lines.get(..i).unwrap_or_default().join("\n");
            let tail = lines.get(i..).unwrap_or_default().join("\n");
            format!("{head}\n\n{new}\n\n{}", tail.trim_start_matches('\n'))
        }
        None => format!("{}\n\n{new}", body.trim_end_matches('\n')),
    }
}

/// The line a new signature goes before in a plain-text body: the quoted text of a reply (its
/// attribution line, and the empty lines over it, the first line staying above) or a forward's
/// header; `None` when there is none.
fn plain_slot(lines: &[&str]) -> Option<usize> {
    let first = lines.iter().position(|l| l.starts_with('>') || l.trim() == "---------- Forwarded message ----------")?;
    let mut at = first;
    if at > 0 && lines.get(at - 1).is_some_and(|l| !l.trim().is_empty() && !l.starts_with('>') && l.trim_end().ends_with(':')) {
        at -= 1;
    }
    while at > 1 && lines.get(at - 1).is_some_and(|l| l.trim().is_empty()) {
        at -= 1;
    }
    Some(at)
}

#[cfg(test)]
mod tests {
    use mailalm_compose::Paragraph;

    use super::*;

    #[test]
    fn a_signature_as_the_editor_puts_it() {
        let doc = signature_document("<p>Alex <b>Morin</b></p><p>Videotron</p>").expect("a signature");
        let texts: Vec<String> = doc.paragraphs.iter().map(Paragraph::text).collect();
        assert_eq!(texts, ["Alex Morin", "Videotron"]);
        assert!(signature_document("  <p> </p>").is_none());
        assert_eq!(signature_text("<p>Alex</p>").as_deref(), Some("-- \nAlex"));
        assert_eq!(signature_text(""), None);
    }

    #[test]
    fn a_plain_signature_goes_above_the_quote() {
        let sig = "-- \nAlex";
        // A new message.
        assert_eq!(place_plain("", None, Some(sig)), "\n\n-- \nAlex");
        // A reply (as mailalm_mime::reply writes it).
        let reply = "\n\nOn Mon, Zoe wrote:\n> Hi\n> there";
        let with = place_plain(reply, None, Some(sig));
        assert_eq!(with, "\n\n-- \nAlex\n\nOn Mon, Zoe wrote:\n> Hi\n> there");
        // Another account's: in its place; none: taken out.
        assert_eq!(place_plain(&with, Some(sig), Some("-- \nBob")), "\n\n-- \nBob\n\nOn Mon, Zoe wrote:\n> Hi\n> there");
        assert_eq!(place_plain(&with, Some(sig), None), "\n\nOn Mon, Zoe wrote:\n> Hi\n> there");
        // A forward.
        let forward = "\n\n---------- Forwarded message ----------\nFrom: Zoe";
        assert_eq!(place_plain(forward, None, Some(sig)), "\n\n-- \nAlex\n\n---------- Forwarded message ----------\nFrom: Zoe");
        // What was typed stays above.
        assert_eq!(place_plain("Hello\n\nOn Mon, Zoe wrote:\n> Hi", None, Some(sig)), "Hello\n\n-- \nAlex\n\nOn Mon, Zoe wrote:\n> Hi");
        // The old one edited away by the user: the new one goes in as new.
        assert_eq!(place_plain("Hi", Some(sig), Some("-- \nBob")), "Hi\n\n-- \nBob");
        assert_eq!(place_plain("Hi", None, None), "Hi");
    }

    #[test]
    fn writing_settings_read_back() {
        let w = Writing {
            account: AccountId(3),
            signature: "<p>Alex</p>".into(),
            templates: vec![Template { name: "Invoice".into(), subject: "Your invoice".into(), html: "<p>Here it is.</p>".into() }],
            quick_replies: vec![QuickReply { name: "Thanks".into(), text: "Thank you!".into() }],
        };
        let json = serde_json::to_string(&w).expect("written");
        assert_eq!(serde_json::from_str::<Writing>(&json).expect("read"), w);
        assert_eq!(serde_json::from_str::<Writing>("{\"account\":4}").expect("read"), Writing::new(AccountId(4)));
    }
}
