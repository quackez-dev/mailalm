//! Antidote in the compose window (PLAN.md section 9): the Corrector, Dictionaries and Guides
//! buttons, shown only when Antidote's connector is installed, and Anti-Oups! before sending (a
//! Settings option). The window's text is given to Antidote as two zones, the subject and the
//! body ([`ComposeText`], the [`Texteur`]): the rich body through the editor's `TextMap`, so a
//! correction keeps the formatting around it. Everything stays on this PC: the conversation goes
//! to the Antidote installed here, over the loopback (`mailalm_antidote`).

use std::borrow::Cow;
use std::ops::Range;

use mailalm_antidote::{AntiOupsReply, Configuration, DocumentType, Error, Session, Style, StyleKind, Texteur, Tool, Unit, Zone};

use crate::compose::Body;

/// The zones' ids, as Antidote names them back.
pub const SUBJECT: &str = "subject";
pub const BODY: &str = "body";

/// A field of the compose window Antidote works on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Field {
    Subject,
    #[default]
    Body,
}

/// What Antidote asked of the window, done in the window's frame.
#[derive(Debug, Default)]
pub struct Requests {
    /// Send the message (Antidote's `envoi`, after Anti-Oups!): through the Send button's checks.
    pub send: bool,
    pub reply: Option<AntiOupsReply>,
    /// Select this range of the subject or of a plain-text body (the rich body is selected at
    /// once).
    pub select: Option<(Field, Range<usize>)>,
    /// Bring the window to the front.
    pub to_front: bool,
    /// The conversation ended: why, if it failed.
    pub ended: Option<Option<String>>,
    /// Antidote is not installed (found when the session started).
    pub missing: bool,
    pub memory: Option<String>,
}

/// The compose window's text as Antidote sees it.
pub struct ComposeText<'a> {
    pub subject: &'a mut String,
    pub body: &'a mut Body,
    /// Where the keyboard is.
    pub focus: Field,
    pub title: String,
    pub memory: Option<String>,
    /// False once the message is sent or the window closed.
    pub available: bool,
    pub requests: &'a mut Requests,
}

/// Whether `range` falls on characters of `text`.
fn fits(text: &str, range: &Range<usize>) -> bool {
    range.start <= range.end && range.end <= text.len() && text.is_char_boundary(range.start) && text.is_char_boundary(range.end)
}

impl ComposeText<'_> {
    fn body_text(&self) -> Cow<'_, str> {
        match &*self.body {
            Body::Rich(r) => Cow::Owned(r.editor.text_map().text),
            Body::Plain(s) => Cow::Borrowed(s.as_str()),
        }
    }

    fn body_zone(&self) -> Zone {
        let (selection, styles) = match &*self.body {
            Body::Rich(r) => {
                let map = r.editor.text_map();
                let mut styles = Vec::new();
                for (range, f) in map.formats(r.editor.document()) {
                    for (on, kind) in [(f.bold, StyleKind::Bold), (f.italic, StyleKind::Italic), (f.strike, StyleKind::Strikethrough)] {
                        if on && !range.is_empty() {
                            styles.push(Style { range: range.clone(), kind });
                        }
                    }
                }
                (r.editor.selected_text_range().filter(|s| !s.is_empty()), styles)
            }
            Body::Plain(_) => (None, Vec::new()),
        };
        Zone { id: BODY.to_owned(), text: self.body_text().into_owned(), selection, focused: self.focus == Field::Body, styles }
    }

    fn subject_zone(&self) -> Zone {
        Zone { id: SUBJECT.to_owned(), text: self.subject.clone(), selection: None, focused: self.focus == Field::Subject, styles: Vec::new() }
    }
}

impl Texteur for ComposeText<'_> {
    fn configuration(&self) -> Configuration {
        Configuration {
            document_title: Some(self.title.clone()),
            newline: Some("\n".to_owned()),
            cache_by_document_path: false,
            allow_newlines: Some(true),
            allow_no_break_space: Some(true),
            allow_thin_space: Some(true),
            allow_send: Some(true),
            replace_without_selection: Some(true),
            correction_memory: self.memory.clone(),
            document_type: Some(DocumentType::Text),
        }
    }

    fn zones(&self, for_selection: bool) -> Vec<Zone> {
        if for_selection {
            return vec![if self.focus == Field::Subject { self.subject_zone() } else { self.body_zone() }];
        }
        vec![self.subject_zone(), self.body_zone()]
    }

    fn zone_text(&self, id: &str) -> Option<Cow<'_, str>> {
        match id {
            SUBJECT => Some(Cow::Borrowed(self.subject.as_str())),
            BODY => Some(self.body_text()),
            _ => None,
        }
    }

    fn replace(&mut self, id: &str, range: Range<usize>, text: &str) -> bool {
        match id {
            SUBJECT => {
                if !fits(self.subject, &range) {
                    return false;
                }
                // The subject is one line.
                let line: String = text.chars().map(|c| if matches!(c, '\n' | '\r') { ' ' } else { c }).collect();
                self.subject.replace_range(range, &line);
                true
            }
            BODY => match &mut *self.body {
                Body::Rich(r) => r.editor.replace_text(range, text),
                Body::Plain(s) => {
                    if !fits(s, &range) {
                        return false;
                    }
                    s.replace_range(range, text);
                    true
                }
            },
            _ => false,
        }
    }

    fn select(&mut self, id: &str, range: Range<usize>) {
        match (id, &mut *self.body) {
            (BODY, Body::Rich(r)) => {
                r.editor.select_text(range);
            }
            (BODY, Body::Plain(_)) => self.requests.select = Some((Field::Body, range)),
            (SUBJECT, _) => self.requests.select = Some((Field::Subject, range)),
            _ => {}
        }
    }

    fn document_available(&self) -> bool {
        self.available
    }

    fn return_to_document(&mut self) {
        self.requests.to_front = true;
    }

    fn new_correction_memory(&mut self, memory: String) {
        self.requests.memory = Some(memory);
    }

    fn send(&mut self) {
        self.requests.send = true;
    }

    fn anti_oups_reply(&mut self, reply: AntiOupsReply) {
        self.requests.reply = Some(reply);
    }

    fn session_ended(&mut self, error: Option<&Error>) {
        if matches!(error, Some(Error::NotInstalled(_) | Error::Unavailable)) {
            self.requests.missing = true;
        }
        self.requests.ended = Some(error.map(|e| e.to_string()));
    }
}

/// A compose window's conversation with Antidote, started the first time it is needed.
#[derive(Default)]
pub struct Bridge {
    session: Option<Session>,
    pub requests: Requests,
    /// The state of the correction Antidote gave last (opaque), given back when it starts again.
    memory: Option<String>,
    /// Antidote turned out not to be installed: its buttons are hidden.
    pub missing: bool,
}

impl Bridge {
    fn session(&mut self, ctx: &egui::Context) -> &mut Session {
        if self.session.as_ref().is_some_and(Session::is_ended) {
            self.session = None;
        }
        self.session.get_or_insert_with(|| {
            let wake = ctx.clone();
            Session::start(Unit::CodePoints, move || wake.request_repaint())
        })
    }

    /// Opens one of Antidote's tools on the message.
    pub fn launch(&mut self, ctx: &egui::Context, tool: Tool) {
        self.session(ctx).launch(tool);
    }

    /// Asks Anti-Oups! to check the message; whether the request went.
    pub fn anti_oups(&mut self, ctx: &egui::Context, subject: &str, body: &str, recipients: &[String]) -> bool {
        self.session(ctx).anti_oups(subject, body, recipients).is_some()
    }

    /// Answers what Antidote asked since the last frame, on the window's text.
    pub fn process(&mut self, subject: &mut String, body: &mut Body, focus: Field, title: &str, available: bool) {
        let Some(session) = &mut self.session else { return };
        let mut text = ComposeText { subject, body, focus, title: title.to_owned(), memory: self.memory.clone(), available, requests: &mut self.requests };
        session.process(&mut text);
        if let Some(memory) = self.requests.memory.take() {
            self.memory = Some(memory);
        }
        if self.requests.missing {
            self.missing = true;
        }
    }

    /// Ends the conversation (the window closes or the message goes).
    pub fn close(&mut self) {
        if let Some(s) = &self.session {
            s.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use mailalm_compose::{Document, Editor, Format, Paragraph, Run, ToolbarState};

    use super::*;
    use crate::compose::Rich;

    fn rich(paragraphs: Vec<Paragraph>) -> Body {
        Body::Rich(Box::new(Rich { editor: Editor::from_document(Document::from_parts(paragraphs, Vec::new())), toolbar: ToolbarState::default() }))
    }

    fn text<'a>(subject: &'a mut String, body: &'a mut Body, requests: &'a mut Requests) -> ComposeText<'a> {
        ComposeText { subject, body, focus: Field::Body, title: "Plan".into(), memory: None, available: true, requests }
    }

    #[test]
    fn the_subject_and_the_body_are_two_zones() {
        let bold = Format { bold: true, ..Format::default() };
        let mut body = rich(vec![Paragraph::new(vec![Run::plain("Hello "), Run::new("wrld", bold)]), Paragraph::new(vec![Run::plain("Bye")])]);
        let mut subject = "Teh plan".to_owned();
        let mut requests = Requests::default();
        let t = text(&mut subject, &mut body, &mut requests);
        let zones = t.zones(false);
        assert_eq!(zones.iter().map(|z| (z.id.as_str(), z.text.as_str(), z.focused)).collect::<Vec<_>>(), [("subject", "Teh plan", false), ("body", "Hello wrld\nBye", true)]);
        assert_eq!(zones[1].styles, vec![Style { range: 6..10, kind: StyleKind::Bold }]);
        assert_eq!(t.zones(true).iter().map(|z| z.id.as_str()).collect::<Vec<_>>(), ["body"], "the focused zone only");
        assert_eq!(t.zone_text("subject").as_deref(), Some("Teh plan"));
        assert_eq!(t.zone_text("other"), None);
        assert_eq!(t.configuration().allow_send, Some(true));
    }

    #[test]
    fn corrections_land_where_antidote_says_and_keep_the_formatting() {
        let bold = Format { bold: true, ..Format::default() };
        let mut body = rich(vec![Paragraph::new(vec![Run::plain("Hello "), Run::new("wrld", bold)])]);
        let mut subject = "Teh plan".to_owned();
        let mut requests = Requests::default();
        let mut t = text(&mut subject, &mut body, &mut requests);
        assert!(t.replace("subject", 0..3, "The"));
        assert!(t.replace("body", 6..10, "world"));
        assert!(!t.replace("body", 6..99, "x"), "outside the text");
        assert!(t.replace("subject", 3..3, "\nnew"), "a line break in the subject becomes a space");
        assert!(!t.replace("nowhere", 0..1, "x"));
        t.select("body", 0..5);
        t.select("subject", 0..3);
        t.send();
        drop(t);
        assert_eq!(subject, "The new plan");
        let Body::Rich(r) = &body else { panic!("rich") };
        let runs = &r.editor.document().paragraphs[0].runs;
        assert_eq!((runs[1].text.as_str(), runs[1].format.bold), ("world", true));
        assert_eq!(r.editor.selected_text_range(), Some(0..5));
        assert_eq!(requests.select, Some((Field::Subject, 0..3)));
        assert!(requests.send);
    }

    #[test]
    fn a_plain_body_is_one_zone_too() {
        let mut body = Body::Plain("Bonjour à tous".to_owned());
        let mut subject = String::new();
        let mut requests = Requests::default();
        let mut t = text(&mut subject, &mut body, &mut requests);
        assert_eq!(t.zone_text("body").as_deref(), Some("Bonjour à tous"));
        assert!(!t.replace("body", 9..10, "a"), "inside a character");
        assert!(t.replace("body", 8..10, "a"));
        t.select("body", 0..7);
        t.session_ended(Some(&Error::NotInstalled("no key".into())));
        drop(t);
        assert!(matches!(&body, Body::Plain(s) if s == "Bonjour a tous"));
        assert_eq!(requests.select, Some((Field::Body, 0..7)));
        assert!(requests.missing && matches!(requests.ended, Some(Some(_))));
    }
}
