//! Writing a message, in its own window (PLAN.md sections 8, 9 and 12): rich text (the editor of
//! `mailalm_compose`, with its toolbar) or plain text, the account's signature (put above a
//! reply's quoted text, switched when the sender changes), its templates and quick replies,
//! attachments (Attach… or dropped on the window), Antidote's tools, drafts kept on the server,
//! and the checks before sending (the sender and its colour, recipients outside its domain, an
//! attachment mentioned but missing, the size the provider takes, Anti-Oups! when it is on). A
//! message sent waits for its undo delay, or for the time it is scheduled at (`send_queue`).

use std::io::Read;
use std::ops::Range;
use std::path::{Path, PathBuf};

use egui::text::{CCursor, CCursorRange};
use egui::text_edit::TextEditState;
use egui::{Align, Button, Color32, Frame, Id, Layout, Margin, RichText, Stroke, TextEdit, ViewportBuilder, ViewportCommand, ViewportId};
use mailalm_antidote::Tool;
use mailalm_compose::{
    Document, Editor, InlineImage, MAX_IMAGE_BYTES, Paragraph, RichEditor, Run, ToolbarRequest, ToolbarState, html_body, import_html, plain_text, toolbar,
};
use mailalm_engine::SizeLimit;
use mailalm_mime::{Outgoing, OutgoingAttachment, format_person, mentions_attachment, parse_recipients};
use mailalm_model::{AccountId, Person};

use crate::antidote_ui::{Bridge, Field};
use crate::attachments::size_text;
use crate::autocomplete;
use crate::icons::{self, Icon};
use crate::signatures::{self, Writing};
use crate::theme as t;
use crate::time;

/// The largest file read to be attached (the provider's limit is checked before sending).
const MAX_FILE: u64 = 40 * 1024 * 1024;
/// The most files attached to one message.
const MAX_FILES: usize = 100;

/// The message's text: rich (the editor and its toolbar) or plain.
pub enum Body {
    Rich(Box<Rich>),
    Plain(String),
}

pub struct Rich {
    pub editor: Editor,
    pub toolbar: ToolbarState,
}

impl Body {
    fn rich(doc: Document) -> Body {
        Body::Rich(Box::new(Rich { editor: Editor::from_document(doc), toolbar: ToolbarState::default() }))
    }

    /// The text as sent (the plain-text alternative of a rich message).
    fn text(&self) -> String {
        match self {
            Body::Rich(r) => plain_text(r.editor.document()),
            Body::Plain(s) => s.clone(),
        }
    }
}

/// An account a message can go out from, as the window shows it.
pub struct AccountChoice {
    pub id: AccountId,
    pub name: String,
    pub address: String,
    pub color: Color32,
    pub limit: SizeLimit,
}

/// What every compose window is shown with.
pub struct ComposeCtx<'a> {
    pub accounts: &'a [AccountChoice],
    pub writing: &'a [Writing],
    /// Offers addresses in the recipient fields (K1).
    pub suggest: &'a dyn Fn(&str) -> Vec<Person>,
    /// Antidote's connector is installed.
    pub antidote: bool,
    /// Anti-Oups! checks each message before it goes.
    pub anti_oups: bool,
}

impl ComposeCtx<'_> {
    fn writing(&self, id: AccountId) -> Option<&Writing> {
        self.writing.iter().find(|w| w.account == id)
    }

    fn account(&self, id: AccountId) -> Option<&AccountChoice> {
        self.accounts.iter().find(|a| a.id == id)
    }
}

/// What the window asks the main window to do.
pub enum ComposeAction {
    None,
    /// Send now (after the undo delay).
    Send(Box<Outgoing>),
    /// Send at this time (UTC seconds).
    Schedule(Box<Outgoing>, i64),
    /// Keep this as the window's draft on the server.
    SaveDraft(Box<Outgoing>),
    /// The window was closed with something written: its draft is saved, then it is forgotten.
    Close(Box<Outgoing>),
    Discard,
}

/// When a message is to go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum When {
    Now,
    At(i64),
}

/// The "Send later" form.
#[derive(Default)]
struct Later {
    date: String,
    time: String,
}

pub struct Compose {
    pub id: u64,
    pub account: AccountId,
    from: Person,
    to: String,
    cc: String,
    bcc: String,
    show_cc: bool,
    subject: String,
    body: Body,
    in_reply_to: Option<String>,
    references: Vec<String>,
    attachments: Vec<OutgoingAttachment>,
    error: Option<String>,
    notice: Option<String>,
    /// The user asked to discard a window with something in it.
    confirm_discard: bool,
    /// The text mentions an attachment and there is none: asked before sending.
    confirm_no_attachment: Option<When>,
    /// Waiting for Anti-Oups!'s answer before sending.
    checking: Option<When>,
    /// Anti-Oups! could not check: the user may send anyway.
    unchecked: Option<When>,
    later: Option<Later>,
    focus_set: bool,
    /// Something was written or changed since the window opened.
    touched: bool,
    revision: u64,
    /// The draft kept on the server: its account and what names it there.
    pub draft: Option<(AccountId, String)>,
    /// Closed: waiting for its draft to be saved, not shown.
    pub closing: bool,
    antidote: Bridge,
    focus: Field,
    /// The plain-text signature put in, to switch it.
    plain_signature: Option<String>,
}

/// A new message's text: a line to write on, an empty line (the signature goes below).
pub fn new_document() -> Document {
    Document::from_text("\n")
}

/// A reply's text: two lines to write on, an empty line, the attribution ("On …, … wrote:"),
/// then the message replied to, quoted.
pub fn reply_document(attribution: &str, original: Document) -> Document {
    let mut paragraphs = vec![Paragraph::default(), Paragraph::default(), Paragraph::default(), Paragraph::new(vec![Run::plain(attribution)])];
    let quoted = original.quoted();
    paragraphs.extend(quoted.paragraphs);
    Document::from_parts(paragraphs, quoted.images)
}

/// A forward's text: lines to write on, then the original's header and text, quoted.
pub fn forward_document(header: &[String], original: Document) -> Document {
    let mut inner: Vec<Paragraph> = header.iter().map(|line| Paragraph::new(vec![Run::plain(line.as_str())])).collect();
    inner.push(Paragraph::default());
    inner.extend(original.paragraphs);
    let quoted = Document::from_parts(inner, original.images).quoted();
    let mut paragraphs = vec![Paragraph::default(), Paragraph::default(), Paragraph::default()];
    paragraphs.extend(quoted.paragraphs);
    Document::from_parts(paragraphs, quoted.images)
}

/// A received message's text as a document: its HTML (with the pictures it shows), else its
/// plain text.
pub fn document_of(text: &str, html: Option<&mailalm_mime::HtmlBody>) -> Document {
    match html {
        Some(h) => import_html(&h.html, &editor_images(&h.images)),
        None => Document::from_text(text),
    }
}

/// The MIME writer's pictures as the editor takes them (checked by their content).
pub fn editor_images(images: &[mailalm_mime::InlineImage]) -> Vec<InlineImage> {
    images.iter().filter_map(|i| InlineImage::with_content_id(i.bytes.clone(), &i.content_id).ok()).collect()
}

impl Compose {
    /// A window for a message: `doc` is its text when the caller built it (a reply, a forward,
    /// a template), else the text of `o` (its HTML version when it has one). `error` says why a
    /// message came back; `plain` writes in plain text.
    pub fn new(id: u64, account: AccountId, o: &Outgoing, error: Option<String>, doc: Option<Document>, plain: bool) -> Self {
        let list = |v: &[Person]| v.iter().map(format_person).collect::<Vec<_>>().join(", ");
        let doc = doc.or_else(|| o.html.as_ref().map(|h| import_html(&h.html, &editor_images(&h.images))));
        let body = match doc {
            Some(d) if plain => Body::Plain(plain_text(&d)),
            Some(d) => Body::rich(d),
            None if plain => Body::Plain(o.text.clone()),
            None => Body::rich(Document::from_text(&o.text)),
        };
        let touched = error.is_some();
        let mut c = Self {
            id,
            account,
            from: o.from.clone(),
            to: list(&o.to),
            cc: list(&o.cc),
            bcc: list(&o.bcc),
            show_cc: !o.cc.is_empty() || !o.bcc.is_empty(),
            subject: o.subject.clone(),
            body,
            in_reply_to: o.in_reply_to.clone(),
            references: o.references.clone(),
            attachments: o.attachments.clone(),
            error,
            notice: None,
            confirm_discard: false,
            confirm_no_attachment: None,
            checking: None,
            unchecked: None,
            later: None,
            focus_set: false,
            touched,
            revision: 0,
            draft: None,
            closing: false,
            antidote: Bridge::default(),
            focus: Field::Body,
            plain_signature: None,
        };
        c.revision = c.editor_revision();
        c
    }

    fn editor_revision(&self) -> u64 {
        match &self.body {
            Body::Rich(r) => r.editor.revision(),
            Body::Plain(_) => 0,
        }
    }

    /// Puts the account's signature in (a new message, a reply, a forward). Not an edit: it is
    /// not undone with Ctrl+Z.
    pub fn put_signature(&mut self, writing: Option<&Writing>) {
        let html = writing.map(|w| w.signature.as_str()).unwrap_or("");
        match &mut self.body {
            Body::Rich(r) => {
                r.editor.set_signature(signatures::signature_document(html).as_ref());
                let doc = r.editor.document().clone();
                r.editor.set_document(doc);
            }
            Body::Plain(s) => {
                let new = signatures::signature_text(html);
                *s = signatures::place_plain(s, self.plain_signature.as_deref(), new.as_deref());
                self.plain_signature = new;
            }
        }
        self.revision = self.editor_revision();
    }

    /// The draft was saved on the server (`Ok`: what names it there), or not.
    pub fn draft_saved(&mut self, result: Result<String, String>) {
        match result {
            Ok(id) => {
                self.draft = Some((self.account, id));
                self.notice = Some(format!("Draft saved in Drafts at {}.", time::clock(time::now())));
            }
            Err(e) => {
                self.notice = None;
                self.error = Some(e);
            }
        }
    }

    /// Back from the undo queue or the schedule: shown again as it was.
    pub fn reopen(&mut self, notice: &str) {
        self.closing = false;
        self.focus_set = false;
        self.notice = Some(notice.to_owned());
        self.touched = true;
    }

    /// Back after its draft could not be saved when it was closed.
    pub fn reopen_with_error(&mut self, error: String) {
        self.closing = false;
        self.focus_set = false;
        self.error = Some(error);
    }

    /// Ends the conversation with Antidote (the message went or the window closed).
    pub fn close_antidote(&mut self) {
        self.antidote.close();
    }

    pub fn subject(&self) -> &str {
        &self.subject
    }

    fn has_content(&self) -> bool {
        self.touched || self.editor_revision() != self.revision
    }

    /// The message as written. `draft`: it may have no recipient yet.
    fn outgoing(&self, draft: bool) -> Result<Outgoing, String> {
        let to = parse_recipients(&self.to)?;
        let cc = parse_recipients(&self.cc)?;
        let bcc = parse_recipients(&self.bcc)?;
        if !draft && to.is_empty() && cc.is_empty() && bcc.is_empty() {
            return Err("Add at least one recipient.".to_owned());
        }
        let (text, html) = match &self.body {
            Body::Rich(r) => (plain_text(r.editor.document()), Some(html_body(r.editor.document()))),
            Body::Plain(s) => (s.clone(), None),
        };
        Ok(Outgoing {
            from: self.from.clone(),
            to,
            cc,
            bcc,
            subject: self.subject.trim().to_owned(),
            text,
            in_reply_to: self.in_reply_to.clone(),
            references: self.references.clone(),
            attachments: self.attachments.clone(),
            calendar: None,
            html,
        })
    }

    /// What counts against the account's limit, and the limit.
    fn measure(limit: SizeLimit, o: &Outgoing) -> (usize, usize) {
        match limit {
            SizeLimit::Attachments(max) => (o.attachments_size(), max),
            SizeLimit::Message(max) => (o.estimated_size(), max),
        }
    }

    /// What is wrong with the message's weight for this account, if anything.
    fn over_limit(limit: SizeLimit, o: &Outgoing) -> Option<String> {
        let (used, max) = Self::measure(limit, o);
        if used <= max {
            return None;
        }
        Some(match limit {
            SizeLimit::Attachments(_) => format!("The attachments weigh {}: this account sends at most {}. Remove one, or share the file by a link.", size_text(used), size_text(max)),
            SizeLimit::Message(_) => format!(
                "The message would weigh about {}: MailALM sends at most {} from an Outlook account for now. Remove or shrink an attachment or a picture, or share the file by a link.",
                size_text(used),
                size_text(max)
            ),
        })
    }

    /// The recipients outside the sender's domain.
    fn external(&self) -> Vec<String> {
        let own = self.from.address.rsplit_once('@').map(|(_, d)| d.to_ascii_lowercase()).unwrap_or_default();
        [&self.to, &self.cc, &self.bcc]
            .iter()
            .flat_map(|f| parse_recipients(f).unwrap_or_default())
            .filter(|p| p.address.rsplit_once('@').is_some_and(|(_, d)| d.to_ascii_lowercase() != own))
            .map(|p| p.address)
            .collect()
    }

    /// The Send button's checks, then the message goes (now or at its time): the recipients and
    /// addresses, the size the account takes, a time to come, an attachment mentioned but
    /// missing (asked unless `attachment_ok`), Anti-Oups! when it is on (unless `checked`).
    /// Antidote's own `envoi` comes through here too.
    fn request_send(&mut self, ctx: &egui::Context, cx: &ComposeCtx<'_>, when: When, attachment_ok: bool, checked: bool) -> ComposeAction {
        self.error = None;
        let o = match self.outgoing(false) {
            Ok(o) => o,
            Err(e) => {
                self.error = Some(e);
                return ComposeAction::None;
            }
        };
        let Some(account) = cx.account(self.account) else {
            self.error = Some("This account is no longer in MailALM.".to_owned());
            return ComposeAction::None;
        };
        if let Some(e) = Self::over_limit(account.limit, &o) {
            self.error = Some(e);
            return ComposeAction::None;
        }
        if let When::At(at) = when
            && at <= time::now()
        {
            self.error = Some("That time has passed: choose one to come.".to_owned());
            return ComposeAction::None;
        }
        if !attachment_ok && o.attachments.is_empty() && (mentions_attachment(&o.text) || mentions_attachment(&o.subject)) {
            self.confirm_no_attachment = Some(when);
            return ComposeAction::None;
        }
        if !checked && cx.anti_oups && cx.antidote && !self.antidote.missing {
            if self.antidote.anti_oups(ctx, &o.subject, &o.text, &o.recipients()) {
                self.checking = Some(when);
                self.notice = Some("Antidote's Anti-Oups! is checking the message…".to_owned());
            } else {
                self.unchecked = Some(when);
            }
            return ComposeAction::None;
        }
        self.later = None;
        match when {
            When::Now => ComposeAction::Send(Box::new(o)),
            When::At(at) => ComposeAction::Schedule(Box::new(o), at),
        }
    }

    /// Adds files from the computer; a file that cannot be attached is said.
    fn attach(&mut self, paths: &[PathBuf]) {
        let mut problems: Vec<String> = Vec::new();
        for path in paths {
            if self.attachments.len() >= MAX_FILES {
                problems.push(format!("At most {MAX_FILES} files can be attached."));
                break;
            }
            match read_file(path, MAX_FILE) {
                Ok((name, bytes)) => self.attachments.push(OutgoingAttachment::new(&name, bytes)),
                Err(e) => problems.push(e),
            }
        }
        self.confirm_no_attachment = None;
        self.touched = true;
        self.error = (!problems.is_empty()).then(|| problems.join(" "));
    }

    /// A picture from the computer, in the rich text at the caret.
    fn insert_picture(&mut self) {
        let Some(path) = rfd::FileDialog::new().set_title("Insert a picture").add_filter("Pictures", &["png", "jpg", "jpeg", "gif"]).pick_file() else { return };
        let Body::Rich(r) = &mut self.body else { return };
        let result = read_file(&path, MAX_IMAGE_BYTES as u64)
            .and_then(|(_, bytes)| InlineImage::new(bytes).map_err(|e| e.to_string()))
            .and_then(|image| r.editor.insert_image(image).map_err(|e| e.to_string()));
        if let Err(e) = result {
            self.error = Some(e);
        }
    }

    /// Text put in at the caret (a template's or a quick reply's).
    fn insert(&mut self, ctx: &egui::Context, doc: &Document) {
        let id = self.body_id();
        match &mut self.body {
            Body::Rich(r) => r.editor.insert_document(doc),
            Body::Plain(s) => insert_at_caret(ctx, id, s, &plain_text(doc)),
        }
        self.touched = true;
    }

    fn body_id(&self) -> Id {
        Id::unique(("mailalm-compose-body", self.id))
    }

    fn subject_id(&self) -> Id {
        Id::unique(("mailalm-compose-subject", self.id))
    }

    /// Another sender: its address, and its signature in place of the other's.
    fn switch_account(&mut self, cx: &ComposeCtx<'_>, id: AccountId) {
        let Some(a) = cx.account(id) else { return };
        self.account = id;
        self.from = Person { name: String::new(), address: a.address.clone() };
        let html = cx.writing(id).map(|w| w.signature.as_str()).unwrap_or("");
        match &mut self.body {
            Body::Rich(r) => r.editor.set_signature(signatures::signature_document(html).as_ref()),
            Body::Plain(s) => {
                let new = signatures::signature_text(html);
                *s = signatures::place_plain(s, self.plain_signature.as_deref(), new.as_deref());
                self.plain_signature = new;
            }
        }
        self.touched = true;
        self.error = None;
    }

    /// Rich text to plain text (formatting and pictures go) or back.
    fn switch_mode(&mut self, cx: &ComposeCtx<'_>) {
        let html = cx.writing(self.account).map(|w| w.signature.as_str()).unwrap_or("");
        let body = std::mem::replace(&mut self.body, Body::Plain(String::new()));
        self.body = match body {
            Body::Rich(r) => {
                let text = plain_text(r.editor.document());
                self.plain_signature = signatures::signature_text(html).filter(|s| text.contains(s.as_str()));
                Body::Plain(text)
            }
            Body::Plain(s) => {
                // The signature comes back as the editor's own, where it belongs.
                let had = self.plain_signature.take();
                let text = signatures::place_plain(&s, had.as_deref(), None);
                let mut rich = Rich { editor: Editor::from_document(Document::from_text(&text)), toolbar: ToolbarState::default() };
                if had.is_some() {
                    rich.editor.set_signature(signatures::signature_document(html).as_ref());
                }
                Body::Rich(Box::new(rich))
            }
        };
        self.touched = true;
    }

    /// Shows the window (while it is not closing).
    pub fn show(&mut self, ctx: &egui::Context, cx: &ComposeCtx<'_>) -> ComposeAction {
        let title = if self.subject.trim().is_empty() { "New message".to_owned() } else { self.subject.trim().to_owned() };
        let builder = ViewportBuilder::default()
            .with_title(format!("{title} · MailALM"))
            .with_app_id("mailalm")
            .with_inner_size([860.0, 700.0])
            .with_min_inner_size([520.0, 380.0])
            .with_drag_and_drop(true);
        ctx.show_viewport_immediate(ViewportId::from_hash_of(("mailalm-compose", self.id)), builder, |ui, _class| {
            let ctx = ui.ctx().clone();
            let mut action = ComposeAction::None;

            // Antidote's requests, on this window's text.
            self.antidote.process(&mut self.subject, &mut self.body, self.focus, &title, !self.closing);
            self.after_antidote(&ctx, cx, &mut action);

            if ctx.input(|i| i.viewport().close_requested()) {
                if !self.has_content() {
                    return ComposeAction::Discard;
                }
                match self.outgoing(true) {
                    Ok(o) => {
                        self.closing = true;
                        return ComposeAction::Close(Box::new(o));
                    }
                    Err(e) => {
                        ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                        self.error = Some(format!("The draft could not be kept: {e} Correct it, or Discard."));
                    }
                }
            }
            // Files dropped on this window: attached. Only their paths are taken here; each file
            // is read with a size cap (`read_file`).
            let (dropped, hovering): (Vec<PathBuf>, bool) =
                ctx.input(|i| (i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect(), !i.raw.hovered_files.is_empty()));
            if !dropped.is_empty() {
                self.attach(&dropped);
            }
            egui::CentralPanel::default().frame(Frame::new().fill(t::panel()).inner_margin(Margin::symmetric(18, 14))).show(ui, |ui| {
                self.commands(ui, &ctx, cx, &mut action);
                self.banners(ui, &ctx, cx, &mut action);
                ui.add_space(10.0);
                self.fields(ui, cx);
                self.attachment_list(ui, cx);
                ui.add_space(8.0);
                self.body_area(ui);
            });
            if hovering {
                let screen = ctx.content_rect();
                let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::unique(("mailalm-compose-drop", self.id))));
                painter.rect_filled(screen, 0.0, Color32::from_black_alpha(150));
                painter.text(screen.center(), egui::Align2::CENTER_CENTER, "Drop to attach", egui::FontId::proportional(22.0), t::text());
            }
            action
        })
    }

    /// What Antidote asked during this frame.
    fn after_antidote(&mut self, ctx: &egui::Context, cx: &ComposeCtx<'_>, action: &mut ComposeAction) {
        let r = &mut self.antidote.requests;
        let (to_front, select, ended, reply, send) = (std::mem::take(&mut r.to_front), r.select.take(), r.ended.take(), r.reply.take(), std::mem::take(&mut r.send));
        if to_front {
            ctx.send_viewport_cmd(ViewportCommand::Focus);
        }
        if let Some((field, range)) = select {
            match (field, &self.body) {
                (Field::Subject, _) => select_chars(ctx, self.subject_id(), &self.subject, range),
                (Field::Body, Body::Plain(s)) => select_chars(ctx, self.body_id(), s, range),
                (Field::Body, Body::Rich(_)) => {}
            }
        }
        if let Some(ended) = ended {
            if let Some(e) = ended {
                self.notice = None;
                self.error = Some(format!("Antidote: {e}"));
            }
            if let Some(when) = self.checking.take() {
                self.unchecked = Some(when);
            }
        }
        if let Some(reply) = reply
            && let Some(when) = self.checking.take()
        {
            self.notice = None;
            if let Some(code) = &reply.error_code {
                self.error = Some(format!("Anti-Oups! could not check the message ({code})."));
                self.unchecked = Some(when);
            } else if reply.should_open_corrector() {
                self.antidote.launch(ctx, Tool::Corrector);
                self.notice = Some("Anti-Oups! found something to look at: Antidote's corrector is open. Send again when you are ready.".to_owned());
            } else {
                *action = self.request_send(ctx, cx, when, true, true);
            }
        }
        if send {
            // Antidote just checked it: the Send button's other checks, then it goes.
            *action = self.request_send(ctx, cx, When::Now, false, true);
        }
        if self.editor_revision() != self.revision {
            self.touched = true;
        }
    }

    /// The command row: Send, Send later, Save draft, Attach, Templates, Antidote, Discard, and
    /// rich or plain text.
    fn commands(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, cx: &ComposeCtx<'_>, action: &mut ComposeAction) {
        ui.horizontal_wrapped(|ui| {
            let busy = self.checking.is_some();
            if ui.add_enabled(!busy, Button::new(RichText::new("Send").strong().color(t::on_accent())).fill(t::accent())).clicked() {
                *action = self.request_send(ctx, cx, When::Now, false, false);
            }
            if ui.add_enabled(!busy, Button::new("Send later…")).on_hover_text("Choose the date and time it goes").clicked() {
                self.later = if self.later.is_some() { None } else { Some(Later::default()) };
            }
            if ui.button("Save draft").on_hover_text("Keep it in the account's Drafts folder on the server").clicked() {
                match self.outgoing(true) {
                    Ok(o) => {
                        self.notice = Some("Saving the draft…".to_owned());
                        *action = ComposeAction::SaveDraft(Box::new(o));
                    }
                    Err(e) => self.error = Some(format!("The draft could not be kept: {e}")),
                }
            }
            if ui.button("Attach…").clicked()
                && let Some(paths) = rfd::FileDialog::new().set_title("Attach files").pick_files()
            {
                self.attach(&paths);
            }
            self.templates_menu(ui, ctx, cx);
            if cx.antidote && !self.antidote.missing {
                ui.separator();
                for (label, tool, tip) in [
                    ("Corrector", Tool::Corrector, "Antidote's corrector on the subject and the text"),
                    ("Dictionaries", Tool::Dictionaries, "Antidote's dictionaries on the word selected"),
                    ("Guides", Tool::Guides, "Antidote's guides on the text selected"),
                ] {
                    if ui.button(label).on_hover_text(tip).clicked() {
                        self.antidote.launch(ctx, tool);
                    }
                }
            }
            ui.separator();
            if ui.button("Discard").clicked() {
                if self.has_content() {
                    self.confirm_discard = true;
                } else {
                    *action = ComposeAction::Discard;
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let rich = matches!(self.body, Body::Rich(_));
                let (label, tip) =
                    if rich { ("Plain text", "Write in plain text: the formatting and the pictures are lost") } else { ("Rich text", "Write with formatting") };
                if ui.small_button(label).on_hover_text(tip).clicked() {
                    self.switch_mode(cx);
                }
            });
        });
    }

    /// Templates and quick replies of the sending account.
    fn templates_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, cx: &ComposeCtx<'_>) {
        let writing = cx.writing(self.account);
        let mut chosen: Option<(String, Document)> = None;
        let _ = ui.menu_button(("Templates", icons::atom(Icon::ChevronDown)), |ui| {
            let templates = writing.map(|w| w.templates.as_slice()).unwrap_or_default();
            let replies = writing.map(|w| w.quick_replies.as_slice()).unwrap_or_default();
            if templates.is_empty() && replies.is_empty() {
                ui.label(RichText::new("No template or quick reply for this account yet: add them in Settings, Writing.").color(t::faint()));
                return;
            }
            if !templates.is_empty() {
                ui.label(RichText::new("TEMPLATES").size(11.0).strong().color(t::faint()));
                for tpl in templates {
                    if ui.button(&tpl.name).clicked() {
                        chosen = Some((tpl.subject.clone(), import_html(&tpl.html, &[])));
                        ui.close();
                    }
                }
            }
            if !replies.is_empty() {
                ui.label(RichText::new("QUICK REPLIES").size(11.0).strong().color(t::faint()));
                for q in replies {
                    if ui.button(&q.name).on_hover_text(&q.text).clicked() {
                        chosen = Some((String::new(), Document::from_text(&q.text)));
                        ui.close();
                    }
                }
            }
        });
        if let Some((subject, doc)) = chosen {
            if self.subject.trim().is_empty() && !subject.trim().is_empty() {
                self.subject = subject;
            }
            self.insert(ctx, &doc);
        }
    }

    /// Questions, errors and what to know, above the fields.
    fn banners(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, cx: &ComposeCtx<'_>, action: &mut ComposeAction) {
        let warn = |ui: &mut egui::Ui, add: &mut dyn FnMut(&mut egui::Ui)| {
            ui.add_space(6.0);
            Frame::new().fill(t::warn_bg()).corner_radius(6).inner_margin(Margin::symmetric(10, 6)).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| add(ui));
            });
        };
        if self.confirm_discard {
            let draft = self.draft.is_some();
            warn(ui, &mut |ui| {
                let text = if draft { "Discard this message? What you wrote is lost, and its draft is deleted from Drafts." } else { "Discard this message? What you wrote is lost." };
                ui.label(RichText::new(text).color(t::warn()));
                if ui.button("Discard").clicked() {
                    *action = ComposeAction::Discard;
                }
                if ui.button("Keep writing").clicked() {
                    self.confirm_discard = false;
                }
            });
        }
        if let Some(when) = self.confirm_no_attachment {
            let mut answer: Option<u8> = None;
            warn(ui, &mut |ui| {
                ui.label(RichText::new("Your message mentions an attachment, but nothing is attached.").color(t::warn()));
                for (n, label) in ["Send anyway", "Attach a file…", "Cancel"].into_iter().enumerate() {
                    if ui.button(label).clicked() {
                        answer = Some(n as u8);
                    }
                }
            });
            match answer {
                Some(0) => {
                    self.confirm_no_attachment = None;
                    *action = self.request_send(ctx, cx, when, true, false);
                }
                Some(1) => {
                    self.confirm_no_attachment = None;
                    if let Some(paths) = rfd::FileDialog::new().set_title("Attach files").pick_files() {
                        self.attach(&paths);
                    }
                }
                Some(_) => self.confirm_no_attachment = None,
                None => {}
            }
        }
        if let Some(when) = self.unchecked {
            let mut answer: Option<bool> = None;
            warn(ui, &mut |ui| {
                ui.label(RichText::new("Antidote's Anti-Oups! could not check this message.").color(t::warn()));
                if ui.button("Send without the check").clicked() {
                    answer = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    answer = Some(false);
                }
            });
            if let Some(send) = answer {
                self.unchecked = None;
                if send {
                    *action = self.request_send(ctx, cx, when, true, true);
                }
            }
        }
        if self.later.is_some() {
            self.later_form(ui, ctx, cx, action);
        }
        if let Some(e) = &self.error {
            ui.add_space(6.0);
            ui.label(RichText::new(e).color(t::danger()));
        }
        if let Some(n) = &self.notice {
            ui.add_space(4.0);
            ui.label(RichText::new(n).size(12.5).color(t::accent_text()));
        }
    }

    /// The "Send later" form: a day and a time, or a usual choice.
    fn later_form(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, cx: &ComposeCtx<'_>, action: &mut ComposeAction) {
        let now = time::now();
        let today = time::local_day(now);
        let mut at: Option<i64> = None;
        let mut close = false;
        let mut problem: Option<String> = None;
        if let Some(later) = &mut self.later {
            ui.add_space(6.0);
            Frame::new().fill(t::panel2()).stroke(Stroke::new(1.0, t::line())).corner_radius(6).inner_margin(Margin::symmetric(10, 8)).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Send on");
                    ui.add(TextEdit::singleline(&mut later.date).hint_text(time::date(today + 1)).desired_width(96.0));
                    ui.label("at");
                    ui.add(TextEdit::singleline(&mut later.time).hint_text("08:00").desired_width(52.0));
                    if ui.button("Schedule").clicked() {
                        let day = mailalm_model::pim::parse_date(later.date.trim());
                        let clock = time::parse_clock(&later.time);
                        match (day, clock) {
                            (Some(d), Some(c)) => at = Some(time::utc_of_local(d * 86_400 + c)),
                            (None, _) => problem = Some("Write the day as 2026-11-25.".to_owned()),
                            (_, None) => problem = Some("Write the time as 14:30.".to_owned()),
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    let monday = today + (7 - time::weekday_index(today)).max(1);
                    for (label, when) in [
                        ("In one hour", now + 3600),
                        ("Tomorrow 08:00", time::utc_of_local((today + 1) * 86_400 + 8 * 3600)),
                        ("Monday 08:00", time::utc_of_local(monday * 86_400 + 8 * 3600)),
                    ] {
                        if ui.small_button(label).clicked() {
                            at = Some(when);
                        }
                    }
                });
                ui.label(
                    RichText::new("MailALM sends it at that time if it is running; if it is closed then, it sends it as soon as it starts again, and says so.")
                        .size(11.5)
                        .color(t::faint()),
                );
            });
        }
        if close {
            self.later = None;
        }
        if let Some(e) = problem {
            self.error = Some(e);
        }
        if let Some(at) = at {
            *action = self.request_send(ctx, cx, When::At(at), false, false);
        }
    }

    /// From, To, Cc, Bcc and Subject; recipients outside the sender's domain are said.
    fn fields(&mut self, ui: &mut egui::Ui, cx: &ComposeCtx<'_>) {
        let mut switch: Option<AccountId> = None;
        egui::Grid::new(("mailalm-compose-fields", self.id)).num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
            ui.label(RichText::new("From").color(t::muted()));
            ui.horizontal(|ui| {
                let current = cx.account(self.account);
                let color = current.map_or(t::faint(), |a| a.color);
                let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                ui.painter().circle_filled(r.center(), 5.0, color);
                let shown = current.map_or_else(|| format_person(&self.from), |a| format!("{} · {}", a.name, a.address));
                egui::ComboBox::from_id_salt(("mailalm-compose-from", self.id)).selected_text(RichText::new(shown).strong().color(color)).show_ui(ui, |ui| {
                    for a in cx.accounts {
                        if ui.selectable_label(a.id == self.account, RichText::new(format!("{} · {}", a.name, a.address)).color(a.color)).clicked() && a.id != self.account {
                            switch = Some(a.id);
                        }
                    }
                });
            });
            ui.end_row();
            ui.label(RichText::new("To").color(t::muted()));
            ui.horizontal(|ui| {
                let width = ui.available_width() - 70.0;
                let to = autocomplete::field(ui, Id::unique(("mailalm-compose-to", self.id)), &mut self.to, width, "name@example.com, …", cx.suggest);
                if to.changed() {
                    self.touched = true;
                }
                if !self.focus_set && self.to.is_empty() {
                    to.request_focus();
                    self.focus_set = true;
                }
                if !self.show_cc && ui.small_button("Cc / Bcc").clicked() {
                    self.show_cc = true;
                }
            });
            ui.end_row();
            if self.show_cc {
                ui.label(RichText::new("Cc").color(t::muted()));
                let width = ui.available_width();
                if autocomplete::field(ui, Id::unique(("mailalm-compose-cc", self.id)), &mut self.cc, width, "", cx.suggest).changed() {
                    self.touched = true;
                }
                ui.end_row();
                ui.label(RichText::new("Bcc").color(t::muted()));
                let width = ui.available_width();
                if autocomplete::field(ui, Id::unique(("mailalm-compose-bcc", self.id)), &mut self.bcc, width, "", cx.suggest).changed() {
                    self.touched = true;
                }
                ui.end_row();
            }
            ui.label(RichText::new("Subject").color(t::muted()));
            let subject_id = self.subject_id();
            let subject = ui.add(TextEdit::singleline(&mut self.subject).id(subject_id).desired_width(f32::INFINITY));
            if subject.changed() {
                self.touched = true;
            }
            if subject.has_focus() {
                self.focus = Field::Subject;
            }
            ui.end_row();
        });
        if let Some(id) = switch {
            self.switch_account(cx, id);
        }
        let external = self.external();
        if !external.is_empty() {
            let domain = self.from.address.rsplit_once('@').map_or("", |(_, d)| d).to_owned();
            let shown: Vec<&str> = external.iter().take(4).map(String::as_str).collect();
            let more = external.len().saturating_sub(shown.len());
            let text = format!(
                "Outside {domain}: {}{}",
                shown.join(", "),
                if more > 0 { format!(" and {more} more") } else { String::new() }
            );
            ui.add_space(4.0);
            ui.label(RichText::new(text).size(12.0).color(t::warn())).on_hover_text("These recipients are not at the sender's own domain: check that they should get this message.");
        }
    }

    /// The attached files, each with its remove button, and their weight against the account's
    /// limit.
    fn attachment_list(&mut self, ui: &mut egui::Ui, cx: &ComposeCtx<'_>) {
        if self.attachments.is_empty() {
            return;
        }
        ui.add_space(8.0);
        let mut remove: Option<usize> = None;
        ui.horizontal_wrapped(|ui| {
            for (n, a) in self.attachments.iter().enumerate() {
                Frame::new().fill(t::panel2()).stroke(Stroke::new(1.0, t::line())).corner_radius(6).inner_margin(Margin::symmetric(8, 4)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&a.name).strong());
                        ui.label(RichText::new(size_text(a.bytes.len())).size(12.0).color(t::faint()));
                        if icons::button(ui, Icon::Close, egui::vec2(18.0, 18.0), t::muted(), t::sel(), "Remove this attachment").clicked() {
                            remove = Some(n);
                        }
                    });
                });
            }
        });
        if let Some(n) = remove
            && n < self.attachments.len()
        {
            self.attachments.remove(n);
            self.touched = true;
            self.error = None;
        }
        let Some(limit) = cx.account(self.account).map(|a| a.limit) else { return };
        let probe = Outgoing { text: self.body.text(), attachments: self.attachments.clone(), ..Outgoing::default() };
        let (used, max) = Self::measure(limit, &probe);
        let what = match limit {
            SizeLimit::Attachments(_) => "Attachments",
            SizeLimit::Message(_) => "Message, about",
        };
        let color = if used > max { t::danger() } else { t::faint() };
        ui.label(RichText::new(format!("{what} {} of {} at most for this account", size_text(used), size_text(max))).size(12.0).color(color));
    }

    /// The text: the toolbar and the rich editor, or a plain text box.
    fn body_area(&mut self, ui: &mut egui::Ui) {
        let body_id = self.body_id();
        let mut picture = false;
        if let Body::Rich(r) = &mut self.body {
            let Rich { editor, toolbar: state } = &mut **r;
            if toolbar(ui, editor, state) == Some(ToolbarRequest::InsertImage) {
                picture = true;
            }
            ui.add_space(6.0);
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let min = ui.available_height().max(240.0);
            let response = match &mut self.body {
                Body::Rich(r) => RichEditor::new(&mut r.editor).id(body_id).min_height(min).hint_text("Write your message").show(ui),
                Body::Plain(s) => ui.add_sized([ui.available_width(), min], TextEdit::multiline(s).id(body_id).lock_focus(true).desired_width(f32::INFINITY)),
            };
            if response.changed() {
                self.touched = true;
            }
            if response.has_focus() {
                self.focus = Field::Body;
            }
            if !self.focus_set {
                response.request_focus();
                self.focus_set = true;
            }
        });
        if picture {
            self.insert_picture();
        }
    }
}

/// `insert` put in `text` where the caret of the text box `id` is (at the start without one).
fn insert_at_caret(ctx: &egui::Context, id: Id, text: &mut String, insert: &str) {
    let caret = TextEditState::load(ctx, id).and_then(|s| s.cursor.char_range()).map(|r| r.primary.index.0);
    let at = caret.and_then(|c| text.char_indices().nth(c).map(|(b, _)| b).or_else(|| (c >= text.chars().count()).then_some(text.len()))).unwrap_or(0);
    text.insert_str(at, insert);
}

/// Selects the byte range `range` of `text` in the text box `id`, and gives it the keyboard.
fn select_chars(ctx: &egui::Context, id: Id, text: &str, range: Range<usize>) {
    let (Some(a), Some(b)) = (text.get(..range.start), text.get(..range.end)) else { return };
    let (a, b) = (a.chars().count(), b.chars().count());
    let mut state = TextEditState::load(ctx, id).unwrap_or_default();
    state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(a), CCursor::new(b))));
    state.store(ctx, id);
    ctx.memory_mut(|m| m.request_focus(id));
}

/// A file from the computer: its name and its bytes, at most `max`.
fn read_file(path: &Path, max: u64) -> Result<(String, Vec<u8>), String> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "attachment".to_owned());
    let meta = std::fs::metadata(path).map_err(|e| format!("{name} could not be read: {e}."))?;
    if !meta.is_file() {
        return Err(format!("{name} is a folder: attach the files in it instead."));
    }
    let too_large = || format!("{name} is larger than {} MB: share it by a link instead.", max >> 20);
    if meta.len() > max {
        return Err(too_large());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path).and_then(|f| f.take(max + 1).read_to_end(&mut bytes)).map_err(|e| format!("{name} could not be read: {e}."))?;
    if bytes.len() as u64 > max {
        return Err(too_large());
    }
    Ok((name, bytes))
}

#[cfg(test)]
mod tests {
    use mailalm_compose::Kind;

    use super::*;

    fn texts(d: &Document) -> Vec<String> {
        d.paragraphs.iter().map(Paragraph::text).collect()
    }

    #[test]
    fn a_reply_quotes_the_message_below_its_signature() {
        let original = import_html("<p>Hi <b>Alex</b></p><p>See you</p>", &[]);
        let mut c = Compose::new(1, AccountId(1), &Outgoing::default(), None, Some(reply_document("On Mon, Zoe wrote:", original)), false);
        let w = Writing { signature: "<p>Alex</p>".into(), ..Writing::new(AccountId(1)) };
        c.put_signature(Some(&w));
        let Body::Rich(r) = &c.body else { panic!("rich") };
        let d = r.editor.document();
        assert_eq!(texts(d), ["", "", "Alex", "", "On Mon, Zoe wrote:", "Hi Alex", "See you"]);
        assert!(d.paragraphs[5].quote == 1 && d.paragraphs[5].runs.iter().any(|r| r.format.bold));
        assert!(!r.editor.can_undo(), "the signature is no edit to undo");
        assert!(!c.has_content(), "nothing written yet");
    }

    #[test]
    fn a_forward_quotes_its_header_and_text() {
        let d = forward_document(&["---------- Forwarded message ----------".into(), "From: Zoe".into()], Document::from_text("Body"));
        assert_eq!(texts(&d), ["", "", "", "---------- Forwarded message ----------", "From: Zoe", "", "Body"]);
        assert!(d.paragraphs.iter().skip(3).all(|p| p.quote == 1 && p.kind != Kind::Bullet));
        assert_eq!(mailalm_compose::signature_slot(&d), 2);
    }

    #[test]
    fn a_plain_message_gets_its_signature_switched() {
        let mut c = Compose::new(2, AccountId(1), &Outgoing { text: "Hi".into(), ..Outgoing::default() }, None, None, true);
        c.put_signature(Some(&Writing { signature: "<p>Alex</p>".into(), ..Writing::new(AccountId(1)) }));
        assert!(matches!(&c.body, Body::Plain(s) if s == "Hi\n\n-- \nAlex"));
        let accounts = [
            AccountChoice { id: AccountId(1), name: "Home".into(), address: "alex@videotron.ca".into(), color: Color32::RED, limit: SizeLimit::Attachments(10) },
            AccountChoice { id: AccountId(2), name: "Work".into(), address: "alex@work.example".into(), color: Color32::BLUE, limit: SizeLimit::Attachments(10) },
        ];
        let writing = [Writing { signature: "<p>Alex at work</p>".into(), ..Writing::new(AccountId(2)) }];
        let suggest = |_: &str| -> Vec<Person> { Vec::new() };
        let cx = ComposeCtx { accounts: &accounts, writing: &writing, suggest: &suggest, antidote: false, anti_oups: false };
        c.switch_account(&cx, AccountId(2));
        assert_eq!(c.from.address, "alex@work.example");
        assert!(matches!(&c.body, Body::Plain(s) if s == "Hi\n\n-- \nAlex at work"), "{:?}", c.body.text());
        // To rich text and back: the signature stays the editor's own.
        c.switch_mode(&cx);
        let Body::Rich(r) = &c.body else { panic!("rich") };
        assert!(r.editor.document().paragraphs.iter().any(|p| p.signature && p.text() == "Alex at work"));
        assert_eq!(r.editor.document().paragraphs.iter().filter(|p| p.text() == "-- ").count(), 0);
    }

    #[test]
    fn recipients_outside_the_domain_are_found() {
        let o = Outgoing {
            from: Person { name: String::new(), address: "alex@videotron.ca".into() },
            to: vec![Person { name: String::new(), address: "sam@VIDEOTRON.ca".into() }, Person { name: String::new(), address: "zoe@studio.example".into() }],
            bcc: vec![Person { name: String::new(), address: "boss@gmail.com".into() }],
            ..Outgoing::default()
        };
        let c = Compose::new(3, AccountId(1), &o, None, None, false);
        assert_eq!(c.external(), ["zoe@studio.example", "boss@gmail.com"]);
        assert!(c.outgoing(true).is_ok());
        let empty = Compose::new(4, AccountId(1), &Outgoing::default(), None, None, false);
        assert!(empty.outgoing(false).is_err() && empty.outgoing(true).is_ok(), "a draft needs no recipient");
    }
}
