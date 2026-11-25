#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};

use mailalm_model::{AccountId, Person, Summary};

use super::*;

/// A fresh directory for one test, under the system's temp folder.
fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mailalm-porting-test-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Everything an import hands over.
fn collect(run: impl FnOnce(&mut dyn Sink) -> Result<Report, String>) -> (Result<Report, String>, Vec<Item>) {
    let mut items = Vec::new();
    let mut sink = |item: Item| -> Result<(), String> {
        items.push(item);
        Ok(())
    };
    let report = run(&mut sink);
    (report, items)
}

const ONE: &[u8] = b"From: Ann <ann@example.org>\r\nSubject: One\r\n\r\nFrom the start, a line.\r\n>From here, another.\r\n>>From there.\r\nFrom\r\n";
const TWO: &[u8] = b"Subject: Two\nX-Mozilla-Status: 0005\n\nFrom me\nend\n\n";

#[test]
fn mbox_escaping_round_trips() {
    let mut w = mbox::Writer::new(Vec::new());
    w.add(ONE, "ann@example.org", 1_795_600_800).unwrap();
    w.add(TWO, "", 0).unwrap();
    let file = w.finish().unwrap();
    let text = String::from_utf8_lossy(&file);
    assert!(text.starts_with("From ann@example.org Wed Nov 25 10:00:00 2026\n"), "{text}");
    assert!(text.contains("\r\n>From the start") && text.contains("\r\n>>From here") && text.contains("\r\n>>>From there"));
    assert!(text.contains("\nFrom MAILER-DAEMON Thu Jan  1 00:00:00 1970\n"));
    assert!(text.contains("\n>From me\n"));

    let (report, items) = collect(|sink| mbox::import_bytes(&file, &["Archive".to_owned()], sink, &Cancel::new()));
    let report = report.unwrap();
    assert_eq!(report.messages, 2);
    assert_eq!(items[0].raw, ONE, "the first message comes back as it was");
    assert_eq!(items[1].raw, TWO, "a message ending with an empty line too");
    assert_eq!(items[0].folder, ["Archive"]);
    assert!(items[0].flags.seen, "no marks: taken as read");
    let marks = items[1].flags;
    assert!(marks.seen && marks.flagged && !marks.answered, "Thunderbird's marks");
}

#[test]
fn an_mbox_is_split_only_after_an_empty_line() {
    let file = b"From a@x Thu Jan  1 00:00:00 1970\nSubject: A\n\nFrom inside the text, unescaped\n\nFrom b@x Thu Jan  1 00:00:00 1970\r\nSubject: B\r\n\r\nbody\r\n\r\n";
    let (report, items) = collect(|sink| mbox::import_bytes(file, &[], sink, &Cancel::new()));
    // mboxo without escapes: a `From ` line after an empty line is read as a new message (the
    // format's own ambiguity), here an empty one, left out.
    let report = report.unwrap();
    assert_eq!((report.messages, report.skipped.get(&Skip::NotAMessage).copied()), (2, Some(1)));
    assert_eq!(items[0].raw, b"Subject: A\n");
    assert_eq!(items[1].raw, b"Subject: B\r\n\r\nbody\r\n");
    let file = b"From a@x\nSubject: A\nFrom: someone\n\nbody\n";
    let (_, items) = collect(|sink| mbox::import_bytes(file, &[], sink, &Cancel::new()));
    assert_eq!(items.len(), 1, "a `From:` header is no separator");
}

#[test]
fn hostile_mbox_files() {
    // Not an mbox at all.
    let (report, _) = collect(|sink| mbox::import_bytes(b"\x89PNG\r\n\x1a\n", &[], sink, &Cancel::new()));
    assert!(report.is_err());
    // Empty: no message, no error.
    let (report, _) = collect(|sink| mbox::import_bytes(b"", &[], sink, &Cancel::new()));
    assert_eq!(report.unwrap().messages, 0);
    // A message past the cap, as one line without an end, between two good ones.
    let mut file = b"From a\nSubject: 1\n\nok\n\n".to_vec();
    file.extend_from_slice(b"From b\nSubject: 2\n\n");
    file.extend(std::iter::repeat_n(b'x', MAX_MESSAGE + 10));
    file.extend_from_slice(b"\n\nFrom c\nSubject: 3\n\nok\n");
    let (report, items) = collect(|sink| mbox::import_bytes(&file, &[], sink, &Cancel::new()));
    let report = report.unwrap();
    assert_eq!((report.messages, report.skipped.get(&Skip::TooLarge).copied()), (2, Some(1)));
    assert_eq!(items[1].raw, b"Subject: 3\n\nok\n");
    // Cut short anywhere: what is whole comes in, nothing breaks.
    let mut w = mbox::Writer::new(Vec::new());
    for _ in 0..5 {
        w.add(ONE, "a@x", 0).unwrap();
    }
    let file = w.finish().unwrap();
    for len in 0..file.len() {
        let (report, items) = collect(|sink| mbox::import_bytes(&file[..len], &[], sink, &Cancel::new()));
        if let Ok(report) = report {
            assert!(report.messages <= 5 && items.len() as u64 == report.messages);
        }
    }
    // Thunderbird's deleted messages stay out.
    let file = b"From a\nX-Mozilla-Status: 0009\nSubject: gone\n\nx\n";
    let (report, items) = collect(|sink| mbox::import_bytes(file, &[], sink, &Cancel::new()));
    assert!(items.is_empty());
    assert_eq!(report.unwrap().skipped.get(&Skip::Deleted), Some(&1));
}

#[test]
fn a_cancelled_import_hands_nothing_more() {
    let mut w = mbox::Writer::new(Vec::new());
    w.add(ONE, "a@x", 0).unwrap();
    let file = w.finish().unwrap();
    let cancel = Cancel::new();
    cancel.cancel();
    let (report, items) = collect(|sink| mbox::import_bytes(&file, &[], sink, &cancel));
    assert!(report.unwrap().cancelled && items.is_empty());
}

#[test]
fn asctime_dates() {
    assert_eq!(mbox::asctime(0), "Thu Jan  1 00:00:00 1970");
    assert_eq!(mbox::asctime(951_782_400), "Tue Feb 29 00:00:00 2000");
    assert_eq!(mbox::asctime(1_795_600_800 + 3_723), "Wed Nov 25 11:02:03 2026");
    assert_eq!(mbox::asctime(-5), "Thu Jan  1 00:00:00 1970");
    assert_eq!(mbox::asctime(i64::MAX), "Fri Dec 31 23:59:59 9999");
}

/// A cache with two messages in `INBOX`, the second one not downloaded.
fn store_with_two(base: &Path) -> mailalm_store::Store {
    let store = mailalm_store::Store::open(base, AccountId(1)).unwrap();
    let s = |uid: u32, date: i64| Summary { uid, date, from: Person { name: "Ann".into(), address: "ann@example.org".into() }, ..Summary::default() };
    store.save_summaries("INBOX", &[s(2, 200), s(1, 100)]).unwrap();
    store.save_message("INBOX", 1, ONE, "").unwrap();
    store
}

#[test]
fn eml_export_writes_the_same_bytes() {
    let dir = temp("eml");
    let store = store_with_two(&dir.join("cache"));
    let to = dir.join("one.eml");
    eml::export(&store, "INBOX", 1, &to).unwrap();
    assert_eq!(fs::read(&to).unwrap(), ONE);
    assert!(eml::export(&store, "INBOX", 2, &dir.join("two.eml")).is_err(), "not in the cache");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn mbox_export_of_a_folder() {
    let dir = temp("mbox");
    let store = store_with_two(&dir.join("cache"));
    let to = dir.join("INBOX.mbox");
    let mut told = Vec::new();
    let done = mbox::export(&store, "INBOX", &to, &Cancel::new(), &mut |n, of| told.push((n, of))).unwrap();
    assert_eq!((done.written, done.missing, done.cancelled), (1, 1, false));
    assert_eq!(told, [(1, 2), (2, 2)]);
    let (report, items) = collect(|sink| mbox::import_file(&to, &[], sink, &Cancel::new()));
    assert_eq!(report.unwrap().messages, 1);
    assert_eq!(items[0].raw, ONE);
    // Cancelled: no file, not even half of one.
    let cancel = Cancel::new();
    cancel.cancel();
    let other = dir.join("other.mbox");
    let done = mbox::export(&store, "INBOX", &other, &cancel, &mut |_, _| {}).unwrap();
    assert!(done.cancelled && !other.exists());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn pdf_export_is_a_whole_pdf() {
    let parsed = mailalm_mime::parse(ONE).unwrap();
    let pdf = pdf::render(&parsed, "Wed 25 Nov 2026 05:00");
    assert!(pdf.starts_with(b"%PDF-1.7\n"));
    assert!(pdf.ends_with(b"startxref\n") || pdf.ends_with(b"%%EOF\n"));
    let text = String::from_utf8_lossy(&pdf);
    assert!(text.contains("/Title (One)") && text.contains("/FontFile2") && text.contains("trailer\n<< /Size "));
    let dir = temp("pdf");
    let to = dir.join("one.pdf");
    pdf::export(ONE, "", &to).unwrap();
    assert!(fs::read(&to).unwrap().starts_with(b"%PDF"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn eml_files_and_trees() {
    let dir = temp("eml-tree");
    let root = dir.join("export");
    fs::create_dir_all(root.join("Inbox").join("Work")).unwrap();
    fs::write(root.join("Inbox").join("a.eml"), ONE).unwrap();
    fs::write(root.join("Inbox").join("Work").join("b.EML"), TWO).unwrap();
    fs::write(root.join("Inbox").join("picture.eml"), b"\x89PNG\r\n\x1a\n").unwrap();
    fs::write(root.join("Inbox").join("notes.txt"), b"Subject: not an eml file\r\n\r\n").unwrap();
    let (report, items) = collect(|sink| eml::import_tree(&root, sink, &Cancel::new()));
    let report = report.unwrap();
    assert_eq!(report.messages, 2);
    assert_eq!(report.skipped.get(&Skip::NotAMessage), Some(&1));
    assert_eq!(items[0].folder, ["Inbox"]);
    assert_eq!(items[1].folder, ["Inbox", "Work"]);
    assert_eq!(items[0].raw, ONE);

    let picked = vec![root.join("Inbox").join("a.eml"), root.join("missing.eml")];
    let (report, items) = collect(|sink| eml::import_files(&picked, &["Picked".to_owned()], sink, &Cancel::new()));
    let report = report.unwrap();
    assert_eq!((report.messages, report.skipped.get(&Skip::Unreadable).copied()), (1, Some(1)));
    assert_eq!(items[0].folder, ["Picked"]);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_thunderbird_profile() {
    let dir = temp("thunderbird");
    let root = dir.join("Local Folders");
    fs::create_dir_all(root.join("Inbox.sbd")).unwrap();
    let mbox_of = |raw: &[u8]| {
        let mut w = mbox::Writer::new(Vec::new());
        w.add(raw, "a@x", 0).unwrap();
        w.finish().unwrap()
    };
    fs::write(root.join("Inbox"), mbox_of(ONE)).unwrap();
    fs::write(root.join("Inbox.msf"), b"// <!-- <mdb:mork:z v=\"1.4\"/> -->").unwrap();
    fs::write(root.join("Inbox.sbd").join("Work"), mbox_of(TWO)).unwrap();
    fs::write(root.join("Trash"), b"").unwrap();
    fs::write(root.join("msgFilterRules.dat"), b"version=\"9\"").unwrap();
    let (report, items) = collect(|sink| mbox::import_tree(&root, sink, &Cancel::new()));
    let report = report.unwrap();
    assert_eq!((report.messages, report.folders), (2, 2));
    assert_eq!(items[0].folder, ["Inbox"]);
    assert_eq!(items[1].folder, ["Inbox", "Work"]);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn imported_messages_go_to_a_local_folder() {
    let dir = temp("store");
    let store = mailalm_store::Store::open(&dir, AccountId(9)).unwrap();
    let uids = add_to_store(&store, "Imported", vec![(ONE.to_vec(), Flags { seen: true, ..Flags::default() }), (TWO.to_vec(), Flags::default())]).unwrap();
    assert_eq!(uids, vec![1, 2]);
    let summaries = store.summaries("Imported");
    let one = summaries.iter().find(|s| s.uid == 1).unwrap();
    assert_eq!((one.subject.as_str(), one.from.address.as_str(), one.flags.seen), ("One", "ann@example.org", true));
    assert!(one.preview.starts_with("From the start"));
    assert_eq!(store.message("Imported", 1).as_deref(), Some(ONE));
    assert!(store.text("Imported", 2).is_some());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn names() {
    assert_eq!(eml::file_name("Re: plans / 2026?", "eml"), "Re_ plans _ 2026_.eml");
    assert_eq!(eml::file_name("", "pdf"), "message.pdf");
    assert_eq!(eml::file_name("con", "eml"), "_con.eml");
    assert_eq!(eml::file_name("LPT1.txt", "eml"), "_LPT1.txt.eml");
    assert_eq!(folder_name("Work/2026\u{202e}"), "Work-2026");
    assert_eq!(folder_name("\r\n"), "Folder");
    assert!(eml::looks_like_message(ONE) && eml::looks_like_message(b"From x\nSubject: y\n\n"));
    assert!(!eml::looks_like_message(b"\x89PNG\r\n") && !eml::looks_like_message(b": no name\n") && !eml::looks_like_message(b""));
    assert!(EM_CLIENT_HELP.contains("File"));
}
