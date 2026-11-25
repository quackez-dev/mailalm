//! Pictures from the internet in a received message (PLAN.md section 10.2): blocked unless the
//! user loads them for one message or approves its sender. Even then, only `https://` addresses
//! are fetched, at most [`MAX_PICTURES`] per message and [`MAX_PICTURE_BYTES`] each, never a
//! tracking picture (one the message declares 2 pixels wide or high or less, or hides, or whose
//! address is a known tracker's), with no cookie and no `Referer` ([`mailalm_net::fetch_picture`]).
//! What comes back is a PNG, JPEG or GIF by its content, at most 16384 pixels a side and 40
//! megapixels, decoded with those limits by the editor's widget; a picture of 2 pixels or less
//! is dropped as a tracker.

use std::sync::mpsc::{self, Receiver};

use mailalm_compose::{InlineImage, RemoteImage};

/// The most pictures fetched for one message.
pub const MAX_PICTURES: usize = 30;
/// The largest picture fetched.
pub const MAX_PICTURE_BYTES: usize = 5 << 20;

/// Senders of tracking pictures (the host or a name under it).
const TRACKER_HOSTS: [&str; 24] = [
    "mailtrack.io",
    "list-manage.com",
    "mandrillapp.com",
    "sendgrid.net",
    "mailgun.org",
    "mailgun.net",
    "hubspotemail.net",
    "hs-analytics.net",
    "google-analytics.com",
    "doubleclick.net",
    "mixpanel.com",
    "exacttarget.com",
    "returnpath.net",
    "litmus.com",
    "emltrk.com",
    "bananatag.com",
    "yesware.com",
    "mailstat.us",
    "getnotify.com",
    "streak.com",
    "awstrack.me",
    "sparkpostmail.com",
    "pixel.wp.com",
    "cmail19.com",
];

/// What the address of a tracking picture often holds.
const TRACKER_PATHS: [&str; 16] = [
    "/open?", "/open/", "open.gif", "open.php", "open.aspx", "/o.gif", "/wf/open", "pixel", "beacon", "/track/open", "/trk", "spacer.gif",
    "blank.gif", "1x1", "/e2t/o", "openrate",
];

/// The host of an http(s) address, lower case.
fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = host.rsplit_once(':').filter(|(_, p)| p.chars().all(|c| c.is_ascii_digit())).map_or(host, |(h, _)| h);
    host.trim_end_matches('.').to_ascii_lowercase()
}

/// Whether a web picture is a tracking one, never fetched: declared 2 pixels or less on a side
/// (or hidden), or at a known tracker's address.
pub fn is_tracker(r: &RemoteImage) -> bool {
    if r.width.is_some_and(|w| w <= 2) || r.height.is_some_and(|h| h <= 2) {
        return true;
    }
    let host = host_of(&r.url);
    if TRACKER_HOSTS.iter().any(|t| host == *t || host.ends_with(&format!(".{t}"))) {
        return true;
    }
    let lower = r.url.to_ascii_lowercase();
    let path = lower.split_once("://").map_or(lower.as_str(), |(_, rest)| rest.find('/').and_then(|i| rest.get(i..)).unwrap_or(""));
    TRACKER_PATHS.iter().any(|p| path.contains(p))
}

/// What loading a message's web pictures would do.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// The `https://` addresses fetched, in order, at most [`MAX_PICTURES`].
    pub fetch: Vec<String>,
    /// Tracking pictures, never fetched.
    pub trackers: usize,
    /// Plain `http://` pictures, not fetched.
    pub insecure: usize,
    /// Beyond [`MAX_PICTURES`].
    pub over: usize,
}

pub fn plan(remote: &[RemoteImage]) -> Plan {
    let mut p = Plan::default();
    for r in remote {
        if is_tracker(r) {
            p.trackers += 1;
        } else if !r.url.get(..8).is_some_and(|s| s.eq_ignore_ascii_case("https://")) {
            p.insecure += 1;
        } else if p.fetch.len() >= MAX_PICTURES {
            p.over += 1;
        } else {
            p.fetch.push(r.url.clone());
        }
    }
    p
}

/// A picture fetched as the editor shows it, checked by its content: `content_id` names it.
pub fn checked_picture(bytes: Vec<u8>, content_id: &str) -> Result<InlineImage, String> {
    if bytes.len() > MAX_PICTURE_BYTES {
        return Err("larger than 5 MB".to_owned());
    }
    let image = InlineImage::with_content_id(bytes, content_id).map_err(|e| e.to_string())?;
    if image.width <= 2 || image.height <= 2 {
        return Err("a tracking picture (2 pixels or less)".to_owned());
    }
    Ok(image)
}

/// One picture fetched (or why not), by its address.
pub type Fetched = (String, Result<InlineImage, String>);

/// Fetches `urls` one after the other on a thread of its own; each answer comes on the receiver
/// as it is ready (and the window is woken). `serial` keeps the content ids of two loads apart.
pub fn start(ctx: &egui::Context, urls: Vec<String>, serial: u64) -> Receiver<Fetched> {
    let (tx, rx) = mpsc::channel();
    let ctx = ctx.clone();
    let spawned = std::thread::Builder::new().name("mailalm-pictures".to_owned()).spawn(move || {
        for (n, url) in urls.into_iter().take(MAX_PICTURES).enumerate() {
            let result = mailalm_net::fetch_picture(&url, MAX_PICTURE_BYTES)
                .map_err(|e| e.to_string())
                .and_then(|bytes| checked_picture(bytes, &format!("web{serial}-{n}.mailalm")));
            if tx.send((url, result)).is_err() {
                return;
            }
            ctx.request_repaint();
        }
    });
    if let Err(e) = spawned {
        log::warn!("the pictures could not be fetched: {e}");
    }
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(url: &str, w: Option<u32>, h: Option<u32>) -> RemoteImage {
        RemoteImage { url: url.to_owned(), width: w, height: h }
    }

    #[test]
    fn trackers_are_never_fetched() {
        assert!(is_tracker(&r("https://cdn.shop.example/p.png", Some(1), Some(1))));
        assert!(is_tracker(&r("https://cdn.shop.example/p.png", Some(600), Some(2))));
        assert!(is_tracker(&r("https://cdn.shop.example/p.png", Some(0), Some(0))), "hidden");
        assert!(is_tracker(&r("https://u123.ct.sendgrid.net/wf/open?upn=abc", None, None)));
        assert!(is_tracker(&r("https://news.list-manage.com/track/open.php?u=1", None, None)));
        assert!(is_tracker(&r("https://shop.example/email/open?id=9", None, None)));
        assert!(is_tracker(&r("https://shop.example/img/pixel.gif", None, None)));
        assert!(!is_tracker(&r("https://cdn.shop.example/banner.jpg", Some(600), Some(200))));
        assert!(!is_tracker(&r("https://cdn.shop.example/logo.png", None, None)));
        assert!(!is_tracker(&r("https://sendgrid.net.example/logo.png", None, None)), "not under the tracker's name");
    }

    #[test]
    fn what_loading_would_fetch() {
        let mut remote = vec![r("https://t.example/o.gif", Some(1), Some(1)), r("http://cdn.example/a.png", None, None)];
        for n in 0..35 {
            remote.push(r(&format!("HTTPS://cdn.example/{n}.png"), None, None));
        }
        let p = plan(&remote);
        assert_eq!((p.trackers, p.insecure, p.over, p.fetch.len()), (1, 1, 5, MAX_PICTURES));
        assert_eq!(p.fetch[0], "HTTPS://cdn.example/0.png");
    }

    #[test]
    fn fetched_pictures_are_checked_by_content() {
        let png = |w: u32, h: u32| {
            let mut v = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
            v.extend(w.to_be_bytes());
            v.extend(h.to_be_bytes());
            v.extend([8, 6, 0, 0, 0]);
            v
        };
        assert!(checked_picture(png(120, 40), "web1-0.mailalm").is_ok());
        assert!(checked_picture(png(1, 1), "web1-1.mailalm").err().is_some_and(|e| e.contains("tracking")));
        assert!(checked_picture(b"<html>not a picture</html>".to_vec(), "web1-2.mailalm").is_err());
        assert!(checked_picture(png(20_000, 20_000), "web1-3.mailalm").is_err(), "too many pixels");
        assert!(checked_picture(vec![0; MAX_PICTURE_BYTES + 1], "web1-4.mailalm").is_err());
    }
}
