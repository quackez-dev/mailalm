// Protocol ported from Druide's antidote-api-python 1.1.0 (MIT, © 2026 Druide informatique inc.).
// From: agent_connectix.py (the conversation) and agent_texteur.py (the editor's side), rewritten as
// a background thread and a trait the UI drives through channels. The checks on what Antidote asks
// (known zones, positions, sending only when allowed, Anti-Oups! ids) are MailALM's.

//! A conversation with Antidote, and the editor's side of it.

use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt::Write as _;
use std::ops::Range;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;

use ring::rand::{SecureRandom, SystemRandom};

use crate::offsets::{self, Unit};
use crate::protocol::{
    self, AntiOupsReply, Configuration, MAX_ID, MAX_TITLE, Parsed, Reassembler, Request, RequestKind, StyleKind, Tool,
    WireStyle, WireZone,
};
use crate::ws::{Incoming, WebSocket};
use crate::{Error, locate};

/// The most zones given to Antidote at once.
const MAX_ZONES: usize = 64;
/// The longest zone text given to Antidote (bytes); a longer zone is left out.
const MAX_ZONE_TEXT: usize = 4 * 1024 * 1024;

/// A zone of text the editor gives Antidote (the Python's `ZoneDeTexte`). Ranges are byte offsets
/// into `text`, on character boundaries; the bridge converts them to Antidote's [`Unit`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Zone {
    /// Unique among the zones given; Antidote names the zone with it in its requests. One zone
    /// may have an empty id.
    pub id: String,
    pub text: String,
    /// The selection, if any.
    pub selection: Option<Range<usize>>,
    /// Whether this zone has the keyboard focus.
    pub focused: bool,
    /// Styles on ranges of the text, so that Antidote knows the text's look.
    pub styles: Vec<Style>,
}

/// A style on a byte range of a [`Zone`]'s text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Style {
    pub range: Range<usize>,
    pub kind: StyleKind,
}

/// The editor's side of the conversation (the Python's `AgentTexteur`), implemented by the UI on
/// its compose buffer and called only from [`Session::process`], on the UI's thread.
///
/// Every request reaches it checked: the zone is one the editor gave in its last [`zones`], the
/// range is within that zone's current text and on character boundaries, a replacement holds no
/// control character (other than tab and line breaks) and no bidi control, and is at most 1 MiB.
///
/// [`zones`]: Texteur::zones
pub trait Texteur {
    /// The configuration Antidote asks for when it starts (`init`).
    fn configuration(&self) -> Configuration;

    /// The zones of text to correct (`donneZonesTexte`); only the one holding the selection when
    /// `for_selection` (the dictionaries and guides ask so).
    fn zones(&self, for_selection: bool) -> Vec<Zone>;

    /// The current text of the zone `id`, to check Antidote's positions against.
    fn zone_text(&self, id: &str) -> Option<Cow<'_, str>>;

    /// Replaces the byte range `range` of zone `id` with `text` (`remplace`); whether it was done.
    /// This is how Antidote's corrections, and the dictionaries' "replace", change the text.
    fn replace(&mut self, id: &str, range: Range<usize>, text: &str) -> bool;

    /// Selects the byte range `range` of zone `id` (`selectionne`), to show what Antidote is
    /// about to correct.
    fn select(&mut self, _id: &str, _range: Range<usize>) {}

    /// The document's path (`cheminDocument`), with which Antidote keeps the state of the
    /// correction; empty for none.
    fn document_path(&self) -> String {
        String::new()
    }

    /// Whether the text can still be corrected (`docEstDisponible`): false once the message is
    /// closed or sent.
    fn document_available(&self) -> bool {
        true
    }

    /// Antidote asks to bring the editor to the front (`retourneAuDocument`).
    fn return_to_document(&mut self) {}

    /// A new state of the correction (`nouvelleMemoireCorrection`), base64, to give back in
    /// [`Configuration::correction_memory`] next time.
    fn new_correction_memory(&mut self, _memory: String) {}

    /// Antidote asks to send the message (`envoi`). Called only when the last configuration had
    /// `allow_send: Some(true)`.
    fn send(&mut self) {}

    /// The answer to [`Session::anti_oups`] (`reponseAntiOups`); only for an id it returned.
    fn anti_oups_reply(&mut self, _reply: AntiOupsReply) {}

    /// Antidote starts working on the text (`init`).
    fn session_started(&mut self) {}

    /// The conversation is over: Antidote closed it (its window was closed), MailALM did, or
    /// `error` happened. Called once.
    fn session_ended(&mut self, _error: Option<&Error>) {}
}

enum Command {
    /// A packet to send as it is.
    Send(String),
    Close,
}

enum Event {
    Request(Request),
    Ended(Option<Error>),
}

/// A conversation with Antidote (the Python's `AgentConnectix`), on a thread of its own: it finds
/// Antidote, connects, sends what the UI asks, and queues Antidote's requests for
/// [`Session::process`]. Nothing here blocks the UI.
///
/// Calls made before the connection is up wait for it. Dropping the session closes it.
pub struct Session {
    commands: Sender<Command>,
    events: Receiver<Event>,
    unit: Unit,
    /// The zone ids given to Antidote in the last `donneZonesTexte` answer.
    zones: HashSet<String>,
    /// Whether the last configuration allowed `envoi`.
    allow_send: bool,
    /// The Anti-Oups! requests not answered yet.
    pending_anti_oups: HashSet<String>,
    ended: bool,
    rng: SystemRandom,
}

impl Session {
    /// Starts a session: finds Antidote and connects to it, in the background. `unit` is what
    /// Antidote's positions count ([`Unit::CodePoints`] as in Druide's package). `wake` is called
    /// from the session's thread whenever there is something for [`Session::process`] (for egui:
    /// `ctx.request_repaint()`).
    pub fn start(unit: Unit, wake: impl Fn() + Send + 'static) -> Self {
        Self::start_with(unit, locate::find_port, wake)
    }

    fn start_with(
        unit: Unit,
        find_port: impl FnOnce() -> Result<u16, Error> + Send + 'static,
        wake: impl Fn() + Send + 'static,
    ) -> Self {
        let (commands, command_rx) = mpsc::channel();
        let (event_tx, events) = mpsc::channel();
        let failed = event_tx.clone();
        let spawned = thread::Builder::new().name("antidote".into()).spawn(move || {
            let result = converse(find_port, &command_rx, &event_tx, &wake);
            if let Err(e) = &result {
                log::warn!("Antidote: {e}");
            }
            let _ = event_tx.send(Event::Ended(result.err()));
            wake();
        });
        if let Err(e) = spawned {
            let _ = failed.send(Event::Ended(Some(Error::Io(format!("cannot start a thread: {e}")))));
        }
        Self {
            commands,
            events,
            unit,
            zones: HashSet::new(),
            allow_send: false,
            pending_anti_oups: HashSet::new(),
            ended: false,
            rng: SystemRandom::new(),
        }
    }

    /// Opens an Antidote tool on the editor's text (`LanceOutil`). Antidote then asks for the
    /// configuration and the zones through [`Session::process`].
    pub fn launch(&self, tool: Tool) {
        if let Some(packet) = protocol::launch(tool) {
            self.send_packet(packet);
        }
    }

    /// Asks Anti-Oups! to check a message before it is sent (`AntiOups`): its subject, body and
    /// recipients go to Antidote. Returns the request's id, which the answer
    /// ([`Texteur::anti_oups_reply`]) carries; `None` if it could not be sent.
    pub fn anti_oups(&mut self, subject: &str, body: &str, recipients: &[String]) -> Option<String> {
        let id = self.new_id()?;
        let packet = protocol::anti_oups(subject, body, recipients, &id)?;
        self.pending_anti_oups.insert(id.clone());
        self.send_packet(packet);
        Some(id)
    }

    /// Ends the conversation; [`Texteur::session_ended`] follows through [`Session::process`].
    pub fn close(&self) {
        let _ = self.commands.send(Command::Close);
    }

    /// Whether the conversation is over (after [`Texteur::session_ended`]).
    pub fn is_ended(&self) -> bool {
        self.ended
    }

    /// Handles what Antidote asked since the last call, calling `texteur` and answering. Never
    /// waits; call it when `wake` was called (calling it more often is harmless).
    pub fn process(&mut self, texteur: &mut dyn Texteur) {
        if self.ended {
            return;
        }
        loop {
            match self.events.try_recv() {
                Ok(Event::Request(request)) => self.answer(request, texteur),
                Ok(Event::Ended(error)) => return self.end(texteur, error),
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    return self.end(texteur, Some(Error::Io("the session's thread stopped".into())));
                }
            }
        }
    }

    fn end(&mut self, texteur: &mut dyn Texteur, error: Option<Error>) {
        self.ended = true;
        self.zones.clear();
        self.pending_anti_oups.clear();
        texteur.session_ended(error.as_ref());
    }

    fn send_packet(&self, packet: String) {
        if self.commands.send(Command::Send(packet)).is_err() {
            log::debug!("Antidote: the session is over, a message was dropped");
        }
    }

    fn answer(&mut self, request: Request, texteur: &mut dyn Texteur) {
        let id = request.id.as_deref();
        let reply = match request.kind {
            RequestKind::Init => {
                texteur.session_started();
                let configuration = texteur.configuration();
                self.allow_send = configuration.allow_send == Some(true);
                protocol::reply_init(id, &configuration)
            }
            RequestKind::DocumentPath => {
                let path = texteur.document_path();
                let path = if protocol::is_plain(&path, MAX_TITLE) { path } else { String::new() };
                protocol::reply_text(id, &path)
            }
            RequestKind::DocumentAvailable => protocol::reply_bool(id, texteur.document_available()),
            RequestKind::Zones { for_selection } => {
                let zones = texteur.zones(for_selection);
                self.zones_reply(id, &zones)
            }
            RequestKind::CanEdit { zone, context, start, end } => {
                let still_there = self
                    .range_in(&*texteur, &zone, start, end)
                    .is_some_and(|(text, range)| text.get(range) == Some(context.as_str()));
                protocol::reply_bool(id, still_there)
            }
            RequestKind::Replace { zone, text, start, end } => {
                let range = self.range_in(&*texteur, &zone, start, end).map(|(_, range)| range);
                let done = match range {
                    Some(range) => texteur.replace(&zone, range, &text),
                    None => false,
                };
                protocol::reply_bool(id, done)
            }
            RequestKind::Select { zone, start, end } => {
                let range = self.range_in(&*texteur, &zone, start, end).map(|(_, range)| range);
                if let Some(range) = range {
                    texteur.select(&zone, range);
                }
                None
            }
            RequestKind::ReturnToDocument => {
                texteur.return_to_document();
                None
            }
            RequestKind::NewCorrectionMemory(memory) => {
                texteur.new_correction_memory(memory);
                None
            }
            RequestKind::Send => {
                if self.allow_send {
                    texteur.send();
                } else {
                    log::warn!("Antidote: asked to send, which the configuration did not allow");
                }
                None
            }
            RequestKind::AntiOupsReply(reply) => {
                if self.pending_anti_oups.remove(&reply.id) {
                    texteur.anti_oups_reply(reply);
                } else {
                    log::warn!("Antidote: an Anti-Oups! answer to no request of this session");
                }
                None
            }
        };
        if let Some(packet) = reply {
            self.send_packet(packet);
        }
    }

    /// The text of `zone` and the byte range of Antidote's `start..end` in it, if `zone` was given
    /// to Antidote and the range is valid in its current text.
    fn range_in<'t>(
        &self,
        texteur: &'t dyn Texteur,
        zone: &str,
        start: usize,
        end: usize,
    ) -> Option<(Cow<'t, str>, Range<usize>)> {
        if !self.zones.contains(zone) {
            log::warn!("Antidote: a request on a zone it was not given");
            return None;
        }
        let text = texteur.zone_text(zone)?;
        let Some(range) = offsets::to_byte_range(&text, start, end, self.unit) else {
            log::warn!("Antidote: a range outside the zone's text, or inside a character");
            return None;
        };
        Some((text, range))
    }

    /// The answer to `donneZonesTexte`, remembering which zones Antidote was given.
    fn zones_reply(&mut self, id: Option<&str>, zones: &[Zone]) -> Option<String> {
        self.zones.clear();
        let mut wire = Vec::with_capacity(zones.len().min(MAX_ZONES));
        for zone in zones.iter().take(MAX_ZONES) {
            match wire_zone(zone, self.unit) {
                Some(w) if self.zones.insert(zone.id.clone()) => wire.push(w),
                _ => log::warn!("Antidote: a zone was left out (bad or repeated id, or too long)"),
            }
        }
        protocol::reply_zones(id, &wire).or_else(|| {
            self.zones.clear();
            protocol::reply_zones(id, &[])
        })
    }

    /// A random id in the form of a UUID v4, as the Python's `uuid4()`.
    fn new_id(&self) -> Option<String> {
        let mut bytes = [0u8; 16];
        self.rng.fill(&mut bytes).ok()?;
        bytes[6] = (bytes[6] & 0x0F) | 0x40;
        bytes[8] = (bytes[8] & 0x3F) | 0x80;
        let mut id = String::with_capacity(36);
        for (i, byte) in bytes.iter().enumerate() {
            if matches!(i, 4 | 6 | 8 | 10) {
                id.push('-');
            }
            let _ = write!(id, "{byte:02x}");
        }
        Some(id)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Close);
    }
}

/// `zone` as Antidote reads it, positions in `unit`; `None` if its id or size is not acceptable.
/// A selection or a style that is not a valid range of the text is left out.
fn wire_zone(zone: &Zone, unit: Unit) -> Option<WireZone<'_>> {
    if !protocol::is_plain(&zone.id, MAX_ID) || zone.text.len() > MAX_ZONE_TEXT {
        return None;
    }
    let selection = zone.selection.as_ref().and_then(|range| offsets::from_byte_range(&zone.text, range, unit));
    let styles = zone
        .styles
        .iter()
        .filter_map(|style| {
            let (start, end) = offsets::from_byte_range(&zone.text, &style.range, unit)?;
            Some(WireStyle { position_debut: start, position_fin: end, style: style.kind.wire() })
        })
        .collect();
    Some(WireZone {
        texte: &zone.text,
        id_zone: &zone.id,
        position_selection_debut: selection.map(|(start, _)| start),
        position_selection_fin: selection.map(|(_, end)| end),
        zone_est_en_focus: zone.focused,
        info_style: styles,
    })
}

/// The session's thread: finds the port, connects, then relays until either side closes.
fn converse(
    find_port: impl FnOnce() -> Result<u16, Error>,
    commands: &Receiver<Command>,
    events: &Sender<Event>,
    wake: &dyn Fn(),
) -> Result<(), Error> {
    let port = find_port()?;
    let mut ws = WebSocket::connect(port)?;
    let mut packets = Reassembler::default();
    loop {
        loop {
            match commands.try_recv() {
                Ok(Command::Send(packet)) => ws.send_text(&packet)?,
                Ok(Command::Close) | Err(TryRecvError::Disconnected) => {
                    ws.close();
                    return Ok(());
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        let text = match ws.poll()? {
            None => continue,
            Some(Incoming::Closed) => return Ok(()),
            Some(Incoming::Message(text)) => text,
        };
        let Some(message) = packets.push(&text)? else { continue };
        match protocol::parse_request(&message) {
            Parsed::Request(request) => {
                if events.send(Event::Request(request)).is_err() {
                    ws.close();
                    return Ok(());
                }
                wake();
            }
            Parsed::Refuse { id, why } => {
                log::warn!("Antidote: a request refused: {why}");
                if let Some(packet) = protocol::reply_bool(id.as_deref(), false) {
                    ws.send_text(&packet)?;
                }
            }
            Parsed::Ignore(why) => log::warn!("Antidote: a message ignored: {why}"),
        }
    }
}

#[cfg(test)]
mod tests;
