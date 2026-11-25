use super::*;

#[test]
fn typing_moves_the_caret_and_line_feeds_make_paragraphs() {
    let mut ed = Editor::new();
    ed.insert_text("Hello");
    assert_eq!(caret(&ed), at(0, 5));
    ed.insert_text("\nWorld");
    assert_eq!(texts(&ed), ["Hello", "World"]);
    assert_eq!(caret(&ed), at(1, 5));
    // Control characters and direction overrides are not kept; the object character neither.
    ed.insert_text("\u{7}a\u{202e}b\u{fffc}");
    assert_eq!(texts(&ed), ["Hello", "Worldab"]);
    ed.check();
}

#[test]
fn backspace_and_delete_join_paragraphs() {
    let mut ed = editor(&["ab", "cd"]);
    ed.move_to(at(1, 0), false);
    ed.backspace();
    assert_eq!(texts(&ed), ["abcd"]);
    assert_eq!(caret(&ed), at(0, 2));
    ed.enter();
    assert_eq!(texts(&ed), ["ab", "cd"]);
    assert_eq!(caret(&ed), at(1, 0));
    ed.move_to(at(0, 2), false);
    ed.delete();
    assert_eq!(texts(&ed), ["abcd"]);
    assert_eq!(caret(&ed), at(0, 2));
    ed.backspace();
    assert_eq!(texts(&ed), ["acd"]);
    assert_eq!(caret(&ed), at(0, 1));
    ed.check();
}

#[test]
fn an_accent_or_an_emoji_sequence_goes_as_one_character() {
    let mut ed = Editor::new();
    ed.insert_text("e\u{301}👍🏽x");
    ed.move_left(false);
    ed.move_left(false);
    assert_eq!(caret(&ed), at(0, 2), "before the thumb and its skin tone");
    ed.backspace();
    assert_eq!(texts(&ed), ["👍🏽x"]);
    assert_eq!(caret(&ed), at(0, 0));
    ed.delete();
    assert_eq!(texts(&ed), ["x"]);
}

#[test]
fn a_selection_across_paragraphs_is_deleted() {
    let mut ed = editor(&["abc", "def", "ghi"]);
    ed.set_selection(Selection::new(at(2, 1), at(0, 1)));
    assert!(ed.delete_selection());
    assert_eq!(texts(&ed), ["ahi"]);
    assert_eq!(caret(&ed), at(0, 1));
    // Typing replaces a selection.
    ed.set_selection(Selection::new(at(0, 0), at(0, 2)));
    ed.insert_text("Z");
    assert_eq!(texts(&ed), ["Zi"]);
    assert_eq!(caret(&ed), at(0, 1));
    ed.check();
}

#[test]
fn enter_after_a_heading_gives_a_paragraph() {
    let mut ed = editor(&["Title"]);
    ed.toggle_heading(1);
    ed.move_to_end(false);
    ed.enter();
    let d = ed.document();
    assert_eq!(d.paragraphs[0].kind, Kind::Heading(1));
    assert_eq!(d.paragraphs[1].kind, Kind::Normal);
    assert_eq!(caret(&ed), at(1, 0));
    // A heading split in its middle stays a heading on both sides.
    ed.move_to(at(0, 2), false);
    ed.enter();
    assert_eq!(texts(&ed), ["Ti", "tle", ""]);
    assert_eq!(ed.document().paragraphs[1].kind, Kind::Heading(1));
}

#[test]
fn lists_go_on_and_an_empty_item_ends_them() {
    let mut ed = editor(&["one"]);
    ed.move_to_end(false);
    ed.toggle_list(ListKind::Bullets);
    ed.enter();
    ed.insert_text("two");
    ed.enter();
    let kinds: Vec<Kind> = ed.document().paragraphs.iter().map(|p| p.kind.clone()).collect();
    assert_eq!(kinds, [Kind::Bullet, Kind::Bullet, Kind::Bullet]);
    assert_eq!(caret(&ed), at(2, 0));
    // Enter on the empty item: the list ends there.
    ed.enter();
    assert_eq!(ed.document().paragraphs.len(), 3);
    assert_eq!(ed.document().paragraphs[2].kind, Kind::Normal);

    // Nested: an empty nested item goes up one level first.
    ed.toggle_list(ListKind::Bullets);
    assert!(ed.indent());
    assert_eq!(ed.document().paragraphs[2].level, 1);
    ed.enter();
    assert_eq!((ed.document().paragraphs[2].kind.clone(), ed.document().paragraphs[2].level), (Kind::Bullet, 0));
    ed.enter();
    assert_eq!(ed.document().paragraphs[2].kind, Kind::Normal);
    assert!(!ed.indent(), "only list items are indented");
    ed.check();
}

#[test]
fn tab_and_shift_tab_nest_list_items() {
    let mut ed = editor(&["a", "b"]);
    ed.select_all();
    ed.toggle_list(ListKind::Numbers);
    assert!(ed.indent());
    assert!(ed.indent());
    assert!(ed.document().paragraphs.iter().all(|p| p.level == 2 && p.kind == Kind::Numbered));
    assert!(ed.outdent());
    assert!(ed.outdent());
    assert!(ed.outdent(), "from the first level, out of the list");
    assert!(ed.document().paragraphs.iter().all(|p| p.kind == Kind::Normal && p.level == 0));
    // Toggling a list off.
    ed.toggle_list(ListKind::Bullets);
    ed.toggle_list(ListKind::Bullets);
    assert!(ed.document().paragraphs.iter().all(|p| p.kind == Kind::Normal));
}

#[test]
fn list_numbers_restart_after_other_paragraphs() {
    let mut d = doc(&["a", "b", "c", "x", "d"]);
    for i in [0, 1, 2, 4] {
        d.paragraphs[i].kind = Kind::Numbered;
    }
    d.paragraphs[1].level = 1;
    d.normalize();
    assert_eq!(d.list_numbers(), [Some(1), Some(1), Some(2), None, Some(1)]);
}

#[test]
fn marks_toggle_on_a_selection() {
    let mut ed = editor(&["abcd"]);
    ed.set_selection(Selection::new(at(0, 1), at(0, 3)));
    assert!(!ed.mark_active(Mark::Bold));
    ed.toggle_mark(Mark::Bold);
    let runs = &ed.document().paragraphs[0].runs;
    assert_eq!(runs.len(), 3);
    assert_eq!(runs[1].text, "bc");
    assert!(runs[1].format.bold && !runs[0].format.bold && !runs[2].format.bold);
    assert!(ed.mark_active(Mark::Bold));
    // A selection partly bold becomes all bold.
    ed.set_selection(Selection::new(at(0, 0), at(0, 2)));
    assert!(!ed.mark_active(Mark::Bold));
    ed.toggle_mark(Mark::Bold);
    assert_eq!(ed.document().paragraphs[0].runs[0].text, "abc");
    // And off again: one run.
    ed.select_all();
    ed.toggle_mark(Mark::Bold);
    ed.toggle_mark(Mark::Bold);
    assert_eq!(ed.document().paragraphs[0].runs.len(), 1);
    assert!(!ed.document().paragraphs[0].runs[0].format.bold);
    ed.set_color(Some(Color::Blue));
    assert_eq!(ed.document().paragraphs[0].runs[0].format.color, Some(Color::Blue));
    ed.clear_formatting();
    assert_eq!(ed.document().paragraphs[0].runs[0].format, Format::default());
    ed.check();
}

#[test]
fn a_mark_chosen_with_nothing_selected_is_for_the_next_text() {
    let mut ed = Editor::new();
    ed.toggle_mark(Mark::Bold);
    assert!(ed.mark_active(Mark::Bold));
    ed.insert_text("x");
    ed.insert_text("y");
    assert_eq!(ed.document().paragraphs[0].runs, [Run::new("xy", bold())]);
    ed.toggle_mark(Mark::Bold);
    ed.insert_text("z");
    assert_eq!(ed.document().paragraphs[0].runs, [Run::new("xy", bold()), Run::plain("z")]);
    // Moving the caret forgets a choice not used.
    ed.toggle_mark(Mark::Italic);
    ed.move_left(false);
    assert!(!ed.mark_active(Mark::Italic));
}

#[test]
fn undo_and_redo_group_typing() {
    let mut ed = Editor::new();
    for c in ["a", "b", "c"] {
        ed.insert_text(c);
    }
    ed.insert_text(" ");
    ed.insert_text("d");
    assert_eq!(texts(&ed), ["abc d"]);
    assert!(ed.undo());
    assert_eq!(texts(&ed), ["abc"]);
    assert!(ed.undo());
    assert_eq!(texts(&ed), [""]);
    assert!(!ed.undo());
    assert!(ed.redo());
    assert_eq!(texts(&ed), ["abc"]);
    assert_eq!(caret(&ed), at(0, 3));
    assert!(ed.redo());
    assert_eq!(texts(&ed), ["abc d"]);
    assert!(!ed.redo());
    // A new edit after an undo drops the redo.
    ed.undo();
    ed.enter();
    assert!(!ed.can_redo());
    ed.check();
}

#[test]
fn the_undo_history_is_capped() {
    let mut ed = editor(&["abcd"]);
    ed.select_all();
    for _ in 0..MAX_UNDO + 50 {
        ed.toggle_mark(Mark::Italic);
    }
    let mut n = 0;
    while ed.undo() {
        n += 1;
    }
    assert_eq!(n, MAX_UNDO);
}

#[test]
fn nothing_changed_is_not_an_undo_step() {
    let mut ed = editor(&["abc"]);
    ed.indent();
    ed.backspace();
    ed.move_to_end(false);
    ed.delete();
    assert!(!ed.can_undo());
}

#[test]
fn words_are_found_both_ways() {
    let ed = editor(&["hello world, again"]);
    assert_eq!(ed.word_right(at(0, 0)), at(0, 6));
    assert_eq!(ed.word_right(at(0, 6)), at(0, 11));
    assert_eq!(ed.word_right(at(0, 11)), at(0, 13));
    assert_eq!(ed.word_left(at(0, 18)), at(0, 13));
    assert_eq!(ed.word_left(at(0, 13)), at(0, 11));
    assert_eq!(ed.word_left(at(0, 0)), at(0, 0));
    let mut ed = editor(&["one two", "three"]);
    ed.select_word_at(at(0, 5));
    assert_eq!(ed.selection().range(), (at(0, 4), at(0, 7)));
    ed.move_to(at(1, 0), false);
    ed.move_word_left(false);
    assert_eq!(caret(&ed), at(0, 7), "to the end of the paragraph before");
    ed.move_to(at(0, 4), false);
    ed.delete_word_forward();
    assert_eq!(texts(&ed), ["one ", "three"]);
    ed.delete_word_backward();
    assert_eq!(texts(&ed), ["", "three"]);
}

#[test]
fn links_take_only_safe_schemes() {
    assert_eq!(Link::parse("javascript:alert(1)"), None);
    assert_eq!(Link::parse("JavaScript:alert(1)"), None);
    assert_eq!(Link::parse("data:text/html,<script>"), None);
    assert_eq!(Link::parse("vbscript:x"), None);
    assert_eq!(Link::parse("file:///C:/x"), None);
    assert_eq!(Link::parse("http://"), None);
    assert_eq!(Link::parse("https://a b.example"), None);
    assert_eq!(Link::parse("https://x.example/\u{202e}fdp.exe"), None);
    assert_eq!(Link::parse("mailto:"), None);
    assert_eq!(Link::parse(" HTTPS://Example.com/x ").map(|l| l.as_str().to_owned()).as_deref(), Some("https://Example.com/x"));
    assert_eq!(Link::parse("www.example.org").map(|l| l.as_str().to_owned()).as_deref(), Some("https://www.example.org"));
    assert!(Link::parse("mailto:zoe@studio.example").is_some());
    assert!(Link::parse(&format!("https://x.example/{}", "a".repeat(Link::MAX_LEN))).is_none());
}

#[test]
fn links_are_set_extended_and_removed() {
    let mut ed = editor(&["see site now"]);
    let link = Link::parse("https://site.example/").unwrap();
    ed.set_selection(Selection::new(at(0, 4), at(0, 8)));
    assert!(ed.set_link(link.clone()));
    assert_eq!(ed.link_at_caret(), Some(link.clone()));
    ed.move_to(at(0, 6), false);
    assert_eq!(ed.link_at_caret(), Some(link.clone()));
    // Typing right after a link does not extend it.
    ed.move_to(at(0, 8), false);
    ed.insert_text("X");
    let p = &ed.document().paragraphs[0];
    assert_eq!(p.text(), "see siteX now");
    assert_eq!(p.runs[1], Run::new("site", Format { link: Some(link.clone()), ..Format::default() }));
    // With the caret inside, the whole link changes, then goes.
    let other = Link::parse("https://other.example/").unwrap();
    ed.move_to(at(0, 5), false);
    assert!(ed.set_link(other.clone()));
    assert_eq!(ed.document().paragraphs[0].runs[1].format.link, Some(other));
    ed.remove_link();
    assert!(ed.document().paragraphs[0].runs.iter().all(|r| r.format.link.is_none()));
    assert!(!ed.set_link(link.clone()), "no selection and no link at the caret");
    // A link inserted with its text.
    ed.move_to_end(false);
    ed.insert_link(" docs", link.clone());
    assert!(ed.document().paragraphs[0].text().ends_with(" docs"));
    assert_eq!(ed.link_at_caret(), Some(link));
    ed.check();
}

#[test]
fn pictures_are_checked_by_their_content() {
    let p = InlineImage::new(png(4, 3)).unwrap();
    assert_eq!((p.content_type, p.width, p.height), ("image/png", 4, 3));
    let g = InlineImage::new(gif(5, 6)).unwrap();
    assert_eq!((g.content_type, g.width, g.height), ("image/gif", 5, 6));
    let j = InlineImage::new(jpeg(7, 8)).unwrap();
    assert_eq!((j.content_type, j.width, j.height), ("image/jpeg", 7, 8));
    assert_ne!(p.content_id, g.content_id);
    assert!(is_valid_content_id(&p.content_id));
    assert_eq!(InlineImage::new(b"GIF8".to_vec()), Err(ImageError::Unsupported));
    assert_eq!(InlineImage::new(b"<svg onload=alert(1)>".to_vec()), Err(ImageError::Unsupported));
    assert_eq!(InlineImage::new(png(0, 5)), Err(ImageError::Unsupported));
    assert_eq!(InlineImage::new(png(20_000, 10)), Err(ImageError::TooManyPixels));
    assert_eq!(InlineImage::new(png(10_000, 10_000)), Err(ImageError::TooManyPixels));
    assert_eq!(InlineImage::new(vec![0; MAX_IMAGE_BYTES + 1]), Err(ImageError::TooLarge));
    assert_eq!(InlineImage::with_content_id(png(1, 1), "bad id\"x"), Err(ImageError::BadContentId));
    assert_eq!(InlineImage::with_content_id(png(1, 1), "<ok@x>").map(|i| i.content_id), Ok("ok@x".to_owned()));
    // A JPEG cut before its frame header.
    let mut cut = jpeg(7, 8);
    cut.truncate(22);
    assert_eq!(InlineImage::new(cut), Err(ImageError::Unsupported));
    assert!(format!("{p:?}").len() < 200, "Debug leaves the content out");
}

#[test]
fn a_picture_goes_in_a_paragraph_of_its_own() {
    let mut ed = editor(&["abcd"]);
    ed.move_to(at(0, 2), false);
    ed.insert_image(InlineImage::new(png(2, 2)).unwrap()).unwrap();
    assert_eq!(texts(&ed), ["ab", "\u{fffc}", "cd"]);
    assert!(ed.document().paragraphs[1].kind.is_image());
    assert_eq!(ed.document().images.len(), 1);
    assert_eq!(caret(&ed), at(2, 0));
    ed.check();
    // Backspace after it removes it (and its bytes).
    ed.backspace();
    assert_eq!(texts(&ed), ["ab", "cd"]);
    assert!(ed.document().images.is_empty());
    assert_eq!(caret(&ed), at(1, 0));

    // At the end of the text: an empty paragraph follows for the caret.
    let mut ed = editor(&["ab"]);
    ed.move_to_end(false);
    ed.insert_image(InlineImage::new(png(2, 2)).unwrap()).unwrap();
    assert_eq!(texts(&ed), ["ab", "\u{fffc}", ""]);
    assert_eq!(caret(&ed), at(2, 0));
    // Typing before the picture makes a paragraph before it.
    ed.move_to(at(1, 0), false);
    ed.insert_text("x");
    assert_eq!(texts(&ed), ["ab", "x", "\u{fffc}", ""]);
    assert_eq!(caret(&ed), at(1, 1));
    // Enter after the picture makes a paragraph after it.
    ed.move_to(at(2, 1), false);
    ed.enter();
    assert_eq!(texts(&ed), ["ab", "x", "\u{fffc}", "", ""]);
    assert_eq!(caret(&ed), at(3, 0));
    ed.check();

    // A selection over a picture deletes it, and the text around joins.
    let mut ed = editor(&["abcd"]);
    ed.move_to(at(0, 2), false);
    ed.insert_image(InlineImage::new(png(2, 2)).unwrap()).unwrap();
    ed.set_selection(Selection::new(at(0, 1), at(2, 1)));
    ed.delete_selection();
    assert_eq!(texts(&ed), ["ad"]);
    assert!(ed.document().images.is_empty());
    ed.check();
}

#[test]
fn a_message_holds_a_limited_number_of_pictures() {
    let mut ed = Editor::new();
    for _ in 0..MAX_IMAGES {
        ed.insert_image(InlineImage::new(png(1, 1)).unwrap()).unwrap();
    }
    assert_eq!(ed.insert_image(InlineImage::new(png(1, 1)).unwrap()), Err(ImageError::TooMany));
    assert_eq!(ed.document().images.len(), MAX_IMAGES);
    ed.check();
}

#[test]
fn copy_and_paste_keep_the_formatting_inside_the_editor() {
    let mut ed = editor(&["plain bold"]);
    ed.set_selection(Selection::new(at(0, 6), at(0, 10)));
    ed.toggle_mark(Mark::Bold);
    assert_eq!(ed.copy().as_deref(), Some("bold"));
    ed.move_to_end(false);
    ed.paste_text("bold");
    assert_eq!(ed.document().paragraphs[0].runs, [Run::plain("plain "), Run::new("boldbold", bold())]);
    assert_eq!(caret(&ed), at(0, 14));
    // Other text is pasted as plain text, line by line.
    ed.paste_text("x\r\ny");
    assert_eq!(texts(&ed), ["plain boldboldx", "y"]);
    // Cut.
    ed.set_selection(Selection::new(at(0, 0), at(1, 0)));
    assert_eq!(ed.cut().as_deref(), Some("plain boldboldx\n"));
    assert_eq!(texts(&ed), ["y"]);
    ed.check();
}

#[test]
fn pasted_html_is_imported_at_the_caret() {
    let mut ed = editor(&["start end"]);
    ed.move_to(at(0, 6), false);
    ed.paste_html("<b>bold</b><p>second</p><script>x()</script>", &[]);
    assert_eq!(texts(&ed), ["start bold", "secondend"]);
    assert!(ed.document().paragraphs[0].runs.iter().any(|r| r.text == "bold" && r.format.bold));
    assert_eq!(caret(&ed), at(1, 6));
    ed.check();
}

#[test]
fn the_signature_is_replaced_where_it_is() {
    let mut ed = editor(&["Hi", "", "> quoted"]);
    ed.set_signature(Some(&doc(&["--", "Alex"])));
    assert_eq!(texts(&ed), ["Hi", "", "> quoted", "--", "Alex"]);
    assert!(ed.document().paragraphs[3].signature && ed.document().paragraphs[4].signature);
    // Moved before the quote, then replaced in place.
    let mut d = ed.document().clone();
    let sig: Vec<Paragraph> = d.paragraphs.drain(3..).collect();
    d.paragraphs.splice(1..1, sig);
    ed.set_document(d);
    ed.set_signature(Some(&doc(&["Bob"])));
    assert_eq!(texts(&ed), ["Hi", "Bob", "", "> quoted"]);
    ed.set_signature(None);
    assert_eq!(texts(&ed), ["Hi", "", "> quoted"]);
    // Enter in the signature stays in it.
    ed.set_signature(Some(&doc(&["Bob"])));
    ed.move_to(at(3, 3), false);
    ed.enter();
    assert!(ed.document().paragraphs[4].signature);
    ed.check();
}

#[test]
fn quotes_toggle_and_end_like_lists() {
    let mut ed = editor(&["x", "y"]);
    ed.move_to(at(1, 0), false);
    ed.toggle_quote();
    assert_eq!(ed.document().paragraphs[1].quote, 1);
    // Backspace at the start of a quote takes the quote off before joining.
    ed.backspace();
    assert_eq!(ed.document().paragraphs[1].quote, 0);
    ed.backspace();
    assert_eq!(texts(&ed), ["xy"]);
    // Enter on an empty quoted paragraph takes it out of the quote.
    let mut ed = editor(&[""]);
    ed.toggle_quote();
    ed.enter();
    assert_eq!(ed.document().paragraphs.len(), 1);
    assert_eq!(ed.document().paragraphs[0].quote, 0);
    // Backspace at the start of a list item makes it a paragraph.
    let mut ed = editor(&["a"]);
    ed.toggle_list(ListKind::Bullets);
    ed.backspace();
    assert_eq!(ed.document().paragraphs[0].kind, Kind::Normal);
}

/// A small deterministic random source.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> usize {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        usize::try_from(self.0 >> 33).unwrap()
    }

    fn below(&mut self, n: usize) -> usize {
        self.next() % n.max(1)
    }

    fn pos(&mut self, ed: &Editor) -> Pos {
        let d = ed.document();
        let para = self.below(d.paragraphs.len());
        Pos::new(para, self.below(d.paragraph_len(para) + 1))
    }
}

#[test]
fn random_edits_keep_the_document_valid_and_undo_back_to_the_start() {
    const TEXTS: [&str; 10] = ["a", "é", "🍎", "e\u{301}", " ", "word", "x\ny", "\t", "👍🏽", "\n"];
    for seed in 0..12u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15).wrapping_add(1));
        let mut start = doc(&["First line", "Second", "", "Third one"]);
        start.paragraphs[1].kind = Kind::Bullet;
        start.paragraphs[3].quote = 1;
        start.normalize();
        let mut ed = Editor::from_document(start.clone());
        for step in 0..120 {
            match rng.below(24) {
                0..=4 => ed.insert_text(TEXTS[rng.below(TEXTS.len())]),
                5 => ed.backspace(),
                6 => ed.delete(),
                7 => ed.enter(),
                8 => {
                    let (a, b) = (rng.pos(&ed), rng.pos(&ed));
                    ed.set_selection(Selection::new(a, b));
                }
                9 => {
                    let p = rng.pos(&ed);
                    ed.move_to(p, rng.below(2) == 0);
                }
                10 => ed.move_left(rng.below(2) == 0),
                11 => ed.move_word_right(rng.below(2) == 0),
                12 => ed.toggle_mark([Mark::Bold, Mark::Italic, Mark::Underline, Mark::Strike][rng.below(4)]),
                13 => ed.toggle_list(if rng.below(2) == 0 { ListKind::Bullets } else { ListKind::Numbers }),
                14 => {
                    if rng.below(2) == 0 {
                        ed.indent();
                    } else {
                        ed.outdent();
                    }
                }
                15 => ed.toggle_quote(),
                16 => {
                    let _ = ed.insert_image(InlineImage::new(png(3, 3)).unwrap());
                }
                17 => ed.delete_word_backward(),
                18 => {
                    if let Some(text) = ed.copy() {
                        let p = rng.pos(&ed);
                        ed.move_to(p, false);
                        ed.paste_text(&text);
                    }
                }
                19 => {
                    if !ed.selection().is_empty() {
                        ed.set_link(Link::parse("https://x.example/").unwrap());
                    } else {
                        ed.remove_link();
                    }
                }
                20 => {
                    let map = ed.text_map();
                    let (a, b) = (rng.below(map.text.len() + 1), rng.below(map.text.len() + 1));
                    let _ = ed.replace_text(a.min(b)..a.max(b), "fix");
                }
                21 => ed.set_color(Some(Color::ALL[rng.below(Color::ALL.len())])),
                22 => {
                    ed.undo();
                }
                _ => {
                    ed.redo();
                }
            }
            ed.check();
            let map = ed.text_map();
            for (i, p) in ed.document().paragraphs.iter().enumerate() {
                let byte = map.byte(at(i, 0)).unwrap_or_else(|| panic!("seed {seed} step {step}: no byte for paragraph {i}"));
                if !p.kind.is_image() {
                    assert_eq!(map.pos(byte), Some(at(i, 0)), "seed {seed} step {step}");
                    assert_eq!(map.byte(at(i, p.len())).and_then(|b| map.pos(b)), Some(at(i, p.len())), "seed {seed} step {step}");
                }
            }
            // Exporting never fails, whatever the document.
            let _ = to_html(ed.document());
            let _ = plain_text(ed.document());
        }
        while ed.undo() {}
        assert_eq!(ed.document(), &start, "seed {seed}: undo all the way gives the start back");
    }
}

/// A new signature goes above a reply's quoted text and its attribution line, the first two
/// lines staying above it; with nothing quoted, at the end.
#[test]
fn a_new_signature_goes_above_the_quote() {
    let mut d = doc(&["", "", "", "On Monday, Zoe wrote:", "a", "b"]);
    for p in d.paragraphs.iter_mut().skip(4) {
        p.quote = 1;
    }
    assert_eq!(signature_slot(&d), 2);
    let mut ed = Editor::from_document(d);
    ed.set_signature(Some(&doc(&["Alex", "Videotron"])));
    assert_eq!(texts(&ed), ["", "", "Alex", "Videotron", "", "On Monday, Zoe wrote:", "a", "b"]);
    assert!(ed.document().paragraphs[2].signature && ed.document().paragraphs[3].signature && !ed.document().paragraphs[4].signature);
    // Switched to another account's: in the same place.
    ed.set_signature(Some(&doc(&["Bob"])));
    assert_eq!(texts(&ed), ["", "", "Bob", "", "On Monday, Zoe wrote:", "a", "b"]);
    // A forward's quote without an attribution line; text typed above it stays above.
    let mut d = doc(&["Hello", "", "From: Zoe", "Hi"]);
    for p in d.paragraphs.iter_mut().skip(2) {
        p.quote = 1;
    }
    assert_eq!(signature_slot(&d), 2);
    assert_eq!(signature_slot(&doc(&["Hi", "there", "you"])), 3);
    ed.check();
}

/// What a reader sees of a link: its target and all the text that carries it.
#[test]
fn the_link_under_a_character() {
    let site = Format { link: Link::parse("https://bank.example/"), ..Format::default() };
    let bold_site = Format { bold: true, ..site.clone() };
    let other = Format { link: Link::parse("https://other.example/"), ..Format::default() };
    let d = Document::from_parts(
        vec![Paragraph::new(vec![Run::plain("see "), Run::new("my ", site.clone()), Run::new("bank", bold_site), Run::new("x", other), Run::plain(" ok")])],
        Vec::new(),
    );
    let ed = Editor::from_document(d);
    let (link, text) = ed.link_at(at(0, 8)).unwrap();
    assert_eq!((link.as_str(), text.as_str()), ("https://bank.example/", "my bank"));
    assert_eq!(ed.link_at(at(0, 4)).map(|(_, t)| t).as_deref(), Some("my bank"));
    assert_eq!(ed.link_at(at(0, 11)).map(|(l, t)| (l.as_str().to_owned(), t)), Some(("https://other.example/".to_owned(), "x".to_owned())));
    assert_eq!(ed.link_at(at(0, 2)), None, "plain text");
    assert_eq!(ed.link_at(at(0, 99)), None);
    assert_eq!(ed.link_at(at(3, 0)), None);
}
