use serde_json::json;

use super::*;

/// The message inside a single packet MailALM sends.
fn inner(packet: &str) -> Value {
    let packet: Value = serde_json::from_str(packet).unwrap();
    assert_eq!(packet["idPaquet"], 0);
    assert_eq!(packet["totalPaquet"], 1);
    serde_json::from_str(packet["donnees"].as_str().unwrap()).unwrap()
}

fn packet(id: u64, total: u64, donnees: &str) -> String {
    json!({ "idPaquet": id, "totalPaquet": total, "donnees": donnees }).to_string()
}

fn request(message: Value) -> Parsed {
    parse_request(&message.to_string())
}

// --- What MailALM sends -----------------------------------------------------------------------

#[test]
fn launch_is_what_the_python_sends() {
    let sent = launch(Tool::Corrector).unwrap();
    assert_eq!(sent, r#"{"idPaquet":0,"totalPaquet":1,"donnees":"{\"outilApi\":\"Correcteur\",\"message\":\"LanceOutil\",\"versionApi\":2}"}"#);
    assert_eq!(inner(&launch(Tool::Dictionaries).unwrap())["outilApi"], "Dictionnaires");
    assert_eq!(inner(&launch(Tool::Guides).unwrap())["outilApi"], "Guides");
}

#[test]
fn anti_oups_request() {
    let sent = inner(&anti_oups("Sujet", "Bonjour \"toi\"", &["Zoé <zoe@exemple.ca>".to_string()], "id-1").unwrap());
    assert_eq!(
        sent,
        json!({ "sujet": "Sujet", "corps": "Bonjour \"toi\"", "destinataires": ["Zoé <zoe@exemple.ca>"], "id": "id-1",
                "message": "AntiOups", "versionApi": 2 })
    );
}

#[test]
fn replies_carry_the_message_id() {
    assert_eq!(inner(&reply_bool(Some("m1"), true).unwrap()), json!({ "idMessage": "m1", "donnees": true }));
    assert_eq!(inner(&reply_bool(None, false).unwrap()), json!({ "donnees": false }));
    assert_eq!(inner(&reply_text(Some("m2"), "").unwrap()), json!({ "idMessage": "m2", "donnees": "" }));
}

#[test]
fn zones_reply_leaves_out_what_is_absent() {
    let zones = [
        WireZone {
            texte: "Je manges",
            id_zone: "corps",
            position_selection_debut: Some(3),
            position_selection_fin: Some(9),
            zone_est_en_focus: true,
            info_style: vec![WireStyle { position_debut: 0, position_fin: 2, style: StyleKind::Bold.wire() }],
        },
        WireZone {
            texte: "",
            id_zone: "sujet",
            position_selection_debut: None,
            position_selection_fin: None,
            zone_est_en_focus: false,
            info_style: Vec::new(),
        },
    ];
    assert_eq!(
        inner(&reply_zones(Some("m3"), &zones).unwrap()),
        json!({ "idMessage": "m3", "donnees": [
            { "texte": "Je manges", "idZone": "corps", "positionSelectionDebut": 3, "positionSelectionFin": 9,
              "zoneEstEnFocus": true, "infoStyle": [{ "positionDebut": 0, "positionFin": 2, "style": "gras" }] },
            { "texte": "", "idZone": "sujet", "zoneEstEnFocus": false },
        ]})
    );
}

#[test]
fn init_reply_names_and_checks() {
    let configuration = Configuration {
        document_title: Some("Réponse à Zoé".into()),
        newline: Some("\n".into()),
        cache_by_document_path: true,
        allow_newlines: Some(true),
        allow_no_break_space: Some(true),
        allow_thin_space: Some(false),
        allow_send: Some(true),
        replace_without_selection: Some(false),
        correction_memory: Some("QUJD".into()),
        document_type: Some(DocumentType::Html),
    };
    assert_eq!(
        inner(&reply_init(Some("m0"), &configuration).unwrap()),
        json!({ "idMessage": "m0", "titreDocument": "Réponse à Zoé", "retourChariot": "\n",
                "typeIdentifiantCache": "forceEmplacementFichier", "permetRetourChariot": true,
                "permetEspaceInsecable": true, "permetEspaceFine": false, "permetEnvoiCourriel": true,
                "remplaceSansSelection": false, "memoireCorrection": "QUJD", "filtreActif": "html" })
    );
    // Nothing set: only the id. Invalid values are left out.
    assert_eq!(inner(&reply_init(Some("m0"), &Configuration::default()).unwrap()), json!({ "idMessage": "m0" }));
    let bad = Configuration {
        document_title: Some("a\u{1b}b".into()),
        newline: Some("\\n".into()),
        correction_memory: Some("pas du base64!".into()),
        ..Configuration::default()
    };
    assert_eq!(inner(&reply_init(None, &bad).unwrap()), json!({}));
}

#[test]
fn oversized_messages_are_not_sent() {
    let body = "x".repeat(MAX_MESSAGE);
    assert!(anti_oups("", &body, &[], "id").is_none());
}

// --- Packets ----------------------------------------------------------------------------------

#[test]
fn single_packets_either_numbering() {
    let mut packets = Reassembler::default();
    assert_eq!(packets.push(&packet(0, 1, "{\"a\":1}")).unwrap().as_deref(), Some("{\"a\":1}"));
    assert_eq!(packets.push(&packet(1, 1, "{\"b\":2}")).unwrap().as_deref(), Some("{\"b\":2}"));
}

#[test]
fn several_packets_are_joined_in_order() {
    let mut packets = Reassembler::default();
    // Numbered from 1, as the Python reads them, and out of order.
    assert_eq!(packets.push(&packet(2, 3, "\"b\"")).unwrap(), None);
    assert_eq!(packets.push(&packet(1, 3, "{\"a\":")).unwrap(), None);
    assert_eq!(packets.push(&packet(3, 3, "}")).unwrap().as_deref(), Some("{\"a\":\"b\"}"));
    // Numbered from 0, as the README says.
    assert_eq!(packets.push(&packet(0, 2, "[1,")).unwrap(), None);
    assert_eq!(packets.push(&packet(1, 2, "2]")).unwrap().as_deref(), Some("[1,2]"));
}

#[test]
fn bad_packets_are_refused() {
    fn refused(frames: &[String]) -> bool {
        let mut packets = Reassembler::default();
        frames.iter().map(|f| packets.push(f)).find_map(Result::err).is_some()
    }
    assert!(refused(&["not json".into()]));
    assert!(refused(&[json!({ "idPaquet": 0, "totalPaquet": 1 }).to_string()]));
    assert!(refused(&[json!({ "idPaquet": -1, "totalPaquet": 1, "donnees": "" }).to_string()]));
    assert!(refused(&[json!({ "idPaquet": 0.5, "totalPaquet": 1, "donnees": "" }).to_string()]));
    assert!(refused(&[packet(0, 0, "")]));
    assert!(refused(&[packet(0, MAX_PACKETS as u64 + 1, "")]));
    assert!(refused(&[packet(3, 2, "")]));
    // A duplicate, a change of total, a mix of numberings (0 and 2 of 2).
    assert!(refused(&[packet(1, 3, "a"), packet(1, 3, "b")]));
    assert!(refused(&[packet(1, 3, "a"), packet(2, 2, "b")]));
    assert!(refused(&[packet(0, 2, "a"), packet(2, 2, "b")]));
    // Too many bytes in all.
    let half = "x".repeat(MAX_MESSAGE / 2 + 1);
    assert!(refused(&[packet(1, 2, &half), packet(2, 2, &half)]));
}

#[test]
fn a_refused_packet_starts_over() {
    let mut packets = Reassembler::default();
    assert_eq!(packets.push(&packet(1, 2, "a")).unwrap(), None);
    assert!(packets.push(&packet(1, 2, "a")).is_err());
    assert_eq!(packets.push(&packet(1, 1, "b")).unwrap().as_deref(), Some("b"));
}

// --- What Antidote sends ----------------------------------------------------------------------

fn kind(parsed: Parsed) -> RequestKind {
    match parsed {
        Parsed::Request(request) => request.kind,
        other => panic!("not a request: {other:?}"),
    }
}

#[test]
fn simple_requests() {
    assert_eq!(
        request(json!({ "message": "init", "idMessage": "m1" })),
        Parsed::Request(Request { id: Some("m1".into()), kind: RequestKind::Init })
    );
    assert_eq!(kind(request(json!({ "message": "cheminDocument", "idMessage": "x" }))), RequestKind::DocumentPath);
    assert_eq!(kind(request(json!({ "message": "docEstDisponible" }))), RequestKind::DocumentAvailable);
    assert_eq!(kind(request(json!({ "message": "retourneAuDocument", "donnees": null }))), RequestKind::ReturnToDocument);
    assert_eq!(kind(request(json!({ "message": "envoi", "inconnu": [1, 2] }))), RequestKind::Send);
}

#[test]
fn zones_request_reads_both_places() {
    let zones = |message: Value| kind(request(message));
    assert_eq!(
        zones(json!({ "message": "donneZonesTexte", "donnees": { "pourSelectionActive": true } })),
        RequestKind::Zones { for_selection: true }
    );
    assert_eq!(
        zones(json!({ "message": "donneZonesTexte", "pourSelectionActive": true })),
        RequestKind::Zones { for_selection: true }
    );
    // `donnees` wins, as in the Python; a missing or odd flag is false.
    assert_eq!(
        zones(json!({ "message": "donneZonesTexte", "donnees": {}, "pourSelectionActive": true })),
        RequestKind::Zones { for_selection: false }
    );
    assert_eq!(
        zones(json!({ "message": "donneZonesTexte", "donnees": { "pourSelectionActive": "oui" } })),
        RequestKind::Zones { for_selection: false }
    );
}

#[test]
fn edit_and_replace_requests() {
    assert_eq!(
        kind(request(json!({ "message": "editionPossible", "idMessage": "m",
            "donnees": { "idZone": "corps", "contexte": "manges", "positionDebut": 3, "positionFin": 9 } }))),
        RequestKind::CanEdit { zone: "corps".into(), context: "manges".into(), start: 3, end: 9 }
    );
    assert_eq!(
        kind(request(json!({ "message": "remplace", "idMessage": "m",
            "donnees": { "idZone": "", "nouvelleChaine": "mange\tet\r\nboit\u{a0}!", "positionRemplacementDebut": 3,
                         "positionRemplacementFin": 9 } }))),
        RequestKind::Replace { zone: String::new(), text: "mange\tet\r\nboit\u{a0}!".into(), start: 3, end: 9 }
    );
    assert_eq!(
        kind(request(json!({ "message": "selectionne", "donnees": { "idZone": "corps", "positionDebut": 0, "positionFin": 0 } }))),
        RequestKind::Select { zone: "corps".into(), start: 0, end: 0 }
    );
}

#[test]
fn hostile_edits_are_refused() {
    let replace = |donnees: Value| request(json!({ "message": "remplace", "idMessage": "m", "donnees": donnees }));
    let refused = |parsed: Parsed| matches!(parsed, Parsed::Refuse { id: Some(ref id), .. } if id == "m");
    let ok = json!({ "idZone": "corps", "nouvelleChaine": "x", "positionRemplacementDebut": 1, "positionRemplacementFin": 2 });
    assert!(matches!(replace(ok.clone()), Parsed::Request(_)));
    let with = |field: &str, value: Value| {
        let mut donnees = ok.clone();
        donnees[field] = value;
        donnees
    };
    assert!(refused(replace(with("positionRemplacementDebut", json!(-1)))));
    assert!(refused(replace(with("positionRemplacementDebut", json!(1.5)))));
    assert!(refused(replace(with("positionRemplacementDebut", json!("1")))));
    assert!(refused(replace(with("positionRemplacementFin", json!(u64::MAX)))));
    assert!(refused(replace(with("positionRemplacementFin", json!(MAX_POSITION + 1)))));
    assert!(refused(replace(with("positionRemplacementDebut", json!(3)))));
    assert!(refused(replace(with("idZone", json!("a".repeat(MAX_ID + 1))))));
    assert!(refused(replace(with("idZone", json!("a\nb")))));
    assert!(refused(replace(with("idZone", json!(null)))));
    assert!(refused(replace(with("nouvelleChaine", json!("x".repeat(MAX_EDIT + 1))))));
    assert!(refused(replace(with("nouvelleChaine", json!("a\u{0}b")))));
    assert!(refused(replace(with("nouvelleChaine", json!("a\u{1b}[2Jb")))));
    assert!(refused(replace(with("nouvelleChaine", json!("a\u{85}b")))));
    assert!(refused(replace(with("nouvelleChaine", json!("abc\u{202E}fdp.exe")))));
    assert!(refused(replace(with("nouvelleChaine", json!("a\u{2066}b")))));
    assert!(refused(replace(json!("pas un objet"))));
    assert!(refused(request(json!({ "message": "remplace", "idMessage": "m" }))));
    let can_edit = |donnees: Value| request(json!({ "message": "editionPossible", "idMessage": "m", "donnees": donnees }));
    assert!(refused(can_edit(json!({ "idZone": "z", "contexte": "x".repeat(MAX_EDIT + 1), "positionDebut": 0, "positionFin": 1 }))));
    assert!(refused(can_edit(json!({ "idZone": "z", "contexte": "x", "positionDebut": 2, "positionFin": 1 }))));
    // A bad selection takes no answer: it is ignored.
    assert!(matches!(
        request(json!({ "message": "selectionne", "donnees": { "idZone": "z", "positionDebut": 5, "positionFin": 1 } })),
        Parsed::Ignore(_)
    ));
}

#[test]
fn odd_messages_are_ignored() {
    assert!(matches!(parse_request("pas du json"), Parsed::Ignore(_)));
    assert!(matches!(parse_request("[1,2]"), Parsed::Ignore(_)));
    assert!(matches!(request(json!({ "idMessage": "m" })), Parsed::Ignore(_)));
    assert!(matches!(request(json!({ "message": "effaceTout", "idMessage": "m" })), Parsed::Ignore(_)));
    assert!(matches!(request(json!({ "message": "init", "idMessage": 12 })), Parsed::Ignore(_)));
    assert!(matches!(request(json!({ "message": "init", "idMessage": "a".repeat(MAX_ID + 1) })), Parsed::Ignore(_)));
    assert!(matches!(request(json!({ "message": "init", "idMessage": "a\rb" })), Parsed::Ignore(_)));
    // Deep nesting is stopped by serde_json's recursion limit, not by the stack.
    let deep = format!("{{\"message\":\"init\",\"donnees\":{}{}}}", "[".repeat(100_000), "]".repeat(100_000));
    assert!(matches!(parse_request(&deep), Parsed::Ignore(_)));
}

#[test]
fn correction_memory() {
    let memory = |donnees: Value| request(json!({ "message": "nouvelleMemoireCorrection", "donnees": donnees }));
    assert_eq!(kind(memory(json!("QUJD+/=="))), RequestKind::NewCorrectionMemory("QUJD+/==".into()));
    assert_eq!(kind(memory(json!({ "donnees": "QUJD" }))), RequestKind::NewCorrectionMemory("QUJD".into()));
    assert!(matches!(memory(json!("pas <base64>")), Parsed::Ignore(_)));
    assert!(matches!(memory(json!(12)), Parsed::Ignore(_)));
    assert!(matches!(memory(json!({ "donnees": [1] })), Parsed::Ignore(_)));
    assert!(matches!(memory(json!("A".repeat(MAX_MEMORY + 1))), Parsed::Ignore(_)));
}

#[test]
fn anti_oups_replies() {
    let reply = |donnees: Value| request(json!({ "message": "reponseAntiOups", "donnees": donnees }));
    let got = kind(reply(json!({ "id": "u1", "detections": ["erreurs", "tonVexant", "nouveau"] })));
    let RequestKind::AntiOupsReply(got) = got else { panic!("not an Anti-Oups! reply") };
    assert_eq!(got.detections, vec![Detection::Errors, Detection::HurtfulTone, Detection::Other]);
    assert!(got.should_open_corrector());
    let RequestKind::AntiOupsReply(got) = kind(reply(json!({ "id": "u2", "codeErreur": 3 }))) else { panic!() };
    assert_eq!(got.error_code.as_deref(), Some("3"));
    assert!(!got.should_open_corrector());
    let RequestKind::AntiOupsReply(got) = kind(reply(json!({ "id": "u3", "detections": [] }))) else { panic!() };
    assert!(!got.should_open_corrector());
    assert!(matches!(reply(json!({ "detections": [] })), Parsed::Ignore(_)));
    let many = vec!["erreurs"; MAX_DETECTIONS + 1];
    assert!(matches!(reply(json!({ "id": "u4", "detections": many })), Parsed::Ignore(_)));
    assert!(matches!(reply(json!({ "id": "u5", "detections": [1] })), Parsed::Ignore(_)));
    assert!(matches!(reply(json!({ "id": "u6", "codeErreur": { "a": 1 } })), Parsed::Ignore(_)));
}
