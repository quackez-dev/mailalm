// Adapted from MailCraft (https://github.com/ZIDAN44/mailcraft), commit c3510d13, crates/net/src/oauth.rs (its tests). MIT OR Apache-2.0.
// Changed: Microsoft's provider and redirect (http://localhost), Google's (http://127.0.0.1, a client secret, no scope in token requests); tokens, codes and the client secret checked to stay out of errors; the client id from the build only.

use super::*;
use crate::http::tests::serve;

/// A provider whose token endpoint is a local test server.
fn local(token_url: &str) -> Provider {
    Provider {
        name: "Test".into(),
        auth_url: "http://127.0.0.1:9/auth".into(),
        token_url: token_url.into(),
        scope: "mail offline_access".into(),
        client_id: "mailalm-test".into(),
        client_secret: None,
        redirect_host: "localhost",
        extra_auth: "&response_mode=query",
        refresh_scope: true,
    }
}

fn json(status: &str, body: &str) -> Vec<u8> {
    format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).into_bytes()
}

/// A parameter of a URL's query.
fn param(url: &str, k: &str) -> String {
    query_params(url).into_iter().find(|(n, _)| n == k).map(|(_, v)| v).unwrap_or_default()
}

/// A "browser" that goes back to the redirect with `query` (the state filled in), after a stray
/// request for a favicon.
fn browser(query: &'static str) -> impl FnMut(&str) -> std::result::Result<(), String> {
    move |url: &str| {
        let redirect = param(url, "redirect_uri");
        let state = param(url, "state");
        std::thread::spawn(move || {
            let _ = crate::http::send(&Request::get(&format!("{redirect}/favicon.ico"), 100));
            let _ = crate::http::send(&Request::get(&format!("{redirect}/?{}", query.replace("{state}", &state)), 10_000));
        });
        Ok(())
    }
}

#[test]
fn pkce_and_helpers() {
    let (v, c) = pkce().unwrap();
    assert!(v.len() == 43 && !v.contains(['+', '/', '=']), "{v}");
    assert_eq!(c, challenge_of(&v));
    // RFC 7636 appendix B.
    assert_eq!(challenge_of("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    assert_eq!(query_params("/?code=4%2F0A&state=s+1"), vec![("code".to_string(), "4/0A".to_string()), ("state".into(), "s 1".into())]);
    assert_eq!(unpct("%zz%4"), "%zz%4");
    assert_eq!(unpct("%+5x"), "%+5x");
}

/// The client id comes from the build, trimmed; none, or blank, offers no Microsoft sign-in.
#[test]
fn microsoft_needs_a_client_id() {
    assert!(microsoft_with(None).is_none() && microsoft_with(Some("  ")).is_none());
    let p = microsoft_with(Some(" 0000-id \n")).unwrap();
    assert_eq!(p.client_id, "0000-id");
    assert!(p.auth_url.starts_with("https://login.microsoftonline.com/consumers/") && p.token_url.ends_with("/oauth2/v2.0/token"));
    assert!(p.scope.contains("Mail.ReadWrite") && p.scope.ends_with("offline_access"));
    // Phase 6: the calendars, To Do and the contacts, delegated, by their full names.
    let scopes: Vec<&str> = p.scope.split_whitespace().collect();
    for s in ["Calendars.ReadWrite", "Tasks.ReadWrite", "Contacts.ReadWrite"] {
        assert!(scopes.contains(&format!("https://graph.microsoft.com/{s}").as_str()), "{s}");
    }
    assert_eq!(scopes.len(), 8, "{scopes:?}");
}

/// Google: mail, filters, the calendars, the tasks, and the contacts read only.
#[test]
fn google_scope_names_each_api() {
    let scopes: Vec<&str> = GOOGLE_SCOPE.split_whitespace().collect();
    assert_eq!(
        scopes,
        [
            "https://www.googleapis.com/auth/gmail.modify",
            "https://www.googleapis.com/auth/gmail.settings.basic",
            "https://www.googleapis.com/auth/calendar",
            "https://www.googleapis.com/auth/tasks",
            "https://www.googleapis.com/auth/contacts.readonly",
        ]
    );
}

/// Who signed in, from an ID token's claims.
#[test]
fn id_token_user_is_read() {
    let token = |claims: &str| format!("e30.{}.sig", crate::base64::encode_url(claims.as_bytes()));
    assert_eq!(
        id_token_user(&token(r#"{"preferred_username":"Zoe@Live.example","email":"zoe@outlook.example"}"#)).as_deref(),
        Some("zoe@outlook.example")
    );
    assert_eq!(id_token_user(&token(r#"{"preferred_username":"zoe@live.example"}"#)).as_deref(), Some("zoe@live.example"));
    assert_eq!(id_token_user(&token(r#"{"email":"not an address","preferred_username":"z@x.example"}"#)).as_deref(), Some("z@x.example"));
    assert_eq!(id_token_user(&token(r#"{"email":"z@x\u001b.example"}"#)), None);
    assert_eq!(id_token_user(&token(r#"{"name":"Zoe"}"#)), None);
    for junk in ["", "a", "a.b.c", "a.!!!.c", "a.e30"] {
        assert_eq!(id_token_user(junk), None, "{junk}");
    }
    assert_eq!(id_token_user(&format!("e30.{}.sig", "A".repeat(MAX_ID_TOKEN))), None);
}

#[test]
fn browser_sign_in_then_refresh() {
    let id_token = format!("e30.{}.sig", crate::base64::encode_url(br#"{"email":"zoe@outlook.example"}"#));
    let first = format!(r#"{{"access_token":"good-token","refresh_token":"r1","expires_in":3600,"id_token":"{id_token}"}}"#);
    let (base, seen) = serve(vec![
        json("200 OK", &first),
        json("200 OK", r#"{"access_token":"fresh","refresh_token":"rt-two","expires_in":"3600"}"#),
        json("400 Bad Request", r#"{"error":"invalid_grant","error_description":"AADSTS70000: the grant rt-two was revoked"}"#),
    ]);
    let p = local(&format!("{base}/token"));
    let mut opened = String::new();
    let mut go = browser("code=the-code&state={state}");
    let mut open = |url: &str| {
        opened = url.to_string();
        go(url)
    };
    let t = authorize(&p, "zoe@outlook.example", &mut open, Duration::from_secs(20)).unwrap();
    assert_eq!((t.access.as_str(), t.refresh.as_deref(), t.user.as_deref()), ("good-token", Some("r1"), Some("zoe@outlook.example")));
    let redirect = param(&opened, "redirect_uri");
    assert!(redirect.starts_with("http://localhost:") && !redirect.ends_with('/'), "{redirect}");
    assert!(opened.contains("code_challenge_method=S256") && opened.contains("login_hint=zoe%40outlook.example"), "{opened}");
    assert_eq!(param(&opened, "scope"), "mail offline_access openid email profile");
    assert_eq!(format!("{t:?}"), "Tokens(<redacted>)");

    let t = refresh(&p, "r1").unwrap();
    // Expiry is wall-clock time (it goes on while the computer sleeps), a minute early.
    assert!((now_unix() + 3500..=now_unix() + 3540).contains(&t.expires_at), "{}", t.expires_at);
    assert!(!t.expired() && Tokens { expires_at: now_unix() - 1, ..t.clone() }.expired());
    assert_eq!((t.access.as_str(), t.refresh.as_deref(), t.user), ("fresh", Some("rt-two"), None));
    // A refusal asks to sign in again, and never repeats the token sent.
    let e = refresh(&p, "rt-two").unwrap_err();
    assert!(matches!(&e, NetError::Auth(m) if m.contains("AADSTS70000: the grant *** was revoked")), "{e}");

    let seen = seen.join().unwrap();
    assert!(seen[0].starts_with("POST /token "), "{}", seen[0]);
    for field in ["grant_type=authorization_code", "code=the-code", "code_verifier=", "client_id=mailalm-test", "redirect_uri=http%3A%2F%2Flocalhost%3A"] {
        assert!(seen[0].contains(field), "{field}: {}", seen[0]);
    }
    // The browser sign-in asks who signed in; refreshing asks for the permissions alone.
    assert!(seen[0].contains("scope=mail%20offline_access%20openid%20email%20profile"), "{}", seen[0]);
    assert!(seen[1].contains("grant_type=refresh_token&refresh_token=r1") && seen[1].ends_with("scope=mail%20offline_access"), "{}", seen[1]);
}

/// A denied sign-in is an error, not a hang.
#[test]
fn a_denied_sign_in_is_refused() {
    let p = local("http://127.0.0.1:9/token");
    let mut deny = browser("error=access_denied&error_description=The+user+said+no&state={state}");
    let e = authorize(&p, "", &mut deny, Duration::from_secs(20)).unwrap_err();
    assert!(matches!(&e, NetError::Auth(m) if m.contains("access_denied") && m.contains("The user said no")), "{e}");
    // A browser that never comes back.
    let mut nothing = |_: &str| -> std::result::Result<(), String> { Ok(()) };
    assert!(matches!(authorize(&p, "", &mut nothing, Duration::from_millis(300)), Err(NetError::Auth(m)) if m.contains("in time")));
    // A browser that can't be opened.
    let mut broken = |_: &str| -> std::result::Result<(), String> { Err("no browser".into()) };
    assert!(authorize(&p, "", &mut broken, Duration::from_secs(5)).is_err());
}

/// A connection that sends nothing (a browser's speculative one), half a line, or the wrong
/// state is passed over: the redirect waiting behind it still signs in, and only its code counts.
#[test]
fn stray_connections_dont_fail_the_sign_in() {
    let (base, seen) = serve(vec![json("200 OK", r#"{"access_token":"good-token","refresh_token":"r1","expires_in":3600}"#)]);
    let p = local(&format!("{base}/token"));
    let mut held = Vec::new();
    let mut browser = |url: &str| -> std::result::Result<(), String> {
        let (redirect, state) = (param(url, "redirect_uri"), param(url, "state"));
        let port: u16 = redirect.rsplit(':').next().and_then(|p| p.parse().ok()).ok_or("no port")?;
        let addr = ("127.0.0.1", port);
        // Silent, and kept open until the sign-in is over.
        held.push(TcpStream::connect(addr).map_err(|e| e.to_string())?);
        // Half a request line (with a code of its own), then gone.
        let mut half = TcpStream::connect(addr).map_err(|e| e.to_string())?;
        half.write_all(b"GET /?code=stray-code&sta").map_err(|e| e.to_string())?;
        drop(half);
        std::thread::spawn(move || {
            let _ = crate::http::send(&Request::get(&format!("{redirect}/?code=forged&state=not-ours"), 10_000));
            let _ = crate::http::send(&Request::get(&format!("{redirect}/?code=the-code&state={state}"), 10_000));
        });
        Ok(())
    };
    let started = Instant::now();
    let t = authorize_with(&p, "", &mut browser, Duration::from_secs(20), Duration::from_millis(300)).unwrap();
    assert_eq!(t.access, "good-token");
    assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
    let seen = seen.join().unwrap();
    assert!(seen[0].contains("code=the-code") && !seen[0].contains("stray-code") && !seen[0].contains("forged"), "{}", seen[0]);
    drop(held);
}

/// A browser's request is read to the end of its headers before it is answered (they come in
/// more than one read): answered and closed with part of it unread, the connection can be reset
/// and the page lost. Headers that never end don't hold the answer back for good.
#[test]
fn a_request_is_read_to_the_end_of_its_headers() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let serve = |wait: Duration| {
        let listener = listener.try_clone().unwrap();
        std::thread::spawn(move || {
            let (sock, _) = listener.accept().unwrap();
            let line = request_line(&sock, wait);
            (&sock).write_all(b"answered").unwrap();
            line
        })
    };
    let server = serve(Duration::from_secs(30));
    let mut browser = TcpStream::connect(addr).unwrap();
    browser.write_all(b"GET /?code=c&state=s HTTP/1.1\r\nHost: localhost\r\n").unwrap();
    // Nothing is answered while headers are still to come.
    browser.set_read_timeout(Some(Duration::from_millis(300))).unwrap();
    let mut early = [0u8; 8];
    let e = browser.read(&mut early).unwrap_err();
    assert!(matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut), "{e}");
    // The rest, several reads long, its empty line split in two.
    browser.write_all(format!("Cookie: {}\r\n\r", "c".repeat(5000)).as_bytes()).unwrap();
    browser.write_all(b"\n").unwrap();
    browser.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let mut answer = String::new();
    browser.read_to_string(&mut answer).unwrap();
    assert_eq!(answer, "answered");
    assert_eq!(server.join().unwrap().as_deref(), Some("GET /?code=c&state=s HTTP/1.1\r"));
    // Headers with no end: the line still counts, at the cap or when the time is up.
    for (wait, headers) in [(Duration::from_secs(30), "h".repeat(REQUEST_MAX)), (Duration::from_millis(200), "Host: x\r\n".to_string())] {
        let server = serve(wait);
        let mut browser = TcpStream::connect(addr).unwrap();
        browser.write_all(format!("GET / HTTP/1.1\n{headers}").as_bytes()).unwrap();
        assert_eq!(server.join().unwrap().as_deref(), Some("GET / HTTP/1.1"));
    }
    // And a line that is too long, or never whole, is still no request.
    for sent in ["G".repeat(REQUEST_LINE_MAX + 2) + "\n\n", "GET /?code=c".to_string()] {
        let server = serve(Duration::from_millis(200));
        let mut browser = TcpStream::connect(addr).unwrap();
        let _ = browser.write_all(sent.as_bytes());
        assert_eq!(server.join().unwrap(), None);
    }
}

/// The sign-in service failing (HTTP 5xx or 429, or RFC 6749's `server_error` and
/// `temporarily_unavailable`) refused nothing: the connection is dropped and tried again later.
/// Its refusals (`invalid_grant`, `invalid_client`) ask to sign in again. An answer without a
/// usable token is no sign-in either.
#[test]
fn a_failing_sign_in_service_refuses_nothing() {
    let answers = [
        ("503 Service Unavailable", r#"{"error":"temporarily_unavailable"}"#, false),
        ("500 Internal Server Error", r#"{"error":"server_error","error_description":"oops"}"#, false),
        ("502 Bad Gateway", "<html>down</html>", false),
        ("429 Too Many Requests", r#"{"error":"throttled"}"#, false),
        ("400 Bad Request", r#"{"error":"temporarily_unavailable"}"#, false),
        ("200 OK", r#"{"error":"server_error"}"#, false),
        ("200 OK", r#"{"access_token":"bad token"}"#, false),
        ("200 OK", r#"{"access_token":"ok","refresh_token":"bad\r\ntoken"}"#, false),
        ("400 Bad Request", r#"{"error":"invalid_grant","error_description":"revoked"}"#, true),
        ("401 Unauthorized", r#"{"error":"invalid_client"}"#, true),
    ];
    let (base, seen) = serve(answers.iter().map(|(status, body, _)| json(status, body)).collect());
    let p = local(&format!("{base}/token"));
    for (status, body, refused) in answers {
        let e = refresh(&p, "r1").unwrap_err();
        assert_eq!(matches!(e, NetError::Auth(_)), refused, "{status} {body}: {e}");
        assert!(refused || e.is_fatal(), "dropped and tried again: {status} {body}: {e}");
    }
    assert_eq!(seen.join().unwrap().len(), answers.len());
}

/// Google's client id and secret both come from the build, trimmed; either missing offers no
/// Google sign-in. The secret never shows in a `Debug`.
#[test]
fn google_needs_a_client_id_and_secret() {
    assert!(google_with(None, Some("s3cret")).is_none() && google_with(Some("id"), None).is_none());
    assert!(google_with(Some(" "), Some("s3cret")).is_none() && google_with(Some("id"), Some(" \n")).is_none());
    let p = google_with(Some(" 123-abc.apps.googleusercontent.example "), Some(" GOCSPX-s3cret\n")).unwrap();
    assert_eq!((p.client_id.as_str(), p.client_secret.as_deref()), ("123-abc.apps.googleusercontent.example", Some("GOCSPX-s3cret")));
    assert_eq!((p.auth_url.as_str(), p.token_url.as_str()), ("https://accounts.google.com/o/oauth2/v2/auth", "https://oauth2.googleapis.com/token"));
    assert_eq!((p.scope.as_str(), p.redirect_host, p.extra_auth, p.refresh_scope), (GOOGLE_SCOPE, "127.0.0.1", "&access_type=offline&prompt=consent", false));
    let debug = format!("{p:?}");
    assert!(!debug.contains("s3cret") && debug.contains("<redacted>") && debug.contains("123-abc"), "{debug}");
    let m = microsoft_with(Some("id")).unwrap();
    assert!(m.client_secret.is_none() && m.redirect_host == "localhost" && m.refresh_scope);
    assert!(format!("{m:?}").contains("client_secret: None"));
}

/// Google's sign-in: the redirect to 127.0.0.1, a refresh token asked for at every sign-in, the
/// client secret in both token requests (and out of their errors), no scope in them; the scope
/// granted is read from the answer.
#[test]
fn google_sign_in_then_refresh() {
    let first = r#"{"access_token":"ya29.good","refresh_token":"1//r1","expires_in":3599,"scope":"https://www.googleapis.com/auth/gmail.modify openid","token_type":"Bearer"}"#;
    let (base, seen) = serve(vec![
        json("200 OK", first),
        json("200 OK", r#"{"access_token":"ya29.fresh","expires_in":3599,"token_type":"Bearer"}"#),
        json("401 Unauthorized", r#"{"error":"invalid_client","error_description":"client GOCSPX-s3cret is not valid"}"#),
    ]);
    // Google's own sign-in page is only named, never reached: the "browser" goes straight back.
    let p = Provider { token_url: format!("{base}/token"), ..google_with(Some("gid"), Some("GOCSPX-s3cret")).unwrap() };
    let mut opened = String::new();
    let mut go = browser("code=4%2F0Acode&state={state}");
    let mut open = |url: &str| {
        opened = url.to_string();
        go(url)
    };
    let t = authorize(&p, "", &mut open, Duration::from_secs(20)).unwrap();
    assert_eq!((t.access.as_str(), t.refresh.as_deref()), ("ya29.good", Some("1//r1")));
    assert!(t.granted("https://www.googleapis.com/auth/gmail.modify") && !t.granted("https://www.googleapis.com/auth/gmail.settings.basic"));
    let redirect = param(&opened, "redirect_uri");
    assert!(redirect.starts_with("http://127.0.0.1:") && !redirect.ends_with('/'), "{redirect}");
    assert!(opened.starts_with("https://accounts.google.com/o/oauth2/v2/auth?client_id=gid&"), "{opened}");
    assert_eq!((param(&opened, "access_type"), param(&opened, "prompt")), ("offline".to_string(), "consent".to_string()));
    assert!(!opened.contains("response_mode") && !opened.contains("s3cret") && !opened.contains("login_hint"), "{opened}");
    assert_eq!(param(&opened, "scope"), format!("{GOOGLE_SCOPE} openid email profile"));

    // Google sends no new refresh token: the old one goes on.
    let t = refresh(&p, "1//r1").unwrap();
    assert_eq!((t.access.as_str(), t.refresh.as_deref(), t.scope.as_deref()), ("ya29.fresh", None, None));
    let e = refresh(&p, "1//r1").unwrap_err();
    assert!(matches!(&e, NetError::Auth(m) if m.contains("client *** is not valid") && !m.contains("s3cret")), "{e}");

    let seen = seen.join().unwrap();
    assert_eq!(seen.len(), 3);
    for field in ["client_id=gid&client_secret=GOCSPX-s3cret&", "grant_type=authorization_code", "code=4%2F0Acode", "code_verifier=", "redirect_uri=http%3A%2F%2F127.0.0.1%3A"] {
        assert!(seen[0].contains(field), "{field}: {}", seen[0]);
    }
    assert!(seen[1].ends_with("client_id=gid&client_secret=GOCSPX-s3cret&grant_type=refresh_token&refresh_token=1%2F%2Fr1"), "{}", seen[1]);
    // Google takes the scope from the grant: no token request names one.
    assert!(seen.iter().all(|r| !r.contains("scope=")), "{seen:?}");
}

/// Which permissions were granted: all of them when the answer doesn't say, else exactly those
/// it names.
#[test]
fn granted_scopes() {
    let t = |scope: Option<&str>| Tokens { access: "a".into(), refresh: None, expires_at: 0, user: None, scope: scope.map(str::to_string) };
    assert!(t(None).granted("https://www.googleapis.com/auth/gmail.modify"));
    let some = t(Some("openid  https://www.googleapis.com/auth/gmail.modify\thttps://www.googleapis.com/auth/userinfo.email"));
    assert!(some.granted("https://www.googleapis.com/auth/gmail.modify") && some.granted("openid"));
    assert!(!some.granted("https://www.googleapis.com/auth/gmail.settings.basic") && !some.granted("gmail.modify") && !some.granted(""));
    assert!(!t(Some("")).granted("openid"));
}
