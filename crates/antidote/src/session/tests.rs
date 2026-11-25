use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::*;
use crate::protocol::{Detection, DocumentType};
use crate::test_server::{FakeAntidote, listen, server_frame};

/// "Été 🍎 je manges": "manges" is bytes 14..20, code points 9..15, UTF-16 units 10..16.
const BODY: &str = "Été 🍎 je manges";

/// A compose window: one zone, "corps".
#[derive(Default)]
struct Editor {
    body: String,
    selection: Option<Range<usize>>,
    allow_send: bool,
    started: usize,
    ended: Option<Option<String>>,
    sent: usize,
    returned: usize,
    memory: Option<String>,
    replies: Vec<AntiOupsReply>,
}

impl Editor {
    fn new(allow_send: bool) -> Self {
        Self { body: BODY.into(), selection: Some(14..20), allow_send, ..Self::default() }
    }
}

impl Texteur for Editor {
    fn configuration(&self) -> Configuration {
        Configuration {
            document_title: Some("Message".into()),
            newline: Some("\n".into()),
            allow_send: self.allow_send.then_some(true),
            document_type: Some(DocumentType::Text),
            ..Configuration::default()
        }
    }

    fn zones(&self, _for_selection: bool) -> Vec<Zone> {
        vec![Zone {
            id: "corps".into(),
            text: self.body.clone(),
            selection: self.selection.clone(),
            focused: true,
            styles: vec![Style { range: 0..5, kind: StyleKind::Bold }],
        }]
    }

    fn zone_text(&self, id: &str) -> Option<Cow<'_, str>> {
        (id == "corps").then(|| Cow::Borrowed(self.body.as_str()))
    }

    fn replace(&mut self, id: &str, range: Range<usize>, text: &str) -> bool {
        assert_eq!(id, "corps");
        self.body.replace_range(range, text);
        true
    }

    fn select(&mut self, id: &str, range: Range<usize>) {
        assert_eq!(id, "corps");
        self.selection = Some(range);
    }

    fn document_path(&self) -> String {
        "brouillon-12".into()
    }

    fn return_to_document(&mut self) {
        self.returned += 1;
    }

    fn new_correction_memory(&mut self, memory: String) {
        self.memory = Some(memory);
    }

    fn send(&mut self) {
        self.sent += 1;
    }

    fn anti_oups_reply(&mut self, reply: AntiOupsReply) {
        self.replies.push(reply);
    }

    fn session_started(&mut self) {
        self.started += 1;
    }

    fn session_ended(&mut self, error: Option<&Error>) {
        assert!(self.ended.is_none(), "ended twice");
        self.ended = Some(error.map(Error::to_string));
    }
}

/// Runs the UI's side until the session ends, as egui would on each wake.
fn run(session: &mut Session, editor: &mut Editor) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while editor.ended.is_none() {
        assert!(Instant::now() < deadline, "the session did not end");
        session.process(editor);
        thread::sleep(Duration::from_millis(5));
    }
    assert!(session.is_ended());
}

fn edit(id: &str, zone: &str, context: &str, start: i64, end: i64) -> Value {
    json!({ "message": "editionPossible", "idMessage": id,
            "donnees": { "idZone": zone, "contexte": context, "positionDebut": start, "positionFin": end } })
}

fn replacement(id: &str, zone: &str, text: &str, start: i64, end: i64) -> Value {
    json!({ "message": "remplace", "idMessage": id,
            "donnees": { "idZone": zone, "nouvelleChaine": text, "positionRemplacementDebut": start,
                         "positionRemplacementFin": end } })
}

fn reply(id: &str, value: bool) -> Value {
    json!({ "idMessage": id, "donnees": value })
}

#[test]
fn a_correction_from_start_to_end() {
    let (listener, port) = listen();
    let server = thread::spawn(move || {
        let mut fake = FakeAntidote::accept(&listener, &[]);
        assert_eq!(fake.message(), json!({ "outilApi": "Correcteur", "message": "LanceOutil", "versionApi": 2 }));

        fake.send(&json!({ "message": "init", "idMessage": "m1" }));
        assert_eq!(
            fake.message(),
            json!({ "idMessage": "m1", "titreDocument": "Message", "retourChariot": "\n", "permetEnvoiCourriel": true,
                    "filtreActif": "texte" })
        );
        fake.send(&json!({ "message": "cheminDocument", "idMessage": "m2" }));
        assert_eq!(fake.message(), json!({ "idMessage": "m2", "donnees": "brouillon-12" }));
        fake.send(&json!({ "message": "docEstDisponible", "idMessage": "m2b" }));
        assert_eq!(fake.message(), reply("m2b", true));
        fake.send(&json!({ "message": "donneZonesTexte", "idMessage": "m3", "donnees": { "pourSelectionActive": false } }));
        assert_eq!(
            fake.message(),
            json!({ "idMessage": "m3", "donnees": [{
                "texte": BODY, "idZone": "corps", "positionSelectionDebut": 9, "positionSelectionFin": 15,
                "zoneEstEnFocus": true, "infoStyle": [{ "positionDebut": 0, "positionFin": 3, "style": "gras" }] }] })
        );

        // The text is still what Antidote read, or not.
        fake.send(&edit("m4", "corps", "manges", 9, 15));
        assert_eq!(fake.message(), reply("m4", true));
        fake.send(&edit("m5", "corps", "mange", 9, 15));
        assert_eq!(fake.message(), reply("m5", false));
        // A ping in between is answered.
        fake.send_raw(&server_frame(0x89, b"?"));
        assert_eq!(fake.frame(), (0x8A, b"?".to_vec()));

        // The correction.
        fake.send(&replacement("m6", "corps", "mange", 9, 15));
        assert_eq!(fake.message(), reply("m6", true));
        // Refused: past the end, a zone Antidote was not given, a bidi control, a negative position.
        fake.send(&replacement("m7", "corps", "x", 9, 99));
        assert_eq!(fake.message(), reply("m7", false));
        fake.send(&replacement("m8", "autre", "x", 0, 1));
        assert_eq!(fake.message(), reply("m8", false));
        fake.send(&replacement("m9", "corps", "a\u{202E}b", 0, 1));
        assert_eq!(fake.message(), reply("m9", false));
        fake.send(&replacement("m10", "corps", "x", -1, 1));
        assert_eq!(fake.message(), reply("m10", false));

        // An unknown message gets no answer: the next answer is for m12.
        fake.send(&json!({ "message": "effaceTout", "idMessage": "m11" }));
        // A message in two packets, the second in two WebSocket fragments.
        let message = edit("m12", "corps", "mange", 9, 14).to_string();
        let (first, second) = message.split_at(10);
        let first = json!({ "idPaquet": 1, "totalPaquet": 2, "donnees": first }).to_string();
        let second = json!({ "idPaquet": 2, "totalPaquet": 2, "donnees": second }).to_string();
        fake.send_raw(&server_frame(0x81, first.as_bytes()));
        let (a, b) = second.as_bytes().split_at(7);
        let mut fragments = server_frame(0x01, a);
        fragments.extend(server_frame(0x80, b));
        fake.send_raw(&fragments);
        assert_eq!(fake.message(), reply("m12", true));

        // No answer to these.
        fake.send(&json!({ "message": "selectionne", "donnees": { "idZone": "corps", "positionDebut": 0, "positionFin": 3 } }));
        fake.send(&json!({ "message": "selectionne", "donnees": { "idZone": "corps", "positionDebut": 0, "positionFin": 300 } }));
        fake.send(&json!({ "message": "retourneAuDocument" }));
        fake.send(&json!({ "message": "nouvelleMemoireCorrection", "donnees": "QUJD" }));
        fake.send(&json!({ "message": "envoi" }));
        fake.send(&json!({ "message": "reponseAntiOups", "donnees": { "id": "pas-demande", "detections": ["erreurs"] } }));
        fake.close();
    });

    let mut editor = Editor::new(true);
    let woken = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&woken);
    let mut session = Session::start_with(Unit::CodePoints, move || Ok(port), move || flag.store(true, Ordering::SeqCst));
    session.launch(Tool::Corrector);
    run(&mut session, &mut editor);
    server.join().unwrap();

    assert_eq!(editor.ended, Some(None));
    assert_eq!(editor.body, "Été 🍎 je mange");
    assert_eq!(editor.selection, Some(0..5), "\"Été\", not the selection past the end");
    assert_eq!((editor.started, editor.returned, editor.sent), (1, 1, 1));
    assert_eq!(editor.memory.as_deref(), Some("QUJD"));
    assert!(editor.replies.is_empty(), "an answer to no request");
    assert!(woken.load(Ordering::SeqCst));
}

#[test]
fn anti_oups_then_the_corrector_in_utf16() {
    let (listener, port) = listen();
    let server = thread::spawn(move || {
        let mut fake = FakeAntidote::accept(&listener, &[]);
        let request = fake.message();
        assert_eq!(request["message"], "AntiOups");
        assert_eq!(request["versionApi"], 2);
        assert_eq!(request["sujet"], "Bonjour");
        assert_eq!(request["corps"], "Je manges");
        assert_eq!(request["destinataires"], json!(["zoe@exemple.ca"]));
        let id = request["id"].as_str().unwrap().to_string();
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "4", "a UUID v4");
        fake.send(&json!({ "message": "reponseAntiOups", "donnees": { "id": "autre", "detections": ["erreurs"] } }));
        fake.send(&json!({ "message": "reponseAntiOups", "donnees": { "id": id, "detections": ["erreurs", "tonVexant"] } }));
        fake.send(&json!({ "message": "reponseAntiOups", "donnees": { "id": id, "detections": [] } }));

        assert_eq!(fake.message()["outilApi"], "Correcteur");
        fake.send(&json!({ "message": "init", "idMessage": "i" }));
        assert!(fake.message().get("permetEnvoiCourriel").is_none());
        // Antidote 11 v4's place for the flag.
        fake.send(&json!({ "message": "donneZonesTexte", "idMessage": "z", "pourSelectionActive": true }));
        let zones = fake.message();
        assert_eq!(zones["donnees"][0]["positionSelectionDebut"], 10);
        assert_eq!(zones["donnees"][0]["positionSelectionFin"], 16);
        fake.send(&edit("e1", "corps", "manges", 10, 16));
        assert_eq!(fake.message(), reply("e1", true));
        fake.send(&edit("e2", "corps", "🍎", 4, 6));
        assert_eq!(fake.message(), reply("e2", true));
        fake.send(&edit("e3", "corps", "", 5, 5));
        assert_eq!(fake.message(), reply("e3", false), "inside the apple");
        fake.send(&json!({ "message": "envoi" }));
        fake.close();
    });

    let mut editor = Editor::new(false);
    let mut session = Session::start_with(Unit::Utf16, move || Ok(port), || {});
    let id = session.anti_oups("Bonjour", "Je manges", &["zoe@exemple.ca".to_string()]).unwrap();
    // As Druide's example: wait for Anti-Oups!, then open the corrector if it found something.
    let deadline = Instant::now() + Duration::from_secs(30);
    while editor.replies.is_empty() {
        assert!(Instant::now() < deadline, "no Anti-Oups! answer");
        session.process(&mut editor);
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(editor.replies[0].id, id);
    assert_eq!(editor.replies[0].detections, vec![Detection::Errors, Detection::HurtfulTone]);
    assert!(editor.replies[0].should_open_corrector());
    session.launch(Tool::Corrector);
    run(&mut session, &mut editor);
    server.join().unwrap();

    assert_eq!(editor.replies.len(), 1, "one answer per request of this session");
    assert_eq!(editor.sent, 0, "sending was not allowed");
    assert_eq!(editor.ended, Some(None));
}

#[test]
fn no_antidote() {
    let woken = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&woken);
    let mut editor = Editor::new(false);
    let mut session =
        Session::start_with(Unit::CodePoints, || Err(Error::Unavailable), move || flag.store(true, Ordering::SeqCst));
    session.launch(Tool::Guides);
    run(&mut session, &mut editor);
    assert_eq!(editor.ended, Some(Some(Error::Unavailable.to_string())));
    assert!(woken.load(Ordering::SeqCst));
    // Nothing more happens after the end.
    session.process(&mut editor);
    session.launch(Tool::Guides);
}

#[test]
fn a_broken_packet_ends_the_session() {
    let (listener, port) = listen();
    let server = thread::spawn(move || {
        let mut fake = FakeAntidote::accept(&listener, &[]);
        fake.send_raw(&server_frame(0x81, br#"{"idPaquet":1,"totalPaquet":0,"donnees":""}"#));
        fake.wait_for_hang_up();
    });
    let mut editor = Editor::new(false);
    let mut session = Session::start_with(Unit::CodePoints, move || Ok(port), || {});
    run(&mut session, &mut editor);
    server.join().unwrap();
    let error = editor.ended.flatten().unwrap();
    assert!(error.contains("protocol"), "{error}");
}

#[test]
fn dropping_the_session_closes_it() {
    let (listener, port) = listen();
    let server = thread::spawn(move || {
        let mut fake = FakeAntidote::accept(&listener, &[]);
        assert_eq!(fake.message()["outilApi"], "Dictionnaires");
        assert_eq!(fake.frame(), (0x88, 1000u16.to_be_bytes().to_vec()));
        fake.send_raw(&server_frame(0x88, &1000u16.to_be_bytes()));
    });
    let session = Session::start_with(Unit::CodePoints, move || Ok(port), || {});
    session.launch(Tool::Dictionaries);
    drop(session);
    server.join().unwrap();
}

#[test]
fn zones_are_checked_before_they_go() {
    fn zone(id: &str, text: &str, selection: Option<Range<usize>>) -> Zone {
        Zone { id: id.into(), text: text.into(), selection, focused: false, styles: vec![Style { range: 1..2, kind: StyleKind::Italic }] }
    }
    // A selection or a style inside a character is left out; the zone stays.
    let z = zone("a", "é", Some(0..1));
    let wire = wire_zone(&z, Unit::CodePoints).unwrap();
    assert_eq!((wire.position_selection_debut, wire.position_selection_fin), (None, None));
    assert!(wire.info_style.is_empty());
    // An id with a control character, or a text too long, leaves the zone out.
    assert!(wire_zone(&zone("a\nb", "x", None), Unit::CodePoints).is_none());
    assert!(wire_zone(&zone("a", &"x".repeat(MAX_ZONE_TEXT + 1), None), Unit::CodePoints).is_none());
    let ok = zone("", "abc", Some(1..3));
    let wire = wire_zone(&ok, Unit::Utf16).unwrap();
    assert_eq!((wire.position_selection_debut, wire.position_selection_fin), (Some(1), Some(3)));
    assert_eq!(wire.info_style.len(), 1);
}
