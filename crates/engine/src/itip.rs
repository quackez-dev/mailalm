//! Meeting messages sent by mail (iMIP, RFC 6047): an IMAP account's invitations, cancellations
//! and answers, and an Outlook or Gmail account's answer when its provider can't give it (an
//! event its calendar doesn't have, another time proposed to a Google calendar's organizer).
//!
//! The calendar is written by `mailalm_model::pim::ical` and goes as a `text/calendar` part beside
//! a short text that says the same for a reader without a calendar.

use mailalm_mime::{CalendarPart, Outgoing};
use mailalm_model::Person;
use mailalm_model::pim::ical::{self, Method};
use mailalm_model::pim::{Attendee, Event, Response, ZoneRules, local_time};
use mailalm_model::query::civil_from_days;

/// A time as the text of a meeting message has it: in the event's zone (named), else in UTC.
fn when(e: &Event, t: i64, zones: &dyn ZoneRules) -> String {
    if e.all_day {
        let (y, m, d) = civil_from_days(t.div_euclid(86_400));
        return format!("{y:04}-{m:02}-{d:02}");
    }
    let zone = e.time_zone.trim();
    let (local, name) = if zone.is_empty() || zones.offset(zone, t).is_none() { (t, "UTC") } else { (local_time(zones, zone, t), zone) };
    let (y, m, d) = civil_from_days(local.div_euclid(86_400));
    let s = local.rem_euclid(86_400);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02} ({name})", s / 3600, s % 3600 / 60)
}

/// What a meeting message's text says of the event.
fn summary(e: &Event, zones: &dyn ZoneRules) -> String {
    let mut out = format!("{}\n\nWhen: {} to {}", if e.title.is_empty() { "(no title)" } else { &e.title }, when(e, e.start, zones), when(e, e.end, zones));
    if !e.location.is_empty() {
        out.push_str(&format!("\nWhere: {}", e.location));
    }
    if let Some(url) = e.online.as_ref().map(|o| o.url.as_str()).filter(|u| !u.is_empty()) {
        out.push_str(&format!("\nJoin: {url}"));
    }
    if let Some(o) = &e.organizer {
        out.push_str(&format!("\nOrganizer: {}", o.email));
    }
    if !e.description.trim().is_empty() {
        out.push_str(&format!("\n\n{}", e.description.trim()));
    }
    out
}

fn person(a: &Attendee) -> Person {
    Person { name: a.name.clone(), address: a.email.clone() }
}

/// The guests a message about `e` goes to: every guest but `me` (and the organizer).
fn guests(e: &Event, me: &str) -> Vec<Person> {
    let organizer = e.organizer.as_ref().map(|o| o.email.clone()).unwrap_or_default();
    let mut out: Vec<Person> = Vec::new();
    for a in &e.attendees {
        if a.is(me) || a.is(&organizer) || out.iter().any(|p| p.address.eq_ignore_ascii_case(&a.email)) {
            continue;
        }
        out.push(person(a));
    }
    out
}

/// An invitation (or an update of one) from its organizer `me` to its guests; `None` when it has
/// no guest or can't be written.
pub(crate) fn request(e: &Event, me: &Person, zones: &dyn ZoneRules, now: i64) -> Option<Outgoing> {
    let to = guests(e, &me.address);
    if to.is_empty() {
        return None;
    }
    let text = ical::request(std::slice::from_ref(e), zones, now)?;
    let verb = if e.sequence > 0 { "Updated invitation" } else { "Invitation" };
    Some(Outgoing {
        from: me.clone(),
        to,
        subject: format!("{verb}: {}", e.title),
        text: format!("{} invites you.\n\n{}", me.address, summary(e, zones)),
        calendar: Some(CalendarPart { method: Method::Request.code().to_owned(), text }),
        ..Outgoing::default()
    })
}

/// A cancellation from its organizer `me` to its guests; `None` as for [`request`].
pub(crate) fn cancel(e: &Event, me: &Person, zones: &dyn ZoneRules, now: i64) -> Option<Outgoing> {
    let to = guests(e, &me.address);
    if to.is_empty() {
        return None;
    }
    let text = ical::cancel(e, zones, now)?;
    Some(Outgoing {
        from: me.clone(),
        to,
        subject: format!("Cancelled: {}", e.title),
        text: format!("{} cancelled this meeting.\n\n{}", me.address, summary(e, zones)),
        calendar: Some(CalendarPart { method: Method::Cancel.code().to_owned(), text }),
        ..Outgoing::default()
    })
}

/// A guest's answer from `me` to the organizer (with `proposal`, another time proposed: a
/// `COUNTER`); `None` when the event has no organizer or the answer can't be written.
pub(crate) fn reply(e: &Event, me: &Person, answer: Response, proposal: Option<(i64, i64)>, comment: &str, zones: &dyn ZoneRules, now: i64) -> Option<Outgoing> {
    let organizer = e.organizer.as_ref()?;
    let mine = e.attendee(&me.address).cloned().unwrap_or_else(|| Attendee::new(&me.address, &me.name));
    let text = ical::reply(e, &mine, answer, proposal, comment, now)?;
    let method = if proposal.is_some() { Method::Counter } else { Method::Reply };
    let what = match answer {
        Response::Accepted => "Accepted",
        Response::Tentative => "Tentative",
        Response::Declined => "Declined",
        Response::NeedsAction => return None,
    };
    let mut body = match proposal {
        Some((start, end)) => {
            let proposed = Event { start, end, ..e.clone() };
            format!("{} proposes another time: {} to {}.", me.address, when(&proposed, start, zones), when(&proposed, end, zones))
        }
        None => format!("{} answered: {}.", me.address, what.to_ascii_lowercase()),
    };
    if !comment.trim().is_empty() {
        body.push_str(&format!("\n\n{}", comment.trim()));
    }
    body.push_str(&format!("\n\n{}", summary(e, zones)));
    let subject = if proposal.is_some() { format!("New time proposed: {}", e.title) } else { format!("{what}: {}", e.title) };
    Some(Outgoing { from: me.clone(), to: vec![person(organizer)], subject, text: body, calendar: Some(CalendarPart { method: method.code().to_owned(), text }), ..Outgoing::default() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mailalm_model::pim::UtcOnly;

    fn meeting() -> Event {
        Event {
            uid: "m1@videotron.ca".into(),
            title: "Mix review".into(),
            start: 1_795_615_200,
            end: 1_795_618_800,
            organizer: Some(Attendee::new("alex@videotron.ca", "Alex")),
            attendees: vec![Attendee::new("alex@videotron.ca", "Alex"), Attendee::new("zoe@studio.example", "Zoe"), Attendee::new("sam@example.com", "")],
            is_organizer: true,
            ..Event::default()
        }
    }

    fn me() -> Person {
        Person { name: "Alex".into(), address: "alex@videotron.ca".into() }
    }

    #[test]
    fn an_invitation_goes_to_the_guests_with_its_calendar() {
        let o = request(&meeting(), &me(), &UtcOnly, 1_795_000_000).expect("written");
        let to: Vec<&str> = o.to.iter().map(|p| p.address.as_str()).collect();
        assert_eq!(to, ["zoe@studio.example", "sam@example.com"], "not to the organizer");
        let cal = o.calendar.as_ref().expect("a calendar part");
        assert_eq!(cal.method, "REQUEST");
        let unfolded = cal.text.replace("\r\n ", "");
        assert!(unfolded.contains("METHOD:REQUEST") && unfolded.contains("UID:m1@videotron.ca"), "{unfolded}");
        assert!(o.text.contains("Mix review") && o.text.contains("(UTC)"));
        let (raw, _) = mailalm_mime::build(&o).expect("built");
        let back = mailalm_mime::parse(&raw).expect("parsed");
        assert_eq!(back.attachments.first().map(|a| a.declared_type.as_str()), Some("text/calendar"));

        // Alone, there is no one to invite.
        let alone = Event { attendees: Vec::new(), ..meeting() };
        assert!(request(&alone, &me(), &UtcOnly, 0).is_none());
        let c = cancel(&meeting(), &me(), &UtcOnly, 0).expect("written");
        assert_eq!(c.calendar.map(|c| c.method), Some("CANCEL".to_owned()));
    }

    #[test]
    fn an_answer_goes_to_the_organizer() {
        let zoe = Person { name: "Zoe".into(), address: "zoe@studio.example".into() };
        let e = Event { is_organizer: false, my_response: Some(Response::NeedsAction), ..meeting() };
        let r = reply(&e, &zoe, Response::Accepted, None, "See you", &UtcOnly, 0).expect("written");
        assert_eq!(r.to.len(), 1);
        assert_eq!(r.to[0].address, "alex@videotron.ca");
        assert_eq!(r.subject, "Accepted: Mix review");
        let cal = r.calendar.expect("a calendar part");
        assert_eq!(cal.method, "REPLY");
        let unfolded = cal.text.replace("\r\n ", "");
        assert!(unfolded.contains("PARTSTAT=ACCEPTED") && unfolded.contains("mailto:zoe@studio.example"), "{unfolded}");

        let counter = reply(&e, &zoe, Response::Tentative, Some((e.start + 3600, e.end + 3600)), "", &UtcOnly, 0).expect("written");
        assert_eq!(counter.calendar.map(|c| c.method), Some("COUNTER".to_owned()));
        assert!(counter.subject.starts_with("New time proposed"));
        assert!(reply(&e, &zoe, Response::NeedsAction, None, "", &UtcOnly, 0).is_none());
        let no_organizer = Event { organizer: None, ..e };
        assert!(reply(&no_organizer, &zoe, Response::Declined, None, "", &UtcOnly, 0).is_none());
    }
}
