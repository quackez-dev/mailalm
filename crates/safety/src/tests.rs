use super::*;

// ---- made-up files ----

/// A ZIP holding `names` (no data: the central directory is what is read), entries encrypted
/// when asked, after `prefix` (a self-extracting program's stub, say).
fn zip_with(prefix: &[u8], names: &[&str], encrypted: bool) -> Vec<u8> {
    let mut out = prefix.to_vec();
    // A local header's start, as a real ZIP begins.
    out.extend_from_slice(b"PK\x03\x04");
    out.extend_from_slice(&[0u8; 26]);
    let cd_offset = 30u32; // offsets as written, without the prefix
    let cd_start = out.len();
    for n in names {
        out.extend_from_slice(b"PK\x01\x02");
        out.extend_from_slice(&[20, 0, 20, 0]); // made by, needed
        out.extend_from_slice(&(if encrypted { 1u16 } else { 0 }).to_le_bytes());
        out.extend_from_slice(&[0u8; 2 + 2 + 2 + 4 + 4 + 4]); // method, time, date, crc, sizes
        out.extend_from_slice(&u16::try_from(n.len()).expect("a short name").to_le_bytes());
        out.extend_from_slice(&[0u8; 2 + 2 + 2 + 2 + 4 + 4]); // extra, comment, disk, attrs, offset
        out.extend_from_slice(n.as_bytes());
    }
    let cd_size = u32::try_from(out.len() - cd_start).expect("a small directory");
    out.extend_from_slice(b"PK\x05\x06");
    out.extend_from_slice(&[0u8; 4]);
    let count = u16::try_from(names.len()).expect("few entries");
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&[0u8; 2]);
    out
}

fn zip(names: &[&str]) -> Vec<u8> {
    zip_with(&[], names, false)
}

/// An OLE compound file whose directory (in the first sector after the header) holds `entries`
/// (name, type), the root entry first with class id `root_id`.
fn ole(entries: &[(&str, u8)], root_id: [u8; 16]) -> Vec<u8> {
    let mut out = vec![0u8; 512];
    out[..8].copy_from_slice(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
    out[0x1E] = 9;
    let mut all = vec![("Root Entry", 5u8)];
    all.extend_from_slice(entries);
    for (i, (name, kind)) in all.iter().enumerate() {
        let mut e = [0u8; 128];
        let units: Vec<u16> = name.encode_utf16().collect();
        for (k, u) in units.iter().enumerate() {
            e[k * 2..k * 2 + 2].copy_from_slice(&u.to_le_bytes());
        }
        let len = u16::try_from((units.len() + 1) * 2).expect("a short name");
        e[64..66].copy_from_slice(&len.to_le_bytes());
        e[66] = *kind;
        e[67] = 1;
        if i == 0 {
            e[80..96].copy_from_slice(&root_id);
        }
        out.extend_from_slice(&e);
    }
    out
}

/// A Windows program (a DLL when asked), with its PE header.
fn exe(dll: bool) -> Vec<u8> {
    let mut out = vec![0u8; 0x200];
    out[..2].copy_from_slice(b"MZ");
    out[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
    out[0x80..0x84].copy_from_slice(b"PE\0\0");
    let characteristics: u16 = if dll { 0x2102 } else { 0x0102 };
    out[0x80 + 22..0x80 + 24].copy_from_slice(&characteristics.to_le_bytes());
    out
}

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
const PDF: &[u8] = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n%%EOF\n";

fn input<'a>(name: &'a str, declared: &'a str, bytes: &'a [u8]) -> Input<'a> {
    Input { raw_name: name, name, declared_type: declared, bytes }
}

fn report(name: &str, bytes: &[u8]) -> Report {
    assess(&input(name, "application/octet-stream", bytes))
}

// ---- real types ----

#[test]
fn magic_numbers_give_the_real_type() {
    assert_eq!(detect(b""), RealType::Empty);
    assert_eq!(detect(PNG), RealType::Image(ImageFormat::Png));
    assert_eq!(detect(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10]), RealType::Image(ImageFormat::Jpeg));
    assert_eq!(detect(b"GIF89a\x01\0\x01\0"), RealType::Image(ImageFormat::Gif));
    assert_eq!(detect(b"RIFF\0\0\0\0WEBPVP8 "), RealType::Image(ImageFormat::Webp));
    let mut bmp = b"BM".to_vec();
    bmp.extend_from_slice(&[0u8; 12]);
    bmp.extend_from_slice(&40u32.to_le_bytes());
    assert_eq!(detect(&bmp), RealType::Image(ImageFormat::Bmp));
    assert_eq!(detect(b"BMW is a car maker"), RealType::Text(TextKind::Plain), "\"BM\" alone is not a BMP");
    assert_eq!(detect(PDF), RealType::Pdf { active: vec![] });
    assert_eq!(detect(b"Rar!\x1a\x07\x01\x00"), RealType::Archive(ArchiveInfo { format: "RAR", encrypted: false, programs: vec![] }));
    assert!(matches!(detect(&[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C, 0, 4]), RealType::Archive(ArchiveInfo { format: "7-Zip", .. })));
    assert!(matches!(detect(&[0x1F, 0x8B, 8, 0]), RealType::Archive(ArchiveInfo { format: "gzip", .. })));
    assert_eq!(detect(&exe(false)), RealType::Program(ProgramKind::Windows));
    assert_eq!(detect(&exe(true)), RealType::Program(ProgramKind::WindowsLibrary));
    assert_eq!(detect(b"MZ"), RealType::Program(ProgramKind::Windows), "a DOS header alone is still a program");
    assert_eq!(detect(b"\x7fELF\x02\x01\x01"), RealType::Program(ProgramKind::Linux));
    let lnk = [0x4C, 0, 0, 0, 0x01, 0x14, 0x02, 0, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46, 0x9B, 0];
    assert_eq!(detect(&lnk), RealType::Shortcut);
    let mut iso = vec![0u8; 0x8001];
    iso.extend_from_slice(b"CD001\x01");
    assert_eq!(detect(&iso), RealType::DiskImage("ISO"));
    assert_eq!(detect(b"vhdxfile\0\0"), RealType::DiskImage("VHDX"));
    assert_eq!(detect(&[0x00, 0x01, 0xFE, 0x02, 0x00, 0x80]), RealType::Unknown);
}

#[test]
fn office_open_xml_and_its_macros() {
    let docx = zip(&["[Content_Types].xml", "_rels/.rels", "word/document.xml"]);
    assert_eq!(
        detect(&docx),
        RealType::Office(OfficeInfo { format: OfficeFormat::Ooxml, app: App::Word, macros: Macros::None, embedded: false, equation: false })
    );
    let docm = zip(&["[Content_Types].xml", "word/document.xml", "word/vbaProject.bin"]);
    assert!(matches!(detect(&docm), RealType::Office(OfficeInfo { app: App::Word, macros: Macros::Present, .. })));
    let xlsm = zip(&["[Content_Types].xml", "xl/workbook.xml", "xl/macrosheets/sheet1.xml"]);
    assert!(matches!(detect(&xlsm), RealType::Office(OfficeInfo { app: App::Excel, macros: Macros::Present, .. })));
    let pptx = zip(&["[Content_Types].xml", "ppt/presentation.xml", "ppt/embeddings/oleObject1.bin"]);
    assert!(matches!(detect(&pptx), RealType::Office(OfficeInfo { app: App::PowerPoint, macros: Macros::None, embedded: true, .. })));
    let odt = zip(&["mimetype", "content.xml", "Basic/Standard/Module1.xml"]);
    assert!(matches!(detect(&odt), RealType::Office(OfficeInfo { format: OfficeFormat::OpenDocument, macros: Macros::Present, .. })));
    let jar = zip(&["META-INF/MANIFEST.MF", "a/Main.class"]);
    assert_eq!(detect(&jar), RealType::Program(ProgramKind::Java));
}

#[test]
fn a_zip_lists_the_programs_it_holds() {
    let z = zip(&["docs/", "docs/readme.txt", "docs/Invoice.pdf.exe", "run.js", "photo.jpg"]);
    match detect(&z) {
        RealType::Archive(a) => {
            assert_eq!(a.format, "ZIP");
            assert_eq!(a.programs, vec!["Invoice.pdf.exe".to_owned(), "run.js".to_owned()]);
            assert!(!a.encrypted);
        }
        other => panic!("{other:?}"),
    }
    // Encrypted, and after a self-extractor's stub: still read.
    let z = zip_with(&exe(false), &["a.txt"], true);
    assert!(matches!(detect(&z), RealType::Program(_)), "an MZ start is a program first");
    let z = zip_with(b"some junk before the archive", &["a.txt"], true);
    assert!(matches!(detect(&z), RealType::Text(_) | RealType::Unknown), "junk first: not a ZIP by its start");
    let z = zip_with(&[], &["a.txt"], true);
    assert!(matches!(detect(&z), RealType::Archive(ArchiveInfo { encrypted: true, .. })));
}

#[test]
fn old_office_files_and_installers() {
    let doc = ole(&[("WordDocument", 2), ("1Table", 2)], [0; 16]);
    assert_eq!(detect(&doc), RealType::Office(OfficeInfo { format: OfficeFormat::Ole, app: App::Word, macros: Macros::None, embedded: false, equation: false }));
    let doc_macros = ole(&[("WordDocument", 2), ("Macros", 1), ("VBA", 1), ("_VBA_PROJECT", 2)], [0; 16]);
    assert!(matches!(detect(&doc_macros), RealType::Office(OfficeInfo { macros: Macros::Present, .. })));
    let xls = ole(&[("Workbook", 2)], [0; 16]);
    assert!(matches!(detect(&xls), RealType::Office(OfficeInfo { app: App::Excel, macros: Macros::Unknown, .. })));
    let equation = ole(&[("WordDocument", 2), ("ObjectPool", 1), ("\u{1}Ole10Native", 2), ("Equation Native", 2)], [0; 16]);
    assert!(matches!(detect(&equation), RealType::Office(OfficeInfo { embedded: true, equation: true, .. })));
    let msi_id = [0x84, 0x10, 0x0C, 0, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46];
    assert_eq!(detect(&ole(&[("\u{4840}\u{3f3f}", 2)], msi_id)), RealType::Program(ProgramKind::Installer));
}

#[test]
fn rtf_objects_are_seen() {
    assert!(matches!(detect(b"{\\rtf1\\ansi Hello}"), RealType::Office(OfficeInfo { format: OfficeFormat::Rtf, embedded: false, .. })));
    let rtf = b"{\\rtf1{\\object\\objemb{\\*\\objclass Equation.3}{\\*\\objdata 0105}}}";
    assert!(matches!(detect(rtf), RealType::Office(OfficeInfo { embedded: true, equation: true, .. })));
}

#[test]
fn pdf_active_parts_are_found_even_escaped() {
    let pdf = b"%PDF-1.4\n1 0 obj << /OpenAction << /S /J#61vaScript /JS (app.alert(1)) >> /Names << /EmbeddedFiles 3 0 R >> >> endobj";
    assert_eq!(detect(pdf), RealType::Pdf { active: vec!["JavaScript", "JS", "EmbeddedFiles"] });
    let launch = b"junk\n%PDF-1.4\n<< /Type /Action /S /Launch /F (cmd.exe) >>";
    assert_eq!(detect(launch), RealType::Pdf { active: vec!["Launch"] });
}

#[test]
fn text_scripts_and_web_pages() {
    assert_eq!(detect(b"Hello Alex,\r\nThe numbers are attached.\r\n"), RealType::Text(TextKind::Plain));
    assert_eq!(detect("Réunion à 10 h".as_bytes()), RealType::Text(TextKind::Plain));
    assert_eq!(detect(b"R\xe9union \xe0 10 h"), RealType::Text(TextKind::Plain), "Latin-1 is text too");
    assert_eq!(detect(b"Set s = CreateObject(\"WScript.Shell\")\r\ns.Run \"calc\""), RealType::Text(TextKind::Script("wscript.shell")));
    assert_eq!(detect(b"@ECHO OFF\r\ndel /q *"), RealType::Text(TextKind::Script("@echo off")));
    assert_eq!(detect(b"<!DOCTYPE html><html><body>Sign in</body></html>"), RealType::Text(TextKind::Web));
    assert_eq!(detect(b"<?xml version=\"1.0\"?><svg xmlns=\"http://www.w3.org/2000/svg\"/>"), RealType::Text(TextKind::Web));
    assert_eq!(detect(b"[InternetShortcut]\r\nURL=https://example.com/\r\n"), RealType::Shortcut);
    let utf16: Vec<u8> = [0xFF, 0xFE].into_iter().chain("powershell -enc AAAA".encode_utf16().flat_map(u16::to_le_bytes)).collect();
    assert_eq!(detect(&utf16), RealType::Text(TextKind::Script("powershell")));
}

#[test]
fn hostile_and_cut_files_never_panic() {
    let mut big_name = zip(&["a.txt"]);
    // A name length that runs past the end.
    if let Some(p) = big_name.windows(4).position(|w| w == b"PK\x01\x02") {
        big_name[p + 28] = 0xFF;
        big_name[p + 29] = 0xFF;
    }
    let samples = [
        zip(&["[Content_Types].xml", "word/vbaProject.bin"]),
        big_name,
        ole(&[("WordDocument", 2), ("Macros", 1)], [0; 16]),
        exe(true),
        PDF.to_vec(),
        b"%PDF-1.4 /J#6".to_vec(),
        b"%PDF-1.4 /#zz#".to_vec(),
        b"PK\x05\x06\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff".to_vec(),
        vec![0xFF; 70_000],
    ];
    for s in &samples {
        for cut in 0..s.len().min(4096) {
            let _ = detect(&s[..cut]);
        }
        let _ = detect(s);
        let _ = assess(&input("x.bin", "application/octet-stream", s));
    }
}

// ---- warnings ----

#[test]
fn a_double_extension_and_a_program() {
    let r = report("invoice.pdf.exe", &exe(false));
    assert!(r.warnings.contains(&Warning::DoubleExtension { inner: "pdf".into(), outer: "exe".into() }), "{:?}", r.warnings);
    assert!(r.warnings.contains(&Warning::Program));
    assert!(r.dangerous());
    assert!(r.policy.open.is_err());
    assert_eq!(r.policy.preview, None);
    // Spaces before the last dot hide it the same way.
    let r = report("photo.jpg          .scr", &exe(false));
    assert!(r.warnings.iter().any(|w| matches!(w, Warning::DoubleExtension { .. })));
    // Two harmless extensions are fine.
    assert!(!report("report.2026.pdf", PDF).warnings.iter().any(|w| matches!(w, Warning::DoubleExtension { .. })));
    assert!(!report("backup.tar.gz", &[0x1F, 0x8B, 8, 0]).warnings.iter().any(|w| matches!(w, Warning::DoubleExtension { .. })));
}

#[test]
fn hidden_characters_in_the_announced_name() {
    let r = assess(&Input { raw_name: "invoice\u{202e}fdp.exe", name: "invoicefdp.exe", declared_type: "application/pdf", bytes: &exe(false) });
    assert!(r.warnings.contains(&Warning::HiddenCharacters));
    assert!(r.warnings.iter().any(|w| matches!(w, Warning::DeclaredMismatch { .. })), "declared a PDF: {:?}", r.warnings);
    assert!(has_hidden_characters("a\u{200b}b.txt"));
    assert!(has_hidden_characters("a\u{7}.txt"));
    assert!(!has_hidden_characters("Réunion – compte rendu.pdf"));
}

#[test]
fn an_extension_that_lies() {
    let r = report("photo.jpg", &exe(false));
    assert!(r.warnings.iter().any(|w| matches!(w, Warning::ExtensionMismatch { extension, .. } if extension == "jpg")), "{:?}", r.warnings);
    assert!(r.warnings.contains(&Warning::Program));
    assert!(r.policy.open.is_err());
    // A PNG called .jpg: both pictures, no mismatch, previewable, opens as .jpg.
    let r = report("photo.jpg", PNG);
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    assert_eq!(r.policy.preview, Some(Preview::Image));
    assert_eq!(r.policy.open, Ok("jpg"));
    // A web page called .pdf.
    let r = report("statement.pdf", b"<html><script>location='https://phish.example'</script></html>");
    assert!(r.warnings.iter().any(|w| matches!(w, Warning::ExtensionMismatch { .. })));
    assert!(r.warnings.contains(&Warning::WebPage));
}

#[test]
fn macros_archives_shortcuts_and_disk_images() {
    let docm = zip(&["[Content_Types].xml", "word/document.xml", "word/vbaProject.bin"]);
    let r = report("letter.docx", &docm);
    assert!(r.warnings.contains(&Warning::Macros));
    assert!(r.policy.open.is_err());
    // A macro-enabled type without a VBA project found: still a warning.
    let r = report("sheet.xlsm", &zip(&["[Content_Types].xml", "xl/workbook.xml"]));
    assert!(r.warnings.contains(&Warning::Macros));

    let r = report("files.zip", &zip(&["a/b/setup.exe"]));
    assert!(r.warnings.contains(&Warning::Archive));
    assert!(r.warnings.contains(&Warning::ArchiveHoldsPrograms(vec!["setup.exe".into()])));
    assert!(report("secret.zip", &zip_with(&[], &["a.pdf"], true)).warnings.contains(&Warning::Encrypted));

    let r = report("Documents.lnk", &[0x4C, 0, 0, 0, 0x01, 0x14, 0x02, 0, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
    assert!(r.warnings.contains(&Warning::Shortcut));
    assert!(report("site.url", b"[InternetShortcut]\r\nURL=https://example.com/").warnings.contains(&Warning::Shortcut));

    let mut iso = vec![0u8; 0x8001];
    iso.extend_from_slice(b"CD001");
    let r = report("invoice.iso", &iso);
    assert!(r.warnings.contains(&Warning::DiskImage));
    assert!(r.policy.open.is_err());
}

#[test]
fn scripts_by_name_or_by_content() {
    let r = report("update.js", b"var x = 1;");
    assert!(r.warnings.iter().any(|w| matches!(w, Warning::Script(_))));
    assert_eq!(r.policy.preview, Some(Preview::Text), "its source can be read");
    assert!(r.policy.open.is_err());
    // No extension, but it reads like a script.
    let r = report("README", b"powershell -nop -w hidden -enc AAAA");
    assert!(r.warnings.iter().any(|w| matches!(w, Warning::Script(_))));
    // A .txt that mentions PowerShell is still a .txt: Notepad shows it.
    let r = report("notes.txt", b"Run powershell as admin, then reboot.");
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    assert_eq!(r.policy.open, Ok("txt"));
}

#[test]
fn what_may_be_opened_with_windows() {
    assert_eq!(report("statement.PDF", PDF).policy.open, Ok("pdf"));
    assert_eq!(report("letter.docx", &zip(&["[Content_Types].xml", "word/document.xml"])).policy.open, Ok("docx"));
    assert_eq!(report("budget.xlsx", &zip(&["[Content_Types].xml", "xl/workbook.xml"])).policy.open, Ok("xlsx"));
    assert_eq!(report("old.doc", &ole(&[("WordDocument", 2)], [0; 16])).policy.open, Ok("doc"));
    // Old Excel: macros cannot be checked.
    assert!(report("old.xls", &ole(&[("Workbook", 2)], [0; 16])).policy.open.is_err());
    // Embedded objects, a PDF with JavaScript, no extension: saved, not opened.
    assert!(report("deck.pptx", &zip(&["[Content_Types].xml", "ppt/p.xml", "ppt/embeddings/oleObject1.bin"])).policy.open.is_err());
    assert!(report("form.pdf", b"%PDF-1.4 /JS (x)").policy.open.is_err());
    assert!(report("statement", PDF).policy.open.is_err());
    // A CSV opens in Excel (formulas): not on the list.
    assert!(report("data.csv", b"a,b\n1,2\n").policy.open.is_err());
    assert!(report("empty.txt", b"").warnings.contains(&Warning::Empty));
    // TIFF opens but is not previewed.
    let tiff = report("scan.tif", b"II*\0\x08\0\0\0");
    assert_eq!(tiff.policy.preview, None);
    assert_eq!(tiff.policy.open, Ok("tif"));
}

#[test]
fn the_most_serious_warnings_come_first() {
    let r = report("files.zip.exe", &exe(false));
    let first_caution = r.warnings.iter().position(|w| w.severity() == Severity::Caution).unwrap_or(r.warnings.len());
    assert!(r.warnings[..first_caution].iter().all(|w| w.severity() == Severity::Danger));
    assert!(r.warnings[first_caution..].iter().all(|w| w.severity() == Severity::Caution));
    for w in &r.warnings {
        assert!(!w.text().is_empty());
    }
}

// ---- sender checks ----

#[test]
fn gmail_authentication_results() {
    let h = " mx.google.com;\r\n       dkim=pass header.i=@example.com header.s=s1 header.b=AbC;\r\n       spf=pass (google.com: domain of a@example.com designates 1.2.3.4 as permitted sender) smtp.mailfrom=a@example.com;\r\n       dmarc=pass (p=REJECT sp=REJECT dis=NONE) header.from=example.com";
    let a = authentication(Some(h));
    assert!(a.found);
    assert_eq!(a.checked_by.as_deref(), Some("mx.google.com"));
    assert_eq!(a.spf.verdict, Verdict::Pass);
    assert_eq!(a.dkim.verdict, Verdict::Pass);
    assert_eq!(a.dmarc.verdict, Verdict::Pass);
}

#[test]
fn outlook_authentication_results_have_no_server_name() {
    let h = " spf=softfail (sender IP is 10.0.0.1) smtp.mailfrom=example.com; dkim=none (message not signed) header.d=none;dmarc=fail action=quarantine header.from=example.com;compauth=fail reason=001";
    let a = authentication(Some(h));
    assert_eq!(a.checked_by, None);
    assert_eq!(a.spf, Check { verdict: Verdict::Fail, word: "softfail".into() });
    assert_eq!(a.dkim, Check { verdict: Verdict::Missing, word: "none".into() });
    assert_eq!(a.dmarc.verdict, Verdict::Fail);
}

#[test]
fn comments_and_quotes_do_not_fool_the_reader() {
    // A semicolon and a fake result inside a comment, nested comments, a quoted value.
    let h = "mx.example.net; spf=fail (claims; dmarc=pass (nested) spf=pass) smtp.mailfrom=\"a;b\"@x.example; dkim=fail; dkim=pass header.d=x.example";
    let a = authentication(Some(h));
    assert_eq!(a.spf.verdict, Verdict::Fail);
    assert_eq!(a.dmarc.verdict, Verdict::Missing, "the dmarc=pass was in a comment");
    assert_eq!(a.dkim.verdict, Verdict::Pass, "one DKIM signature passes");
    assert_eq!(authentication(Some("mx.example.net; none")), Authentication { found: true, checked_by: Some("mx.example.net".into()), ..Authentication::default() });
    assert_eq!(authentication(None), Authentication::default());
    assert!(!authentication(Some("   ")).found);
    assert_eq!(authentication(Some("x; spf=TempError")).spf, Check { verdict: Verdict::Other, word: "temperror".into() });
    // Unbalanced and very deep: no panic.
    let _ = authentication(Some(&"(".repeat(100_000)));
    let _ = authentication(Some(&format!("x; spf=pass {}", "\\".repeat(1001))));
}

// ---- files ----

#[test]
fn the_mark_of_the_web() {
    assert_eq!(files::ZONE_IDENTIFIER, "[ZoneTransfer]\r\nZoneId=3\r\nHostUrl=about:internet\r\n");
    let stream = files::zone_stream(std::path::Path::new("C:\\Users\\a\\Downloads\\invoice.pdf"));
    assert_eq!(stream.to_string_lossy(), "C:\\Users\\a\\Downloads\\invoice.pdf:Zone.Identifier");
}

#[test]
fn copies_get_safe_names() {
    assert_eq!(files::safe_stem("Réunion (v2).final.docx"), "Réunion__v2__final");
    assert_eq!(files::safe_stem("..\\..\\evil\u{202e}fdp.exe"), "evilfdp");
    assert_eq!(files::safe_stem("<>:\"|?*.pdf"), "attachment");
    assert_eq!(files::safe_stem(&"a".repeat(300)).chars().count(), 60);
}

#[test]
fn saved_files_and_copies_are_marked() {
    let dir = std::env::temp_dir().join(format!("mailalm-safety-test-{}", std::process::id()));
    let mut folder = files::TempFolder::new(dir.clone());
    let path = folder.copy("Statement March.pdf", "pdf", PDF).expect("written");
    assert_eq!(std::fs::read(&path).expect("read"), PDF);
    let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    assert_eq!(file_name, "1-Statement_March.pdf");
    if cfg!(windows) {
        let mark = std::fs::read_to_string(files::zone_stream(&path)).expect("the mark");
        assert!(mark.contains("ZoneId=3"));
    }
    let second = folder.copy("Statement March.pdf", "pdf", PDF).expect("written");
    assert_ne!(second, path);
    drop(folder);
    assert!(!dir.exists(), "emptied at exit");
}
