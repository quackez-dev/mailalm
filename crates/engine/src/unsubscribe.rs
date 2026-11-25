//! Leaving a mailing list (S4), on the user's confirmation only. The message's
//! `List-Unsubscribe` is read again here, from the cached message: the interface names the
//! message, never an address to contact.
//!
//! - One click (RFC 8058): the POST goes from here ([`mailalm_net::one_click_unsubscribe`]).
//! - By mail: the message the list's `mailto:` asks for, which the worker sends like any other.

use mailalm_mime::{Outgoing, Unsubscribe};
use mailalm_model::Person;

use crate::mail_worker::MailWorker;
use crate::{Event, UnsubscribeHow};

/// Does the one-click POST, or gives the message to send. The outcome is said to the user.
pub(crate) fn handle<W: MailWorker>(w: &W, folder: &str, uid: u32, how: UnsubscribeHow) -> Option<Outgoing> {
    match prepare(w, folder, uid, how) {
        Ok(Plan::Post(url)) => {
            let host = Unsubscribe::host_of(&url).to_owned();
            match mailalm_net::one_click_unsubscribe(&url) {
                Ok(_) => w.tell(Event::Notice {
                    account: w.who(),
                    text: format!("{host} took the unsubscription. Its mail may take a few days to stop."),
                }),
                Err(e) => w.tell(Event::Error { account: w.who(), text: format!("The unsubscription at {host} failed: {e}") }),
            }
            None
        }
        Ok(Plan::Mail(message)) => Some(*message),
        Err(e) => {
            w.tell(Event::Error { account: w.who(), text: e });
            None
        }
    }
}

enum Plan {
    Post(String),
    Mail(Box<Outgoing>),
}

fn prepare<W: MailWorker>(w: &W, folder: &str, uid: u32, how: UnsubscribeHow) -> Result<Plan, String> {
    let raw = w.cache().message(folder, uid).ok_or("This message is no longer in the cache: open it again, then unsubscribe.")?;
    let parsed = mailalm_mime::parse(&raw).ok_or("This message could not be read.")?;
    let u = parsed.unsubscribe.ok_or("This message doesn't say how to leave its list.")?;
    match how {
        UnsubscribeHow::OneClick => u.one_click.map(Plan::Post).ok_or_else(|| "This list doesn't offer a one-click unsubscription.".to_owned()),
        UnsubscribeHow::Mail => {
            let m = u.mailto.ok_or("This list doesn't take its unsubscriptions by mail.")?;
            let (_, address) = w.me();
            Ok(Plan::Mail(Box::new(Outgoing {
                from: Person { name: String::new(), address },
                to: vec![Person { name: String::new(), address: m.to }],
                subject: m.subject,
                text: m.body,
                ..Outgoing::default()
            })))
        }
    }
}
