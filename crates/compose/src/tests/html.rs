use super::*;

fn para(kind: Kind, level: u8, quote: u8, runs: Vec<Run>) -> Paragraph {
    Paragraph { kind, level, quote, signature: false, runs }
}

#[test]
fn text_is_escaped() {
    let html = to_html(&doc(&["<script>alert(\"x\") & 'y'</script>"]));
    assert!(html.contains("&lt;script&gt;alert(&quot;x&quot;) &amp; &#39;y&#39;&lt;/script&gt;"), "{html}");
    assert!(!html.contains("<script"), "{html}");
}

#[test]
fn link_addresses_are_escaped_in_their_attribute() {
    let link = Link::parse("https://a.example/?q=\"x\"&r=<y>").unwrap();
    let d = Document::from_parts(vec![Paragraph::new(vec![Run::new("go", Format { link: Some(link), ..Format::default() })])], Vec::new());
    let html = to_html(&d);
    assert!(html.contains("<a href=\"https://a.example/?q=&quot;x&quot;&amp;r=&lt;y&gt;\">go</a>"), "{html}");
}

#[test]
fn marks_spaces_headings_and_empty_lines() {
    let red_bold_italic = Format { bold: true, italic: true, color: Some(Color::Red), ..Format::default() };
    let d = Document::from_parts(
        vec![
            para(Kind::Heading(2), 0, 0, vec![Run::plain("T")]),
            Paragraph::new(vec![Run::new("x", red_bold_italic), Run::new("u", Format { underline: true, strike: true, ..Format::default() })]),
            Paragraph::new(vec![Run::plain(" a  b\tc")]),
            Paragraph::default(),
        ],
        Vec::new(),
    );
    let html = to_html(&d);
    assert!(html.contains("<h2>T</h2>"), "{html}");
    assert!(html.contains("<b><i><span style=\"color:#cc0000\">x</span></i></b><u><s>u</s></u>"), "{html}");
    assert!(html.contains("&nbsp;a &nbsp;b&nbsp;&nbsp;&nbsp;&nbsp;c"), "{html}");
    assert!(html.contains("<p style=\"margin:0\"><br></p>"), "{html}");
    assert!(html.starts_with("<!DOCTYPE html>") && html.trim_end().ends_with("</body></html>"));
}

#[test]
fn lists_and_quotes_nest() {
    let d = Document::from_parts(
        vec![
            para(Kind::Bullet, 0, 0, vec![Run::plain("a")]),
            para(Kind::Bullet, 1, 0, vec![Run::plain("b")]),
            para(Kind::Bullet, 0, 0, vec![Run::plain("c")]),
            para(Kind::Numbered, 0, 0, vec![Run::plain("d")]),
            para(Kind::Normal, 0, 0, vec![Run::plain("e")]),
            para(Kind::Normal, 0, 1, vec![Run::plain("q")]),
        ],
        Vec::new(),
    );
    let html = to_html(&d);
    let lists = "<ul>\n<li>a<ul>\n<li>b</li>\n</ul>\n</li>\n<li>c</li>\n</ul>\n<ol>\n<li>d</li>\n</ol>\n<p style=\"margin:0\">e</p>\n";
    assert!(html.contains(lists), "{html}");
    assert!(html.contains("<blockquote style=\"margin:0 0 0 0.8ex;border-left:2px solid #cccccc;padding-left:1ex\">\n<p style=\"margin:0\">q</p>\n</blockquote>\n"), "{html}");
    // Every element opened is closed.
    for tag in ["ul", "ol", "li", "blockquote", "p"] {
        let opened = html.matches(&format!("<{tag}>")).count() + html.matches(&format!("<{tag} ")).count();
        assert_eq!(opened, html.matches(&format!("</{tag}>")).count(), "<{tag}> in {html}");
    }
}

#[test]
fn a_list_jumping_two_levels_stays_balanced() {
    let d = Document::from_parts(vec![para(Kind::Bullet, 0, 0, vec![Run::plain("a")]), para(Kind::Bullet, 3, 2, vec![Run::plain("b")]), Paragraph::new(vec![Run::plain("c")])], Vec::new());
    let html = to_html(&d);
    for tag in ["ul", "li", "blockquote"] {
        let opened = html.matches(&format!("<{tag}>")).count() + html.matches(&format!("<{tag} ")).count();
        assert_eq!(opened, html.matches(&format!("</{tag}>")).count(), "<{tag}> in {html}");
    }
}

#[test]
fn pictures_and_the_signature() {
    let image = InlineImage::with_content_id(png(800, 400), "pic@mailalm").unwrap();
    let mut sig = Paragraph::new(vec![Run::plain("Alex")]);
    sig.signature = true;
    let d = Document::from_parts(vec![Paragraph::image("pic@mailalm"), Paragraph::image("pic@mailalm"), sig], vec![image]);
    let html = to_html(&d);
    assert!(html.contains("<img src=\"cid:pic@mailalm\" alt=\"\" width=\"640\" height=\"320\""), "{html}");
    assert!(html.contains("<div class=\"mailalm-signature\">\n<p style=\"margin:0\">Alex</p>\n</div>\n"), "{html}");
    let body = html_body(&d);
    assert_eq!(body.images.len(), 1, "a picture shown twice is sent once");
    assert_eq!(body.images[0].content_id, "pic@mailalm");
    assert_eq!(body.images[0].content_type, "image/png");
    assert_eq!(body.html, html);
}

#[test]
fn the_plain_text_alternative() {
    let site = Format { link: Link::parse("https://x.example/"), ..Format::default() };
    let mail = Format { link: Link::parse("mailto:a@b.example"), ..Format::default() };
    let image = InlineImage::with_content_id(png(2, 2), "p@x").unwrap();
    let mut sig = Paragraph::new(vec![Run::plain("Alex")]);
    sig.signature = true;
    let d = Document::from_parts(
        vec![
            para(Kind::Bullet, 0, 0, vec![Run::plain("a")]),
            para(Kind::Bullet, 1, 0, vec![Run::plain("b")]),
            para(Kind::Numbered, 0, 0, vec![Run::plain("c")]),
            para(Kind::Numbered, 0, 0, vec![Run::plain("d")]),
            para(Kind::Heading(1), 0, 0, vec![Run::plain("e")]),
            para(Kind::Normal, 0, 1, vec![Run::plain("q")]),
            para(Kind::Normal, 0, 2, vec![]),
            Paragraph::new(vec![Run::plain("see "), Run::new("site", site.clone())]),
            Paragraph::new(vec![Run::new("https://x.example/", site)]),
            Paragraph::new(vec![Run::new("a@b.example", mail)]),
            Paragraph::image("p@x"),
            sig,
        ],
        vec![image],
    );
    let expected = ["- a", "  - b", "1. c", "2. d", "e", "> q", ">>", "see site <https://x.example/>", "https://x.example/", "a@b.example", "[image]", "-- ", "Alex"];
    assert_eq!(plain_text(&d), expected.join("\n"));
}

#[test]
fn hostile_html_keeps_only_text_and_known_formats() {
    let d = import_html(
        "<p>Hello <b>bold</b> <i>it</i></p><script>alert(1)</script><style>p{color:red}</style><p onclick=\"evil()\">x</p>",
        &[],
    );
    let texts: Vec<String> = d.paragraphs.iter().map(Paragraph::text).collect();
    assert_eq!(texts, ["Hello bold it", "x"]);
    let runs = &d.paragraphs[0].runs;
    assert!(runs.iter().any(|r| r.text.contains("bold") && r.format.bold));
    assert!(runs.iter().any(|r| r.text == "it" && r.format.italic));
    assert!(!runs.iter().any(|r| r.text.contains("Hello") && r.format.bold));
}

#[test]
fn dangerous_links_lose_their_address() {
    let d = import_html(
        "<p><a href=\"javascript:alert(1)\">a</a> <a href=\"&#106;avascript:alert(1)\">b</a> <a href=\"jav&#x09;ascript:x\">c</a> \
         <a href=\"data:text/html;base64,PHNjcmlwdD4=\">d</a> <a href=\" https://ok.example/ \">ok</a> <a href=\"vbscript:x\">e</a></p>",
        &[],
    );
    let p = &d.paragraphs[0];
    assert_eq!(p.text(), "a b c d ok e");
    let linked: Vec<(&str, &str)> = p.runs.iter().filter_map(|r| r.format.link.as_ref().map(|l| (r.text.as_str(), l.as_str()))).collect();
    assert_eq!(linked, [("ok", "https://ok.example/")]);
}

#[test]
fn remote_pictures_are_dropped_and_provided_ones_kept() {
    let logo = InlineImage::with_content_id(png(2, 2), "logo@x").unwrap();
    let d = import_html(
        "<p>a<img src=\"https://tracker.example/p.gif\">b<img src=\"http://x.example/logo.png\" alt=\"Logo\"></p>\
         <p><img src=\"cid:logo%40x\"></p><p><img src=\"cid:missing@x\"></p><p><img src=\"CID:logo@x\"></p>",
        &[logo.clone()],
    );
    let texts: Vec<String> = d.paragraphs.iter().map(Paragraph::text).collect();
    assert_eq!(texts, ["ab[Logo]", "\u{fffc}", "\u{fffc}"]);
    assert_eq!(d.images, [logo]);
}

#[test]
fn deep_and_huge_html_is_capped() {
    let deep = format!("{}deep{}", "<div><blockquote>".repeat(10_000), "</blockquote></div>".repeat(10_000));
    let d = import_html(&deep, &[]);
    assert_eq!(d.paragraphs.len(), 1);
    assert_eq!(d.paragraphs[0].text(), "deep");
    assert_eq!(d.paragraphs[0].quote, MAX_QUOTE);

    let lists = format!("{}x", "<ul><li>".repeat(1000));
    let d = import_html(&lists, &[]);
    assert_eq!(d.paragraphs[0].text(), "x");
    assert_eq!((d.paragraphs[0].kind.clone(), d.paragraphs[0].level), (Kind::Bullet, MAX_LEVEL));

    let huge = format!("<p>{}</p>", "a".repeat(5 << 20));
    let d = import_html(&huge, &[]);
    let size: usize = d.paragraphs.iter().map(|p| p.text().len()).sum();
    assert!(size > 0 && size <= 1 << 20, "{size}");

    let many = "<p>x</p>".repeat(30_000);
    assert!(import_html(&many, &[]).paragraphs.len() <= 20_000);
}

#[test]
fn entities_hidden_characters_and_skipped_elements() {
    let d = import_html("<p>&lt;b&gt; &amp; &eacute;&nbsp;x</p>", &[]);
    assert_eq!(d.paragraphs[0].text(), "<b> & é\u{a0}x");
    let d = import_html("<p>a\u{202e}b\u{fffc}c</p>", &[]);
    assert_eq!(d.paragraphs[0].text(), "abc");
    let d = import_html("<head><title>T</title></head><svg><text>hidden</text></svg><textarea>secret</textarea><p>ok<!-- comment -->!</p><object><p>in</p></object>", &[]);
    let texts: Vec<String> = d.paragraphs.iter().map(Paragraph::text).collect();
    assert_eq!(texts, ["ok!"]);
    let d = import_html("<p>a</p><script>never closed <b>x</b>", &[]);
    let texts: Vec<String> = d.paragraphs.iter().map(Paragraph::text).collect();
    assert_eq!(texts, ["a"]);
}

#[test]
fn blocks_lists_quotes_headings_and_line_breaks() {
    let d = import_html("<ul><li>a<ul><li>b</li></ul></li><li>c</li></ul><ol><li>d</li></ol>", &[]);
    let shape: Vec<(Kind, u8, String)> = d.paragraphs.iter().map(|p| (p.kind.clone(), p.level, p.text())).collect();
    assert_eq!(
        shape,
        [(Kind::Bullet, 0, "a".to_owned()), (Kind::Bullet, 1, "b".to_owned()), (Kind::Bullet, 0, "c".to_owned()), (Kind::Numbered, 0, "d".to_owned())]
    );
    let d = import_html("<blockquote>q1<blockquote>q2</blockquote></blockquote>after", &[]);
    let quotes: Vec<(u8, String)> = d.paragraphs.iter().map(|p| (p.quote, p.text())).collect();
    assert_eq!(quotes, [(1, "q1".to_owned()), (2, "q2".to_owned()), (0, "after".to_owned())]);
    let d = import_html("<h1>T</h1><h5>S</h5>", &[]);
    assert_eq!(d.paragraphs.iter().map(|p| p.kind.clone()).collect::<Vec<_>>(), [Kind::Heading(1), Kind::Heading(3)]);
    let d = import_html("a<br>b<br><br>c", &[]);
    assert_eq!(d.paragraphs.iter().map(Paragraph::text).collect::<Vec<_>>(), ["a", "b", "", "c"]);
    let d = import_html("<p>one<p>two", &[]);
    assert_eq!(d.paragraphs.iter().map(Paragraph::text).collect::<Vec<_>>(), ["one", "two"]);
    assert!(d.quoted().paragraphs.iter().all(|p| p.quote == 1), "a reply quotes what it imports");
    let d = import_html("<table><tr><td>a</td><td>b</td></tr><tr><td>c</td></tr></table>", &[]);
    assert_eq!(d.paragraphs.iter().map(Paragraph::text).collect::<Vec<_>>(), ["a b", "c"]);
}

#[test]
fn inline_styles_become_marks_and_palette_colours() {
    let d = import_html(
        "<p><span style=\"font-weight:bold;color:#ff0000\">x</span><font color=\"blue\">y</font><span style=\"font-style:italic;text-decoration:underline line-through\">z</span><b style=\"font-weight:normal\">n</b><span style=\"color:#333\">g</span></p>",
        &[],
    );
    let runs = &d.paragraphs[0].runs;
    // Runs of the same format are merged ("n" and "g"): each letter is looked up in its run.
    let f = |t: &str| runs.iter().find(|r| r.text.contains(t)).map(|r| r.format.clone()).unwrap();
    assert_eq!(f("x"), Format { bold: true, color: Some(Color::Red), ..Format::default() });
    assert_eq!(f("y"), Format { color: Some(Color::Blue), ..Format::default() });
    assert_eq!(f("z"), Format { italic: true, underline: true, strike: true, ..Format::default() });
    assert_eq!(f("n"), Format::default());
    assert_eq!(f("g"), Format::default(), "a dark grey is the text's own colour");
}

#[test]
fn what_is_exported_imports_back_the_same() {
    let image = InlineImage::with_content_id(png(10, 5), "pic@mailalm").unwrap();
    let link = Format { link: Link::parse("https://example.com/a?b=1&c=2"), ..Format::default() };
    let red = Format { color: Some(Color::Red), italic: true, ..Format::default() };
    let d = Document::from_parts(
        vec![
            para(Kind::Heading(1), 0, 0, vec![Run::plain("Title")]),
            Paragraph::new(vec![Run::plain("Hello "), Run::new("world", bold()), Run::plain(" & <you>")]),
            para(Kind::Bullet, 0, 0, vec![Run::plain("one")]),
            para(Kind::Bullet, 1, 0, vec![Run::plain("two")]),
            para(Kind::Numbered, 0, 0, vec![Run::plain("three")]),
            para(Kind::Normal, 0, 1, vec![Run::plain("quoted")]),
            Paragraph::new(vec![Run::plain("see "), Run::new("site", link)]),
            Paragraph::image("pic@mailalm"),
            Paragraph::new(vec![Run::new("red", red)]),
        ],
        vec![image.clone()],
    );
    let back = import_html(&to_html(&d), &[image]);
    assert_eq!(back.paragraphs, d.paragraphs);
    assert_eq!(back.images, d.images);
}

/// The reading pane's import: web pictures are listed (never fetched), with the size the message
/// declares; those the window fetched are shown where the message names them.
#[test]
fn web_pictures_are_listed_and_shown_once_given() {
    let html = "<p>Hi</p><img src=\" https://cdn.example/a.png \" alt=\"Logo\" width=\"120\" height=\"40px\">\
                <img src=\"http://t.example/o.gif\" width=\"1\" height=\"1\">\
                <img src=\"https://t.example/p.gif\" style=\"WIDTH: 2px; height:2px\">\
                <img src=\"https://t.example/h.gif\" style=\"display: none\">\
                <img src=\"https://cdn.example/a.png\">\
                <img src=\"https://cdn.example/w.png\" width=\"100%\">\
                <img src=\"javascript:x\"><img src=\"data:image/png;base64,AAAA\"><img src=\"cid:nothere\">";
    let first = import_message_html(html, &[], &[]);
    let listed: Vec<(&str, Option<u32>, Option<u32>)> = first.remote.iter().map(|r| (r.url.as_str(), r.width, r.height)).collect();
    assert_eq!(
        listed,
        [
            ("https://cdn.example/a.png", Some(120), Some(40)),
            ("http://t.example/o.gif", Some(1), Some(1)),
            ("https://t.example/p.gif", Some(2), Some(2)),
            ("https://t.example/h.gif", Some(0), Some(0)),
            ("https://cdn.example/w.png", None, None),
        ]
    );
    assert!(first.document.images.is_empty());
    assert!(first.document.paragraphs.iter().any(|p| p.text().contains("[Logo]")), "the alternative text stands in");

    let logo = InlineImage::with_content_id(png(120, 40), "remote-1.mailalm").unwrap();
    let second = import_message_html(html, &[], &[("https://cdn.example/a.png".to_owned(), logo.clone())]);
    assert_eq!(second.document.images, vec![logo]);
    assert_eq!(second.document.paragraphs.iter().filter(|p| p.kind == Kind::Image("remote-1.mailalm".into())).count(), 2, "shown where named");
    assert!(!second.remote.iter().any(|r| r.url == "https://cdn.example/a.png"), "given: no longer listed");
    // The plain import is the same document as before, without the list.
    assert_eq!(import_html(html, &[]), first.document);
}
