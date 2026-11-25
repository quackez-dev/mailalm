use super::*;

/// "Été 🍎" / a picture / "je " + "manges" in bold. Bytes: "Été 🍎" is 0..10 (É and é take 2,
/// the apple 4), the picture's empty line is 11..11, "je manges" is 12..21.
fn sample() -> Editor {
    let image = InlineImage::with_content_id(png(2, 2), "p@x").unwrap();
    Editor::from_document(Document::from_parts(
        vec![Paragraph::new(vec![Run::plain("Été 🍎")]), Paragraph::image("p@x"), Paragraph::new(vec![Run::plain("je "), Run::new("manges", bold())])],
        vec![image],
    ))
}

#[test]
fn the_text_maps_bytes_to_positions_and_back() {
    let ed = sample();
    let map = ed.text_map();
    assert_eq!(map.text, "Été 🍎\n\nje manges");
    assert_eq!(map.pos(15), Some(at(2, 3)));
    assert_eq!(map.pos(0), Some(at(0, 0)));
    assert_eq!(map.pos(10), Some(at(0, 5)), "the line feed is the end of the paragraph before");
    assert_eq!(map.pos(11), Some(at(1, 0)));
    assert_eq!(map.pos(21), Some(at(2, 9)));
    assert_eq!(map.pos(1), None, "inside É");
    assert_eq!(map.pos(7), None, "inside the apple");
    assert_eq!(map.pos(22), None);
    assert_eq!(map.byte(at(2, 3)), Some(15));
    assert_eq!(map.byte(at(1, 1)), Some(11));
    assert_eq!(map.byte(at(0, 6)), None);
    assert_eq!(map.byte(at(3, 0)), None);
    let formats = map.formats(ed.document());
    assert_eq!(formats, [(0..10, Format::default()), (12..15, Format::default()), (15..21, bold())]);
}

#[test]
fn a_replacement_keeps_the_format_of_the_text_it_replaces() {
    let mut ed = sample();
    assert!(ed.replace_text(15..21, "mange"));
    assert_eq!(ed.document().paragraphs[2].runs, [Run::plain("je "), Run::new("mange", bold())]);
    assert_eq!(caret(&ed), at(2, 8));
    // An insertion takes the format of the text before it.
    assert!(ed.replace_text(20..20, "s"));
    assert_eq!(ed.document().paragraphs[2].runs, [Run::plain("je "), Run::new("manges", bold())]);
    // Off a character, or backwards: refused, nothing changed.
    let before = ed.document().clone();
    assert!(!ed.replace_text(1..3, "x"));
    assert!(!ed.replace_text(15..12, "x"));
    assert!(!ed.replace_text(0..99, "x"));
    assert_eq!(ed.document(), &before);
    // One undo step.
    assert!(ed.undo());
    assert_eq!(ed.document().paragraphs[2].text(), "je mange");
    ed.check();
}

#[test]
fn a_replacement_may_span_paragraphs() {
    let mut ed = editor(&["one", "two"]);
    assert!(ed.replace_text(2..5, "E T"));
    assert_eq!(texts(&ed), ["onE Two"]);
    ed.check();
}

#[test]
fn selections_go_through_the_text_too() {
    let mut ed = sample();
    assert!(ed.select_text(12..14));
    assert_eq!(ed.selection().range(), (at(2, 0), at(2, 2)));
    assert_eq!(ed.selected_text_range(), Some(12..14));
    assert!(!ed.select_text(2..4));
    ed.set_selection(Selection::new(at(2, 9), at(0, 4)));
    assert_eq!(ed.selected_text_range(), Some(6..21));
}

#[test]
fn a_rich_message_round_trips_through_mime() {
    let mut ed = editor(&["Hello"]);
    ed.move_to_end(false);
    ed.insert_image(InlineImage::new(png(3, 2)).unwrap()).unwrap();
    ed.insert_text("after ");
    ed.toggle_mark(Mark::Bold);
    ed.insert_text("all");
    let d = ed.document().clone();

    let mut out = mailalm_mime::Outgoing::default();
    out.from.address = "alex@videotron.ca".into();
    let mut to = out.from.clone();
    to.address = "zoe@studio.example".into();
    out.to = vec![to];
    out.subject = "Rich".into();
    out.text = plain_text(&d);
    out.html = Some(html_body(&d));
    let (raw, _) = mailalm_mime::build(&out).unwrap();

    let parsed = mailalm_mime::parse(&raw).unwrap();
    for line in ["Hello", "[image]", "after all"] {
        assert!(parsed.text.contains(line), "{line} in {:?}", parsed.text);
    }
    let html = mailalm_mime::html_body(&raw).unwrap();
    let images: Vec<InlineImage> =
        mailalm_mime::related_images(&raw).into_iter().filter_map(|i| InlineImage::with_content_id(i.bytes, &i.content_id).ok()).collect();
    let back = import_html(&html, &images);
    assert_eq!(back.paragraphs, d.paragraphs);
    assert_eq!(back.images, d.images);
}
