// Protocol ported from Druide's antidote-api-python 1.1.0 (MIT, © 2026 Druide informatique inc.).
// From: messages.py (packets), requete_sortante.py, requete_entrante.py, params.py, reponses.py,
// configuration_texteur.py, zone_de_texte.py, info_style.py, style_texte.py, type_document.py,
// type_identifiant_cache.py, and the dispatch of agent_connectix.py. The JSON names are those the
// Python produces (dataclasses_json, camelCase), not the README's where the two differ; the size
// caps and every check on what Antidote sends are MailALM's.

//! The messages of Antidote's integration API (version 2), both ways, and the packets that carry
//! them.
//!
//! Every WebSocket message is a packet `{"idPaquet", "totalPaquet", "donnees"}`; the `donnees` of a
//! message's packets, joined in order, are the message's JSON. MailALM sends one packet per
//! message, as the Python does.
//!
//! MailALM sends `LanceOutil` (open a tool) and `AntiOups` (check a message before sending), and
//! answers Antidote's requests: `init`, `cheminDocument`, `docEstDisponible`, `donneZonesTexte`,
//! `editionPossible`, `remplace`. Antidote also sends `selectionne`, `retourneAuDocument`,
//! `nouvelleMemoireCorrection`, `envoi` and `reponseAntiOups`, which take no answer.

use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Error, base64, clip};

/// The API version the Python sends (`version_api`).
const API_VERSION: u32 = 2;
/// The largest WebSocket frame, message, or joined packets, both ways.
pub(crate) const MAX_MESSAGE: usize = 16 * 1024 * 1024;
/// The most packets in one message.
const MAX_PACKETS: usize = 65_536;
/// The longest id: a message's, a zone's, an Anti-Oups! request's (bytes).
pub(crate) const MAX_ID: usize = 256;
/// The longest `contexte` or `nouvelleChaine` (bytes): Antidote replaces words and sentences.
const MAX_EDIT: usize = 1024 * 1024;
/// The longest correction memory (bytes of base64).
const MAX_MEMORY: usize = 4 * 1024 * 1024;
/// The largest position: no zone text is longer than a message.
const MAX_POSITION: u64 = MAX_MESSAGE as u64;
/// The longest document title or path given to Antidote (bytes).
pub(crate) const MAX_TITLE: usize = 4096;
const MAX_DETECTIONS: usize = 32;

/// An Antidote tool the editor can open on its text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    /// The corrector, on the zones the editor gives.
    Corrector,
    /// The dictionaries, on the selection (Antidote can then replace a word).
    Dictionaries,
    /// The guides, on the selection.
    Guides,
}

impl Tool {
    fn wire(self) -> &'static str {
        match self {
            Tool::Corrector => "Correcteur",
            Tool::Dictionaries => "Dictionnaires",
            Tool::Guides => "Guides",
        }
    }
}

/// The markup of the text, which Antidote filters out to correct only the words (`filtreActif`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentType {
    Text,
    Markdown,
    Latex,
    SubRip,
    Html,
}

impl DocumentType {
    fn wire(self) -> &'static str {
        match self {
            DocumentType::Text => "texte",
            DocumentType::Markdown => "markdown",
            DocumentType::Latex => "latex",
            DocumentType::SubRip => "subrip",
            DocumentType::Html => "html",
        }
    }
}

/// A style on a range of a zone's text (`infoStyle`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StyleKind {
    Bold,
    Italic,
    Superscript,
    Subscript,
    Strikethrough,
}

impl StyleKind {
    pub(crate) fn wire(self) -> &'static str {
        match self {
            StyleKind::Bold => "gras",
            StyleKind::Italic => "italique",
            StyleKind::Superscript => "exposant",
            StyleKind::Subscript => "indice",
            StyleKind::Strikethrough => "barre",
        }
    }
}

/// The editor's answer to `init` (the Python's `ConfigurationTexteur`). `None` leaves a setting
/// out, and Antidote uses its own default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Configuration {
    /// Shown in the title bar of Antidote's corrector (`titreDocument`).
    pub document_title: Option<String>,
    /// The editor's line break: `"\n"`, `"\r"` or `"\r\n"` (`retourChariot`); anything else is
    /// left out.
    pub newline: Option<String>,
    /// Antidote keeps the state of the correction by document path (`typeIdentifiantCache:
    /// forceEmplacementFichier`).
    pub cache_by_document_path: bool,
    /// Whether the corrector may insert line breaks (`permetRetourChariot`).
    pub allow_newlines: Option<bool>,
    /// Whether the editor takes no-break spaces (`permetEspaceInsecable`).
    pub allow_no_break_space: Option<bool>,
    /// Whether the editor takes thin spaces (`permetEspaceFine`).
    pub allow_thin_space: Option<bool>,
    /// Whether Antidote may ask to send the message (`permetEnvoiCourriel`, for Anti-Oups!). Only
    /// when this is `Some(true)` does an `envoi` from Antidote reach [`crate::Texteur::send`].
    pub allow_send: Option<bool>,
    /// Whether Antidote may replace text without selecting it first (`remplaceSansSelection`).
    pub replace_without_selection: Option<bool>,
    /// The correction memory Antidote gave last time for this document (base64, opaque).
    pub correction_memory: Option<String>,
    /// The markup of the text (`filtreActif`).
    pub document_type: Option<DocumentType>,
}

/// What Anti-Oups! found in a message, one kind per detection (`detections`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detection {
    /// `erreurs`: spelling or grammar errors.
    Errors,
    /// `typographie`.
    Typography,
    /// `piecesJointes`: an attachment mentioned but missing.
    Attachments,
    /// `langueInactive`: text in a language the corrector does not check.
    InactiveLanguage,
    /// `tonVexant`: a tone that may hurt.
    HurtfulTone,
    /// A kind this version of the protocol does not name.
    Other,
}

/// Antidote's answer to an Anti-Oups! request (`reponseAntiOups`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AntiOupsReply {
    /// The id [`crate::Session::anti_oups`] returned.
    pub id: String,
    /// Set when Antidote could not check the message (`codeErreur`).
    pub error_code: Option<String>,
    pub detections: Vec<Detection>,
}

impl AntiOupsReply {
    /// Whether to open the corrector on the message, as Druide's example decides: no error, and
    /// at least one detection.
    pub fn should_open_corrector(&self) -> bool {
        self.error_code.is_none() && !self.detections.is_empty()
    }
}

// ---------------------------------------------------------------------------------------------
// Packets.

#[derive(Serialize)]
struct OutPacket<'a> {
    #[serde(rename = "idPaquet")]
    id: u32,
    #[serde(rename = "totalPaquet")]
    total: u32,
    donnees: &'a str,
}

/// `message` serialized and wrapped in a single packet, as the Python sends every message; `None`
/// (logged) if it cannot be serialized or is larger than [`MAX_MESSAGE`].
fn to_packet<T: Serialize>(message: &T) -> Option<String> {
    let packet = serde_json::to_string(message)
        .and_then(|json| serde_json::to_string(&OutPacket { id: 0, total: 1, donnees: &json }));
    match packet {
        Ok(packet) if packet.len() <= MAX_MESSAGE => Some(packet),
        Ok(packet) => {
            log::warn!("Antidote: a message of {} bytes was not sent (too large)", packet.len());
            None
        }
        Err(e) => {
            log::warn!("Antidote: a message could not be written: {e}");
            None
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InPacket {
    id_paquet: u64,
    total_paquet: u64,
    donnees: String,
}

/// Joins the packets of Antidote's messages.
///
/// The README numbers a message's packets `0..total`, the Python's `liste[id_paquet - 1]` reads
/// them as `1..=total`: either numbering is accepted, not a mix. A message's packets come one
/// after the other (README); a packet of another message, a duplicate or a change of total is an
/// error, which ends the session.
#[derive(Default)]
pub(crate) struct Reassembler {
    total: usize,
    parts: BTreeMap<usize, String>,
    bytes: usize,
}

impl Reassembler {
    /// Takes one WebSocket message; the whole message once its last packet is in.
    pub(crate) fn push(&mut self, frame: &str) -> Result<Option<String>, Error> {
        let result = self.add(frame);
        if result.is_err() {
            *self = Self::default();
        }
        result
    }

    fn add(&mut self, frame: &str) -> Result<Option<String>, Error> {
        let packet: InPacket =
            serde_json::from_str(frame).map_err(|e| Error::Protocol(format!("not a packet: {e}")))?;
        let total = usize::try_from(packet.total_paquet)
            .ok()
            .filter(|t| (1..=MAX_PACKETS).contains(t))
            .ok_or_else(|| Error::Protocol(format!("{} packets in a message", packet.total_paquet)))?;
        let id = usize::try_from(packet.id_paquet)
            .ok()
            .filter(|i| *i <= total)
            .ok_or_else(|| Error::Protocol(format!("packet {} of {total}", packet.id_paquet)))?;
        if self.parts.is_empty() {
            self.total = total;
            self.bytes = 0;
        } else if total != self.total {
            return Err(Error::Protocol("the packet count changed within a message".into()));
        }
        self.bytes = self.bytes.saturating_add(packet.donnees.len());
        if self.bytes > MAX_MESSAGE {
            return Err(Error::Protocol(format!("a message of more than {MAX_MESSAGE} bytes")));
        }
        if self.parts.insert(id, packet.donnees).is_some() {
            return Err(Error::Protocol(format!("packet {id} came twice")));
        }
        if self.parts.len() < total {
            return Ok(None);
        }
        let parts = std::mem::take(&mut self.parts);
        // `total` distinct ids in 0..=total: they are 0..total or 1..=total when the first and the
        // last say so.
        let first = parts.keys().next().copied();
        let last = parts.keys().next_back().copied();
        let numbered = (first == Some(0) && last == Some(total - 1)) || (first == Some(1) && last == Some(total));
        if !numbered {
            return Err(Error::Protocol("packets numbered neither from 0 nor from 1".into()));
        }
        Ok(Some(parts.into_values().collect()))
    }
}

// ---------------------------------------------------------------------------------------------
// From MailALM to Antidote.

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LaunchTool {
    outil_api: &'static str,
    message: &'static str,
    version_api: u32,
}

/// `LanceOutil` (the README says `LanceOutils`; the Python, which Druide tests against Antidote,
/// sends `LanceOutil`).
pub(crate) fn launch(tool: Tool) -> Option<String> {
    to_packet(&LaunchTool { outil_api: tool.wire(), message: "LanceOutil", version_api: API_VERSION })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AntiOups<'a> {
    sujet: &'a str,
    corps: &'a str,
    destinataires: &'a [String],
    id: &'a str,
    message: &'static str,
    version_api: u32,
}

/// `AntiOups`: the subject, body and recipients of the message to check.
pub(crate) fn anti_oups(subject: &str, body: &str, recipients: &[String], id: &str) -> Option<String> {
    to_packet(&AntiOups {
        sujet: subject,
        corps: body,
        destinataires: recipients,
        id,
        message: "AntiOups",
        version_api: API_VERSION,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Reply<'a, T> {
    #[serde(skip_serializing_if = "Option::is_none")]
    id_message: Option<&'a str>,
    donnees: T,
}

/// The answer to `docEstDisponible`, `editionPossible` or `remplace`.
pub(crate) fn reply_bool(id: Option<&str>, value: bool) -> Option<String> {
    to_packet(&Reply { id_message: id, donnees: value })
}

/// The answer to `cheminDocument`.
pub(crate) fn reply_text(id: Option<&str>, value: &str) -> Option<String> {
    to_packet(&Reply { id_message: id, donnees: value })
}

/// A zone of text as Antidote reads it, positions in its unit.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireZone<'a> {
    pub(crate) texte: &'a str,
    pub(crate) id_zone: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) position_selection_debut: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) position_selection_fin: Option<usize>,
    pub(crate) zone_est_en_focus: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) info_style: Vec<WireStyle>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireStyle {
    pub(crate) position_debut: usize,
    pub(crate) position_fin: usize,
    pub(crate) style: &'static str,
}

/// The answer to `donneZonesTexte`.
pub(crate) fn reply_zones(id: Option<&str>, zones: &[WireZone<'_>]) -> Option<String> {
    to_packet(&Reply { id_message: id, donnees: zones })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InitReply<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    id_message: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    titre_document: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    retour_chariot: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    type_identifiant_cache: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    permet_retour_chariot: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    permet_espace_insecable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    permet_espace_fine: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    permet_envoi_courriel: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    remplace_sans_selection: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    memoire_correction: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    filtre_actif: Option<&'static str>,
}

/// The answer to `init`: the editor's configuration, less what is not valid (a title with control
/// characters, an unknown line break, a memory that is not base64).
pub(crate) fn reply_init(id: Option<&str>, configuration: &Configuration) -> Option<String> {
    let c = configuration;
    let title = c.document_title.as_deref().filter(|t| is_plain(t, MAX_TITLE));
    let newline = c.newline.as_deref().filter(|n| matches!(*n, "\n" | "\r" | "\r\n"));
    let memory = c.correction_memory.as_deref().filter(|m| m.len() <= MAX_MEMORY && base64::is_base64_text(m));
    to_packet(&InitReply {
        id_message: id,
        titre_document: title,
        retour_chariot: newline,
        type_identifiant_cache: c.cache_by_document_path.then_some("forceEmplacementFichier"),
        permet_retour_chariot: c.allow_newlines,
        permet_espace_insecable: c.allow_no_break_space,
        permet_espace_fine: c.allow_thin_space,
        permet_envoi_courriel: c.allow_send,
        remplace_sans_selection: c.replace_without_selection,
        memoire_correction: memory,
        filtre_actif: c.document_type.map(DocumentType::wire),
    })
}

// ---------------------------------------------------------------------------------------------
// From Antidote to MailALM.

/// A request from Antidote, read and checked (positions still in Antidote's unit).
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Request {
    /// `idMessage`, given back in the answer.
    pub(crate) id: Option<String>,
    pub(crate) kind: RequestKind,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RequestKind {
    /// `init`: answer the configuration.
    Init,
    /// `cheminDocument`: answer the document's path.
    DocumentPath,
    /// `docEstDisponible`: answer whether the text can still be corrected.
    DocumentAvailable,
    /// `donneZonesTexte`: answer the zones of text (only the one with the selection, if
    /// `for_selection`).
    Zones { for_selection: bool },
    /// `editionPossible`: answer whether `context` is still at `start..end` of `zone`.
    CanEdit { zone: String, context: String, start: usize, end: usize },
    /// `remplace`: replace `start..end` of `zone` with `text`, and answer whether it was done.
    Replace { zone: String, text: String, start: usize, end: usize },
    /// `selectionne`: select `start..end` of `zone`.
    Select { zone: String, start: usize, end: usize },
    /// `retourneAuDocument`: bring the editor to the front.
    ReturnToDocument,
    /// `nouvelleMemoireCorrection`: keep this state of the correction (base64).
    NewCorrectionMemory(String),
    /// `envoi`: send the message.
    Send,
    /// `reponseAntiOups`.
    AntiOupsReply(AntiOupsReply),
}

/// What to do with a message from Antidote.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Parsed {
    Request(Request),
    /// A request that wants a yes or no but cannot be read: answer no.
    Refuse { id: Option<String>, why: String },
    /// Not a request MailALM can act on: log it and go on.
    Ignore(String),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawRequest {
    message: String,
    #[serde(default)]
    id_message: Option<String>,
    #[serde(default)]
    donnees: Option<Value>,
    /// Antidote 11 v4 puts it here instead of in `donnees` (requete_entrante.py).
    #[serde(default)]
    pour_selection_active: Option<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EditParams {
    id_zone: String,
    contexte: String,
    position_debut: u64,
    position_fin: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReplaceParams {
    id_zone: String,
    nouvelle_chaine: String,
    position_remplacement_debut: u64,
    position_remplacement_fin: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SelectParams {
    id_zone: String,
    position_debut: u64,
    position_fin: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AntiOupsParams {
    id: String,
    #[serde(default)]
    code_erreur: Option<Value>,
    #[serde(default)]
    detections: Option<Vec<String>>,
}

/// Reads one whole message from Antidote.
pub(crate) fn parse_request(message: &str) -> Parsed {
    let raw: RawRequest = match serde_json::from_str(message) {
        Ok(raw) => raw,
        Err(e) => return Parsed::Ignore(format!("not a request: {e}")),
    };
    if let Some(id) = &raw.id_message
        && !is_plain(id, MAX_ID)
    {
        return Parsed::Ignore("a message id that is too long or holds control characters".into());
    }
    let id = raw.id_message;
    let kind = match raw.message.as_str() {
        "init" => RequestKind::Init,
        "cheminDocument" => RequestKind::DocumentPath,
        "docEstDisponible" => RequestKind::DocumentAvailable,
        "donneZonesTexte" => {
            // As the Python: `donnees` if there is any, else the field Antidote 11 v4 sends.
            let flag = match &raw.donnees {
                Some(donnees) => donnees.get("pourSelectionActive"),
                None => raw.pour_selection_active.as_ref(),
            };
            RequestKind::Zones { for_selection: flag.and_then(Value::as_bool).unwrap_or(false) }
        }
        "editionPossible" => match params::<EditParams>(raw.donnees).and_then(|p| {
            let (start, end) = range(p.position_debut, p.position_fin)?;
            Ok(RequestKind::CanEdit { zone: zone_id(p.id_zone)?, context: sized(p.contexte, "contexte")?, start, end })
        }) {
            Ok(kind) => kind,
            Err(why) => return Parsed::Refuse { id, why },
        },
        "remplace" => match params::<ReplaceParams>(raw.donnees).and_then(|p| {
            let (start, end) = range(p.position_remplacement_debut, p.position_remplacement_fin)?;
            Ok(RequestKind::Replace { zone: zone_id(p.id_zone)?, text: insertable(p.nouvelle_chaine)?, start, end })
        }) {
            Ok(kind) => kind,
            Err(why) => return Parsed::Refuse { id, why },
        },
        "selectionne" => match params::<SelectParams>(raw.donnees).and_then(|p| {
            let (start, end) = range(p.position_debut, p.position_fin)?;
            Ok(RequestKind::Select { zone: zone_id(p.id_zone)?, start, end })
        }) {
            Ok(kind) => kind,
            Err(why) => return Parsed::Ignore(why),
        },
        "retourneAuDocument" => RequestKind::ReturnToDocument,
        "nouvelleMemoireCorrection" => match memory(raw.donnees) {
            Ok(memory) => RequestKind::NewCorrectionMemory(memory),
            Err(why) => return Parsed::Ignore(why),
        },
        "envoi" => RequestKind::Send,
        "reponseAntiOups" => match params::<AntiOupsParams>(raw.donnees).and_then(anti_oups_reply) {
            Ok(reply) => RequestKind::AntiOupsReply(reply),
            Err(why) => return Parsed::Ignore(why),
        },
        other => return Parsed::Ignore(format!("unknown message \"{}\"", clip(other))),
    };
    Parsed::Request(Request { id, kind })
}

fn params<T: DeserializeOwned>(donnees: Option<Value>) -> Result<T, String> {
    serde_json::from_value(donnees.unwrap_or(Value::Null)).map_err(|e| format!("bad parameters: {e}"))
}

/// Whether `text` is at most `max` bytes and holds no control character: ids, titles, paths.
pub(crate) fn is_plain(text: &str, max: usize) -> bool {
    text.len() <= max && !text.chars().any(char::is_control)
}

fn zone_id(id: String) -> Result<String, String> {
    if is_plain(&id, MAX_ID) { Ok(id) } else { Err("a zone id that is too long or holds control characters".into()) }
}

fn position(p: u64) -> Result<usize, String> {
    usize::try_from(p).ok().filter(|_| p <= MAX_POSITION).ok_or_else(|| format!("position {p} out of bounds"))
}

fn range(start: u64, end: u64) -> Result<(usize, usize), String> {
    let (start, end) = (position(start)?, position(end)?);
    if start > end {
        return Err(format!("range {start}..{end} ends before it starts"));
    }
    Ok((start, end))
}

fn sized(text: String, what: &str) -> Result<String, String> {
    if text.len() > MAX_EDIT {
        return Err(format!("{what} of {} bytes", text.len()));
    }
    Ok(text)
}

/// Characters Antidote may not put into the text: control characters other than tab and line
/// breaks, and the invisible marks that reorder text (bidi controls).
fn forbidden(c: char) -> bool {
    (c.is_control() && !matches!(c, '\t' | '\n' | '\r'))
        || matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// The text of a replacement: sized, and free of [`forbidden`] characters.
fn insertable(text: String) -> Result<String, String> {
    let text = sized(text, "nouvelleChaine")?;
    if let Some(c) = text.chars().find(|c| forbidden(*c)) {
        return Err(format!("a replacement holding U+{:04X}", u32::from(c)));
    }
    Ok(text)
}

/// The memory of `nouvelleMemoireCorrection`: a base64 string, in `donnees` (README) or in
/// `donnees.donnees` (the Python's `ParamsNouvelleMemoireCorrection`).
fn memory(donnees: Option<Value>) -> Result<String, String> {
    let memory = match donnees {
        Some(Value::String(memory)) => memory,
        Some(Value::Object(mut fields)) => match fields.remove("donnees") {
            Some(Value::String(memory)) => memory,
            _ => return Err("a correction memory that is not a string".into()),
        },
        _ => return Err("a correction memory that is not a string".into()),
    };
    if memory.len() > MAX_MEMORY || !base64::is_base64_text(&memory) {
        return Err("a correction memory that is too large or not base64".into());
    }
    Ok(memory)
}

fn anti_oups_reply(p: AntiOupsParams) -> Result<AntiOupsReply, String> {
    if !is_plain(&p.id, MAX_ID) {
        return Err("an Anti-Oups! id that is too long or holds control characters".into());
    }
    // A string in the Python, a number in the README: either is kept as text.
    let error_code = match p.code_erreur {
        None => None,
        Some(Value::String(code)) if is_plain(&code, MAX_ID) => Some(code),
        Some(Value::Number(code)) => Some(code.to_string()),
        Some(_) => return Err("an Anti-Oups! error code that is not a short string or a number".into()),
    };
    let detections = p.detections.unwrap_or_default();
    if detections.len() > MAX_DETECTIONS {
        return Err(format!("{} Anti-Oups! detections", detections.len()));
    }
    let detections = detections
        .iter()
        .map(|d| match d.as_str() {
            "erreurs" => Detection::Errors,
            "typographie" => Detection::Typography,
            "piecesJointes" => Detection::Attachments,
            "langueInactive" => Detection::InactiveLanguage,
            "tonVexant" => Detection::HurtfulTone,
            _ => Detection::Other,
        })
        .collect();
    Ok(AntiOupsReply { id: p.id, error_code, detections })
}

#[cfg(test)]
mod tests;
