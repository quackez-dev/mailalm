// Adapted from MailCraft (https://github.com/ZIDAN44/mailcraft), commit c3510d13, the tests of
// crates/print/src/lib.rs. MIT OR Apache-2.0.
// Changed: the laid-out message's test and the measurement are left out (no blocks or pictures
// here); the font embedded is egui's Ubuntu Light instead of one read from the system; a test of
// the stroked bold and of the caps added.
#![allow(clippy::unwrap_used)]

use super::*;

fn doc(body: &str) -> Doc {
    Doc {
        title: "Storyboard (v2) review".into(),
        header: vec![("From".into(), "Zoe Park <zoe@studio.example>".into()), ("Date".into(), "Wed 14 Oct 2026 09:30".into())],
        body: body.into(),
    }
}

/// Every object starts exactly where the cross-reference table says (on the bytes: the streams
/// are binary), and the file has its header and its end.
fn objects_are_where_the_xref_says(pdf: &[u8]) {
    assert!(pdf.starts_with(b"%PDF-1.7\n"));
    assert!(pdf.ends_with(b"%%EOF\n"));
    let find_last = |needle: &[u8]| pdf.windows(needle.len()).rposition(|w| w == needle);
    let sx = find_last(b"startxref\n").unwrap() + 10;
    let tail = String::from_utf8_lossy(&pdf[sx..]);
    let start: usize = tail.lines().next().unwrap().trim().parse().unwrap();
    assert!(pdf[start..].starts_with(b"xref"));
    let table = String::from_utf8_lossy(&pdf[start..sx]).to_string();
    let mut n = 0;
    for (i, line) in table.lines().skip(3).take_while(|l| l.ends_with(" n ")).enumerate() {
        let off: usize = line[..10].parse().unwrap();
        assert!(pdf[off..].starts_with(format!("{} 0 obj", i + 1).as_bytes()), "object {} at {off}", i + 1);
        n += 1;
    }
    assert!(n >= 4);
    assert!(String::from_utf8_lossy(&pdf[start..]).contains(&format!("trailer\n<< /Size {} /Root 1 0 R", n + 1)));
}

fn page_count(pdf: &[u8]) -> usize {
    let text = String::from_utf8_lossy(pdf);
    text.split("/Count ").nth(1).and_then(|r| r.split_whitespace().next()).and_then(|n| n.parse().ok()).unwrap()
}

#[test]
fn helvetica_pages() {
    let long = (0..400).map(|i| format!("Line {i}: the quick brown fox jumps over the lazy dog (café, “quotes”)")).collect::<Vec<_>>().join("\n");
    let p = pdf(&doc(&long), None);
    let text = String::from_utf8_lossy(&p);
    assert!(text.contains("/BaseFont /Helvetica"));
    assert!(page_count(&p) >= 8);
    assert!(text.contains("/Title (Storyboard \\(v2\\) review)"));
    assert!(text.contains("/Producer (MailALM)"));
    objects_are_where_the_xref_says(&p);
}

#[test]
fn wrapping() {
    let f = Face::Helvetica { bold: false };
    let lines = wrap(&f, "a b c", 11.0, 1000.0);
    assert_eq!(lines, vec!["a b c".to_string()]);
    let lines = wrap(&f, &"word ".repeat(100), 11.0, 200.0);
    assert!(lines.len() > 5 && lines.iter().all(|l| f.text_width(l, 11.0) <= 200.0));
    let lines = wrap(&f, &"x".repeat(500), 11.0, 100.0);
    assert!(lines.len() > 5 && lines.iter().all(|l| f.text_width(l, 11.0) <= 100.0), "long words are cut");
    assert_eq!(wrap(&f, "> quoted\n\n  indented", 11.0, 400.0), vec!["> quoted".to_string(), String::new(), "  indented".to_string()]);
    assert_eq!(win_ansi('é'), 0xE9);
    assert_eq!(win_ansi('€'), 0x80);
    assert_eq!(win_ansi('漢'), b'?');
}

#[test]
fn empty_and_hostile_input() {
    let p = pdf(&Doc::default(), None);
    objects_are_where_the_xref_says(&p);
    // Not a font: falls back to Helvetica rather than failing.
    let p = pdf(&doc("x"), Some(Fonts { regular: b"not a font", bold: None }));
    assert!(String::from_utf8_lossy(&p).contains("/Helvetica"));
    // Pages are capped, with a note.
    let huge = "y\n".repeat(200_000);
    let p = pdf(&doc(&huge), None);
    assert_eq!(page_count(&p), MAX_PAGES);
    objects_are_where_the_xref_says(&p);
    // A text over the cap, on one line, cut on a character's boundary.
    let wide = "é".repeat(MAX_BODY);
    let (cut, was_cut) = capped_body(&wide);
    assert!(was_cut && cut.len() <= MAX_BODY && cut.chars().all(|c| c == 'é'));
    // A header of many lines, and a title of one long word.
    let mut d = doc("body");
    d.header = (0..1000).map(|i| (format!("X-{i}"), "v".repeat(10_000))).collect();
    d.title = "t".repeat(100_000);
    objects_are_where_the_xref_says(&pdf(&d, None));
}

/// egui's Ubuntu Light is embedded, the text maps back to its characters, and bold is the same
/// font stroked (no second font file).
#[test]
fn embeds_the_font_given() {
    let data = epaint_default_fonts::UBUNTU_LIGHT;
    let p = pdf(&doc("Hello café — ünïcödé"), Some(Fonts { regular: data, bold: None }));
    let text = String::from_utf8_lossy(&p);
    assert!(text.contains("/Subtype /CIDFontType2") && text.contains("/FontFile2") && text.contains("/ToUnicode"));
    assert_eq!(text.matches("/FontFile2").count(), 1, "one font file");
    assert!(text.contains("/F1 ") && !text.contains("/F2 "));
    assert!(p.len() < data.len(), "the font file is compressed");
    objects_are_where_the_xref_says(&p);
}

/// A long word is cut asking each character's width once: its piece was measured again from its
/// start at every character, so a run of characters that take no room (accents), which never
/// fills a piece, was measured as many times over as it is long.
#[test]
fn a_word_is_cut_measuring_each_character_once() {
    let asked = std::cell::Cell::new(0usize);
    // Accents take no room; a letter half the size.
    let advance = |c: char| {
        asked.set(asked.get() + 1);
        if c == '\u{301}' { 0.0 } else { 500.0 }
    };
    let word: String = "\u{301}".repeat(3000) + &"a".repeat(7) + &"\u{301}".repeat(2000) + "bc";
    let mut pieces: Vec<(String, f32)> = Vec::new();
    // Size 10, 20 wide: four letters a piece.
    let rest = cut_word(&word, 10.0, 20.0, advance, |piece, w| {
        pieces.push((piece, w));
        true
    });
    assert_eq!(asked.get(), word.chars().count(), "each character's width asked once");
    pieces.extend(rest);
    let letters: Vec<usize> = pieces.iter().map(|(p, _)| p.chars().filter(|c| *c != '\u{301}').count()).collect();
    assert_eq!(letters, [4, 4, 1]);
    let widths: Vec<f32> = pieces.iter().map(|(_, w)| *w).collect();
    assert_eq!(widths, [20.0, 20.0, 5.0]);
    assert_eq!(pieces.iter().map(|(p, _)| p.as_str()).collect::<String>(), word, "nothing lost");
    // Told to stop at the first full piece: no more asked than the characters up to it.
    asked.set(0);
    assert!(cut_word(&word, 10.0, 20.0, advance, |_, _| false).is_none());
    assert_eq!(asked.get(), 3005);
}
