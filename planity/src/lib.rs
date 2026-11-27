// SPDX-License-Identifier: GPL-3.0-or-later

//! A Planity Pro salon agenda, read-only, for Sioul. **Unofficial**: not
//! Planity's, not endorsed by it.
//!
//! Two sources each hold half of a booking, and neither is enough alone
//! (`merge-design.md`):
//!
//! - **the official feed**: Planity's own iCalendar export of a calendar (its
//!   "sync with another calendar" link, a keyed web address the person pastes
//!   into Sioul's sign-in form): the times, the services, the prices, the
//!   cancellations, the recurring series; no client's name nor phone;
//! - **the Firestore read**: Planity Pro is a Firebase web app (project
//!   `planity-production`); the extension signs in with the salon's email and
//!   password, does Firebase's own dance to a Pro-claimed token, keeps only
//!   the resulting refresh token, and reads the `vevents` appointment
//!   documents over the Firestore REST API: the client's name and phone, the
//!   note, the all-day blocks, the bookings older than the feed's window.
//!
//! Each booking becomes one busy event ([`merge`]); the salon's working time
//! lands in a calendar the person chooses. When one source fails, the other's
//! bookings still come, and nothing is removed (no `period`): degraded, never
//! destructive.
//!
//! **The history**: each refresh reads a window, sixty days back to a hundred
//! and twenty ahead; from the first refresh on a device (and the first after
//! each sign-in), and again a week after the last, the refreshes also read
//! the whole past, two years back (`HISTORY_DAYS`), one piece of whole days
//! each (`PIECE`, within `DOCUMENTS` with the window: Sioul's interpreter
//! gives a call 4 billion steps). The period the door may remove from begins
//! now, so an appointment of the past, once in the calendar, stays there
//! whatever later refreshes bring; Sioul keeps every event older than a
//! period. How far the past is read is kept on the device beside the sign-in
//! (`Kept::p`, `Kept::h`), noted last, once the reply is ready.
//!
//! The clients seen in the window's documents come back too, as one set of
//! contacts ([`clients`]), which Sioul writes into an address book the person
//! chooses, only ever adding to it; when Firestore did not answer, the reply
//! holds no contacts (the feed holds no client).
//!
//! The data model and the request/response shapes are validated from a
//! consented live capture, written down in the publisher's design notes
//! (`api.md`, `extension-spec.md`, `merge-design.md`). No real account,
//! business, client, token or key appears in this code or its tests; the
//! Firebase API key and the Planity hostnames are the only real strings, both
//! public.

mod clients;
mod feed;
mod merge;
mod planityapi;
#[cfg(test)]
#[path = "../tests/recorded.rs"]
mod recorded;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use planityapi::Link;
use serde::{Deserialize, Serialize};
use sioul_extension::ical::{self, Calendar, Event};
use sioul_extension::{Agenda, Call, Extension, Host, Period, Problem, ProblemKind, RefreshReply, Refused, SignInReply};
use std::collections::{BTreeMap, BTreeSet};

/// The extension.
pub struct Planity;

sioul_extension::export!(Planity);

/// Planity's Firebase web API key: public, embedded in the web app (`api.md`),
/// as a community client's vendor secret is. It only names the Firebase
/// project; it grants nothing without a sign-in.
pub const API_KEY: &str = "AIzaSyDrSE1PzMwLKyhEvh2x8eV7s1NYwKGRC5Q";

const PRODUCT: &str = concat!("planity ", env!("CARGO_PKG_VERSION"));
/// The window read each refresh: sixty days back, a hundred and twenty ahead
/// (within the interface's four-hundred-day cap).
const BEFORE_DAYS: i64 = 60;
const AHEAD_DAYS: i64 = 120;
/// The whole past: two years back, read from the first refresh on a device
/// (and the first after each sign-in), then again once a week, so that the
/// history comes back into a calendar that lost it (another calendar chosen
/// for the agenda, an event deleted by hand) without every refresh paying
/// for it.
const HISTORY_DAYS: i64 = 730;
const HISTORY_AGAIN: i64 = 7 * 86_400;
/// The past comes in pieces, one a refresh, each ending at the end of a
/// day: in Sioul's interpreter a document costs about 0.9 million of the 4
/// billion steps a call may take (measured on 27 November 2026: read 0.29 M,
/// merged 0.4 M, written 0.32 M). A refresh reads the window first, then a
/// piece of what is left of `DOCUMENTS` documents, `PIECE` at most, and
/// none under `PIECE_LEAST` (a window that fills a refresh by itself): about
/// 2.2 billion steps with the window, whatever the salon.
const PIECE: usize = 1_500;
const DOCUMENTS: usize = 2_400;
const PIECE_LEAST: usize = 300;
/// A whole read made within the hour after a sign-in is made again at the
/// next refresh: the first refresh comes before the person has chosen a
/// calendar for the agenda, and Sioul writes nothing until one is chosen.
const SETTLING: i64 = 3_600;
/// What a reply may weigh in Sioul (docs/extensions.md, "Limits of a call",
/// and the reply's bounds), with a margin: the reply 8 MiB, an agenda's text
/// 8 MiB, 250,000 lines and 20,000 events. Past one, the oldest
/// appointments of the past are left out, never the window's.
const REPLY_MOST: usize = 7 << 20;
const LINES_MOST: usize = 200_000;
const EVENTS_MOST: usize = 16_000;
/// The salon's zone: its `s` strings are Europe/Paris wall-clock, no zone.
const ZONE: &str = "Europe/Paris";
/// The most the keyring keeps for an extension (docs/extensions.md).
const KEPT_BYTES: usize = 2400;
/// An agenda's name when the feed gives none.
const DEFAULT_NAME: &str = "Planity";

impl Extension for Planity {
    fn sign_in(host: &mut dyn Host, call: &Call) -> SignInReply {
        let signed = signed_in(host, call);
        match &signed {
            Ok(()) => host.log("sign-in: signed in to Planity; the sign-in is kept on this device"),
            Err(fault) => host.log(&format!("sign-in: {}", fault.detail())),
        }
        match signed {
            Ok(()) => SignInReply::SignedIn,
            Err(fault) => SignInReply::Problem(fault.problem()),
        }
    }

    fn refresh(host: &mut dyn Host, call: &Call) -> RefreshReply {
        let kept = match kept(host) {
            Ok(kept) => kept,
            Err(fault) => {
                host.log(&format!("refresh: {}", fault.detail()));
                return RefreshReply { problem: Some(fault.problem()), ..RefreshReply::default() };
            }
        };
        read(host, kept, call.french())
    }
}

/// The fields of the form, checked (which one is empty or malformed, never
/// its value), the link checked, then the sign-in's three steps, the business
/// compared, and what is kept, kept.
fn signed_in(host: &mut dyn Host, call: &Call) -> Result<(), Fault> {
    let credentials = call.credentials.as_ref().ok_or_else(|| Fault::SignIn("the form: no credentials reached the extension".into()))?;
    // Sioul takes out the spaces a phone's keyboard adds around an email or a link; so does
    // this, for an older Sioul. A password is taken exactly as typed: a space may be part of it.
    let trimmed = |value: &Option<String>| value.as_deref().map(str::trim).unwrap_or("").to_string();
    let (email, address) = (trimmed(&credentials.email), trimmed(&credentials.web_address));
    let password = credentials.password.clone().unwrap_or_default();
    // Defensive: never let a credential reach the log, even though Sioul already hides them.
    host.hide(&[&email, &password, &address]);
    let empty: Vec<&str> = [("email", email.is_empty()), ("password", password.is_empty()), ("iCal private link", address.is_empty())].iter().filter(|(_, empty)| *empty).map(|(name, _)| *name).collect();
    match empty.as_slice() {
        [] => {}
        [one] => return Err(Fault::SignIn(format!("the form: the {one} field is empty"))),
        several => return Err(Fault::SignIn(format!("the form: the {} fields are empty", several.join(" and ")))),
    }
    if !email.contains('@') || email.contains(char::is_whitespace) {
        return Err(Fault::SignIn("the form: the email field does not hold an address (no @, or a space inside)".into()));
    }
    // The link is checked before anything is asked of the network.
    let link = Link::read(&address).map_err(|why| Fault::SignIn(format!("the form: {why}")))?;
    host.hide(&[&link.key, &link.address, &link.business, &link.calendar]);
    host.log("sign-in: the form's three fields read, the iCal private link is Planity's export");
    let (refresh, business) = planityapi::sign_in(host, &email, &password)?;
    if business != link.business {
        return Err(Fault::SignIn("the form: the iCal private link is another business's calendar than the account's".into()));
    }
    // A sign-in reads the whole past again from its first refresh (no `h`), and notes when it was made (`s`).
    let now = host.now();
    keep(host, &Kept { r: Some(refresh), b: business, l: Some(link.address.clone()), h: None, s: Some(now), p: None })
}

/// The sign-in kept on this device, read: "no sign-in kept on this device"
/// (none in its keyring: a sign-in made on another device is not shared),
/// "the sign-in kept here does not read" (not what this extension keeps), or
/// "the keyring did not answer", each its own problem.
fn kept(host: &mut dyn Host) -> Result<Kept, Fault> {
    match host.secret() {
        Ok(Some(text)) => match serde_json::from_str::<Kept>(&text).map(Kept::trimmed) {
            Ok(kept) if kept.r.is_some() || kept.l.is_some() => {
                host.log(&format!("refresh: the sign-in kept here read ({}{})", if kept.r.is_some() { "a refresh token" } else { "no refresh token" }, if kept.l.is_some() { ", the calendar's link" } else { ", no calendar link" }));
                Ok(kept)
            }
            Ok(_) => Err(Fault::SignIn("the sign-in kept here does not read: it holds neither a refresh token nor the calendar's link; sign in again".into())),
            Err(_) => Err(Fault::SignIn(format!("the sign-in kept here does not read: its {} bytes are not what this extension keeps; sign in again", text.len()))),
        },
        Ok(None) => Err(Fault::SignIn("no sign-in kept on this device: sign in on this device (a sign-in made on another device stays there)".into())),
        Err(refused) => Err(Fault::Unreachable(format!("the keyring did not answer when the sign-in kept here was read (Sioul refused: {})", refused.0))),
    }
}

// ---------------------------------------------------------------- what is kept

/// What the keyring holds: the refresh token B, the business id and the
/// calendar's link, never the password; and when this device last read the
/// whole past, how far its reading of it has come, and when it signed in.
/// Short keys so it fits. An older version of the
/// extension reads it too: it leaves out the keys it does not know.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Kept {
    /// The refresh token B (none: the feed alone, and Sioul asks to sign in).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    r: Option<String>,
    /// The business id.
    #[serde(default)]
    b: String,
    /// The calendar's export link (none: Firestore alone, and Sioul asks to sign in).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    l: Option<String>,
    /// When this device last read the whole past, in seconds since 1970
    /// (none: not since its sign-in, or a sign-in kept before 0.2.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    h: Option<i64>,
    /// When this device signed in, in seconds since 1970 (none: a sign-in
    /// kept before 0.2.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    s: Option<i64>,
    /// While this device reads the whole past piece by piece: the local day
    /// the next piece begins at (`"YYYY-MM-DD 00:00"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    p: Option<String>,
}

impl Kept {
    /// An empty value read as none.
    fn trimmed(self) -> Self {
        let some = |value: Option<String>| value.filter(|v| !v.trim().is_empty());
        Self { r: some(self.r), b: self.b.trim().to_string(), l: some(self.l), h: self.h, s: self.s, p: some(self.p) }
    }

    /// Whether this refresh reads the whole past: never read on this device
    /// since its sign-in, read a week ago or more (or at a time to come: a
    /// clock set back), or read within the hour after the sign-in, before a
    /// calendar was likely chosen ([`SETTLING`]).
    fn whole_due(&self, now: i64) -> bool {
        match self.h {
            None => true,
            Some(at) => now - at >= HISTORY_AGAIN || at > now || self.s.is_some_and(|signed| at - signed < SETTLING),
        }
    }
}

/// Keeps the sign-in (never the password), when it fits the keyring.
fn keep(host: &mut dyn Host, kept: &Kept) -> Result<(), Fault> {
    let text = serde_json::to_string(kept).unwrap_or_default();
    if text.len() > KEPT_BYTES {
        return Err(Fault::SignIn(format!("the sign-in to keep weighs {} bytes, more than the keyring keeps ({KEPT_BYTES})", text.len())));
    }
    host.keep_secret(&text).map_err(|refused| match refused.0.as_str() {
        "keyring" => Fault::Unreachable("the keyring did not answer when the sign-in was kept (Sioul refused: keyring)".into()),
        why => Fault::Changed(format!("the sign-in could not be kept (Sioul refused: {why})")),
    })
}

// ---------------------------------------------------------------- the refresh

/// How far back this refresh read Firestore.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Depth {
    /// The window alone.
    Window,
    /// A piece of the past and the window; the next piece begins that day.
    Piece(String),
    /// The last piece of the past, which reached the window, and the window:
    /// the whole past is read.
    Whole,
}

/// Reads both sources, then one event per booking: one busy agenda per
/// calendar, and the clients of the window's documents as one set of
/// contacts when Firestore answered. A piece of the past comes beside the
/// window while the whole past is being read, or when it is due again
/// ([`Kept::whole_due`]). The period read goes
/// with the agendas only when both answered, and it begins now: Sioul then
/// removes an appointment to come that Planity no longer holds, never one
/// of the past, whatever a later refresh brings. A sign-in that ended, on
/// either side, is said with what still came.
fn read(host: &mut dyn Host, mut kept: Kept, french: bool) -> RefreshReply {
    if let Some(r) = kept.r.clone() {
        host.hide(&[&r]);
    }
    if let Some(l) = kept.l.clone() {
        host.hide(&[&l]);
    }
    let link = kept.l.as_deref().map(Link::read);
    if let Some(Ok(link)) = &link {
        host.hide(&[&link.key, &link.business, &link.calendar]);
        if kept.b.is_empty() {
            kept.b = link.business.clone();
        }
    }
    // The business's id names it in Firestore's path, which Sioul's line of each request shows.
    if !kept.b.is_empty() {
        let business = kept.b.clone();
        host.hide(&[&business]);
    }

    let zone = jiff::tz::db().get(ZONE).unwrap_or(TimeZone::UTC);
    let now = host.now();
    let (from_ts, until_ts) = (now - BEFORE_DAYS * 86_400, now + AHEAD_DAYS * 86_400);
    let history_from = now - HISTORY_DAYS * 86_400;
    // A piece of the past: where the last one ended, or two years back when the whole past is due.
    let past_from = match kept.p.clone() {
        Some(resume) => Some(resume),
        None if kept.whole_due(now) => local_string(history_from, &zone).ok(),
        None => None,
    };

    // The Firestore half.
    let firestore = firestore(host, &mut kept, &zone, past_from.clone(), from_ts, until_ts);
    let depth = match &firestore {
        Ok((appointments, depth)) => {
            host.log(&format!("refresh: {} appointment(s) from Firestore", appointments.len()));
            depth.clone()
        }
        Err(fault) => {
            host.log(&format!("refresh: Firestore not read: {}", fault.detail()));
            Depth::Window
        }
    };
    // The dates of the past this refresh read, for the series' occurrences of the same dates.
    let at = |text: &str| parse_local(text, &zone);
    let past = match (&depth, past_from.as_deref().and_then(at)) {
        (Depth::Piece(resume), Some(first)) => at(resume).map(|end| (first, end)),
        (Depth::Whole, Some(first)) => Some((first, from_ts)),
        _ => None,
    }
    .filter(|(first, end)| first < end);
    let firestore = firestore.map(|(appointments, _)| appointments);
    // The feed half.
    let feed = match &link {
        None => Err(Fault::SignIn("no iCal private link kept on this device: sign in again with it".into())),
        Some(Err(why)) => Err(Fault::SignIn(format!("the iCal private link kept here does not read ({why}); sign in again with it"))),
        Some(Ok(link)) => planityapi::read_feed(host, link).and_then(|text| feed::read(&text).ok_or_else(|| Fault::Changed(format!("{} answered HTTP 200 with what is not an iCalendar", planityapi::FEED_STEP)))),
    };
    match &feed {
        Ok(feed) => host.log(&format!("refresh: {} event(s) from the feed, {} that do not read", feed.events.len(), feed.skipped)),
        Err(fault) => host.log(&format!("refresh: the feed not read: {}", fault.detail())),
    }

    let (appointments, feed) = match (firestore, feed) {
        (Err(firestore), Err(feed)) => {
            // Both failed: the problem, as before; a sign-in to renew first.
            let problem = if firestore.problem().kind != ProblemKind::SignIn && feed.problem().kind == ProblemKind::SignIn { feed.problem() } else { firestore.problem() };
            return RefreshReply { problem: Some(problem), ..RefreshReply::default() };
        }
        (firestore, feed) => (firestore, feed),
    };
    // The period, which lets the door remove what vanished, only when both answered; from now
    // on, so that an appointment of the past, once in the calendar, stays there.
    let period = (appointments.is_ok() && feed.is_ok()).then(|| Period::between(now, until_ts));
    // A side that failed only says so when the person can mend it (a sign-in, a refusal).
    let said = |fault: &Fault| Some(fault.problem()).filter(|p| matches!(p.kind, ProblemKind::SignIn | ProblemKind::Refused));
    let problem = match (&appointments, &feed) {
        (Err(fault), _) | (_, Err(fault)) => said(fault),
        _ => None,
    };
    // The clients of the window's documents, whatever became of their appointments, the same
    // whether or not the whole past came (a day's margin: the window's first local minute); none
    // when Firestore did not answer: the feed holds no client.
    let contacts = match &appointments {
        Ok(appointments) => {
            let window: Vec<merge::Appointment> = appointments.iter().filter(|a| a.start >= from_ts - 86_400).cloned().collect();
            let (set, lines) = clients::set(&window);
            for line in &lines {
                host.log(&format!("refresh: {line}"));
            }
            vec![set]
        }
        Err(_) => Vec::new(),
    };
    let appointments = appointments.unwrap_or_default();
    let feed = feed.ok();

    let calendar = match &link {
        Some(Ok(link)) => Some(link.calendar.clone()),
        _ => None,
    };
    let merged = merge::merge(feed.as_ref(), &appointments, (from_ts, until_ts), past, calendar.as_deref(), french);
    drop(appointments);
    for line in &merged.log {
        host.log(line);
    }

    // The feed's calendars take its name; the others, Planity's.
    let named = feed.as_ref().and_then(|f| f.name.clone());
    let feed_calendars: Vec<&str> = calendar.iter().map(String::as_str).chain(feed.iter().flat_map(|f| f.events.iter().filter_map(|e| e.calendar.as_deref()))).collect();
    let mut calendars = merged.calendars;
    // The link's calendar is always handed back, empty or not, so that its period reaches the door.
    if let Some(own) = &calendar {
        calendars.entry(own.clone()).or_default();
    }
    if calendars.is_empty() {
        calendars.insert("planity".into(), Vec::new());
    }
    let names: Vec<(String, String)> = calendars.keys().map(|id| (id.clone(), named.clone().filter(|_| feed_calendars.contains(&id.as_str())).unwrap_or_else(|| DEFAULT_NAME.into()))).collect();
    // Within what Sioul takes: the contacts, the reply's own keys and words, then each agenda.
    let others = serde_json::to_string(&contacts).map(|t| t.len()).unwrap_or(0) + 64 * 1024;
    let (texts, left_out) = written(calendars, &names, from_ts, others);
    if left_out > 0 {
        host.log(&format!("refresh: {left_out} of the oldest appointment(s) of the past left out, to stay within what Sioul takes in one reply"));
    }
    let agendas = texts.into_iter().map(|(id, name, text)| Agenda { id: agenda_id(&id), name, calendar: text, period: period.clone(), unread: Vec::new(), busy: true }).collect();
    // How far this device has read the whole past, noted beside its sign-in last of all: a
    // call stopped before its reply (Sioul's steps, its memory) notes nothing, and the next
    // refresh reads the same piece again.
    let noted = match &depth {
        Depth::Window => None,
        Depth::Piece(resume) => {
            kept.p = Some(resume.clone());
            Some(format!("a piece of the past read; the next begins on {}", resume.get(..10).unwrap_or("")))
        }
        Depth::Whole => {
            kept.h = Some(now);
            kept.p = None;
            Some("the whole past read".to_string())
        }
    };
    if let Some(noted) = noted {
        match keep(host, &kept) {
            Ok(()) => host.log(&format!("refresh: {noted}, noted on this device")),
            Err(fault) => host.log(&format!("refresh: {noted}, not noted on this device: {}", fault.detail())),
        }
    }
    RefreshReply { agendas, contacts, problem }
}

/// Each calendar's events as iCalendar text, by calendar id, with its name
/// (`names`, in the calendars' order), within what Sioul takes in one reply
/// beside `others` bytes: the reply and each agenda's text [`REPLY_MOST`]
/// once written as JSON, [`LINES_MOST`] lines and [`EVENTS_MOST`] events an
/// agenda. Past one, the oldest events that begin before `past_until` (the
/// window's start) are left out, the oldest first, never one of the window;
/// how many, beside the texts.
fn written(mut calendars: BTreeMap<String, Vec<Event>>, names: &[(String, String)], past_until: i64, others: usize) -> (Vec<(String, String, String)>, usize) {
    let text = |name: &str, events: Vec<Event>| Calendar { product: PRODUCT.into(), name: name.to_string(), events }.text();
    let name_of = |id: &str| names.iter().find(|(i, _)| i == id).map(|(_, n)| n.clone()).unwrap_or_else(|| DEFAULT_NAME.into());
    // Each written once, its events taken back afterwards (never copied: a whole past holds thousands).
    let mut texts: Vec<(String, String, String)> = Vec::with_capacity(calendars.len());
    for (id, events) in calendars.iter_mut() {
        let calendar = Calendar { product: PRODUCT.into(), name: name_of(id), events: std::mem::take(events) };
        texts.push((id.clone(), calendar.name.clone(), calendar.text()));
        *events = calendar.events;
    }
    let fits = |weights: &[(usize, usize, usize)]| {
        let total: usize = weights.iter().map(|(bytes, _, _)| bytes).sum::<usize>() + others;
        total <= REPLY_MOST && weights.iter().all(|(bytes, lines, events)| *bytes <= REPLY_MOST && *lines <= LINES_MOST && *events <= EVENTS_MOST)
    };
    let mut weights: Vec<(usize, usize, usize)> = texts.iter().zip(calendars.values()).map(|((_, _, t), events)| (json_bytes(t), t.matches('\n').count(), events.len())).collect();
    if fits(&weights) {
        return (texts, 0);
    }
    // Each event of the past, weighed alone (its text in a calendar of its own, the calendar's own lines aside).
    let mut past: Vec<(i64, String, usize, usize, usize)> = Vec::new();
    for (at, (id, events)) in calendars.iter().enumerate() {
        let name = name_of(id);
        let bare = text(&name, Vec::new());
        let (bare_bytes, bare_lines) = (json_bytes(&bare), bare.matches('\n').count());
        for event in events.iter().filter(|e| e.start < past_until) {
            let alone = text(&name, vec![event.clone()]);
            past.push((event.start, event.uid.clone(), at, json_bytes(&alone).saturating_sub(bare_bytes), alone.matches('\n').count().saturating_sub(bare_lines)));
        }
    }
    past.sort();
    let mut left: BTreeMap<usize, BTreeSet<String>> = BTreeMap::new();
    let mut count = 0;
    for (_, uid, at, bytes, lines) in past {
        if fits(&weights) {
            break;
        }
        let weight = &mut weights[at];
        *weight = (weight.0.saturating_sub(bytes), weight.1.saturating_sub(lines), weight.2.saturating_sub(1));
        left.entry(at).or_default().insert(uid);
        count += 1;
    }
    for (at, (id, events)) in calendars.iter_mut().enumerate() {
        if let Some(gone) = left.get(&at) {
            events.retain(|e| !(e.start < past_until && gone.contains(&e.uid)));
            texts[at] = (id.clone(), name_of(id), text(&name_of(id), std::mem::take(events)));
        }
    }
    (texts, count)
}

/// A text's length once written as a JSON string: each quote, backslash and
/// common control character two bytes, any other control character six.
fn json_bytes(text: &str) -> usize {
    text.len() + text.chars().map(|c| match c {
        '"' | '\\' | '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 1,
        c if c.is_control() && (c as u32) < 0x20 => 5,
        _ => 0,
    }).sum::<usize>()
}

/// Mints a fresh id token (keeping a rotated refresh token) and reads the
/// appointment documents: the window's, from `from_ts`, then, when one is
/// due, a piece of the past from `past_from` (a local wall-clock string) up
/// to the window, of what the window leaves of [`DOCUMENTS`]; each document
/// once. How far back it read, beside them: a piece of the past that fails
/// is only logged (the next refresh asks it again), the window's failing is
/// the Firestore half's problem.
fn firestore(host: &mut dyn Host, kept: &mut Kept, zone: &TimeZone, past_from: Option<String>, from_ts: i64, until_ts: i64) -> Result<(Vec<merge::Appointment>, Depth), Fault> {
    let refresh = kept.r.clone().ok_or_else(|| Fault::SignIn("no Firebase sign-in kept on this device (only the calendar's link): sign in again".into()))?;
    if kept.b.is_empty() {
        return Err(Fault::SignIn("no business kept on this device: sign in again".into()));
    }
    let (id_token, rotated) = planityapi::mint(host, &refresh)?;
    host.hide(&[&id_token]);
    if let Some(new_refresh) = rotated.filter(|new| *new != refresh) {
        host.hide(&[&new_refresh]);
        kept.r = Some(new_refresh);
        keep(host, kept)?;
        host.log("refresh: the new refresh token kept on this device");
    }
    let (from, until) = (local_string(from_ts, zone)?, local_string(until_ts, zone)?);
    let take = |document: planityapi::Document| merge::Appointment::read(&document, zone);
    let unread = |host: &mut dyn Host, read: &planityapi::Read<merge::Appointment>| {
        if read.items.len() < read.documents {
            host.log(&format!("refresh: {} appointment(s) whose start does not read", read.items.len().abs_diff(read.documents)));
        }
    };
    let read = planityapi::read_vevents(host, &kept.b, &id_token, &from, &until, planityapi::WINDOW_PAGES, None, take)?;
    if !read.complete {
        return Err(Fault::Changed(format!("{}: more than {} pages of appointments", planityapi::FIRESTORE_STEP, planityapi::WINDOW_PAGES)));
    }
    unread(host, &read);
    let mut window = read.items;
    let piece = DOCUMENTS.saturating_sub(window.len()).min(PIECE);
    let mut depth = Depth::Window;
    match past_from {
        Some(past_from) if past_from >= from => depth = Depth::Whole,
        Some(_) if piece < PIECE_LEAST => host.log(&format!("refresh: the window's {} appointment(s) fill this refresh: no piece of the past", window.len())),
        Some(past_from) => match planityapi::read_vevents(host, &kept.b, &id_token, &past_from, &from, planityapi::PIECE_PAGES, Some(piece), take) {
            Ok(read) => {
                unread(host, &read);
                host.log(&format!("refresh: {} appointment(s) of the past, from {}", read.items.len(), past_from.get(..10).unwrap_or("")));
                depth = match read.resume.clone() {
                    _ if read.complete => Depth::Whole,
                    // Never the same day again: a day that alone fills a piece's pages is left for the next.
                    Some(resume) if resume > past_from => Depth::Piece(resume),
                    _ => Depth::Piece(next_day(&past_from)),
                };
                // A document at the window's very first minute is in both reads: the window's kept.
                let ids: BTreeSet<&str> = window.iter().map(|a| a.id.as_str()).collect();
                let mut past: Vec<merge::Appointment> = read.items.into_iter().filter(|a| !ids.contains(a.id.as_str())).collect();
                past.append(&mut window);
                window = past;
            }
            Err(fault) => host.log(&format!("refresh: the past not read this time: {}", fault.detail())),
        },
        None => {}
    }
    Ok((window, depth))
}

/// The local day after `"YYYY-MM-DD …"`, as `"YYYY-MM-DD 00:00"`.
fn next_day(text: &str) -> String {
    let day = parse_date(text).map(|(y, m, d)| ical::days(y, m, d) + 1).unwrap_or(0);
    let (y, m, d) = ical::civil(day);
    format!("{y:04}-{m:02}-{d:02} 00:00")
}

/// Seconds since 1970 (UTC) as a Europe/Paris wall-clock string `"YYYY-MM-DD HH:MM"`.
fn local_string(at: i64, zone: &TimeZone) -> Result<String, Fault> {
    let instant = Timestamp::from_second(at).map_err(|_| Fault::Changed(format!("the time {at} given by Sioul is out of range")))?;
    Ok(instant.to_zoned(zone.clone()).strftime("%Y-%m-%d %H:%M").to_string())
}

// ---------------------------------------------------------------- times and ids

/// The date part of `"YYYY-MM-DD HH:MM"` as `(year, month, day)`.
pub(crate) fn parse_date(text: &str) -> Option<(i64, u32, u32)> {
    let mut fields = text.get(0..10)?.split('-');
    let year = fields.next()?.parse().ok()?;
    let month = fields.next()?.parse().ok()?;
    let day = fields.next()?.parse().ok()?;
    Some((year, month, day))
}

/// `"YYYY-MM-DD HH:MM"` read as a Europe/Paris wall clock, to its UTC instant
/// in seconds (summer or winter time, DST gaps and folds disambiguated by jiff).
pub(crate) fn parse_local(text: &str, zone: &TimeZone) -> Option<i64> {
    let mut normalized = text.replace(' ', "T");
    if normalized.len() == 16 {
        normalized.push_str(":00");
    }
    let civil: jiff::civil::DateTime = normalized.parse().ok()?;
    Some(civil.to_zoned(zone.clone()).ok()?.timestamp().as_second())
}

/// A stable lower-case agenda id from a calendar id: letters and digits kept,
/// anything else `-`, runs of `-` collapsed and trimmed, 48 characters at
/// most; a hash ends a longer one, and stands in for one with nothing left
/// (never an id beginning with `-`, which Sioul refuses).
fn agenda_id(calendar: &str) -> String {
    let mut id: String = calendar.to_ascii_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    while id.contains("--") {
        id = id.replace("--", "-");
    }
    let id = id.trim_matches('-').to_string();
    if !id.is_empty() && id.len() <= 48 {
        return id;
    }
    let hash = calendar.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193));
    let head: String = id.chars().take(39).collect();
    let head = head.trim_matches('-');
    format!("{}-{hash:08x}", if head.is_empty() { "planity" } else { head })
}

// ---------------------------------------------------------------- what went wrong

/// What went wrong, before it is a problem said to Sioul: its kind, and its
/// detail, which names the step (the form, the password sign-in, the token's
/// renewal, Firestore, the feed, the keyring), the HTTP status and the
/// service's own code when there is one, and the cause in words; never a
/// value (a password, a token, the link or its key).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Fault {
    SignIn(String),
    Unreachable(String),
    Refused(String),
    Changed(String),
}

impl Fault {
    pub(crate) fn detail(&self) -> String {
        match self {
            Self::SignIn(d) | Self::Unreachable(d) | Self::Refused(d) | Self::Changed(d) => d.clone(),
        }
    }

    pub(crate) fn problem(&self) -> Problem {
        let kind = match self {
            Self::SignIn(_) => ProblemKind::SignIn,
            Self::Unreachable(_) => ProblemKind::Unreachable,
            Self::Refused(_) => ProblemKind::Refused,
            Self::Changed(_) => ProblemKind::Changed,
        };
        Problem::new(kind, self.detail())
    }

    /// Sioul's refusal of `step`'s request: the network, the time or the
    /// count, that it could not be reached now; a host or an address it
    /// would not reach, that Planity moved (an update is needed).
    pub(crate) fn refused_at(step: &str, refused: &Refused) -> Self {
        let detail = format!("{step} could not be asked: Sioul refused it ({})", refused.0);
        match refused.0.as_str() {
            "network" | "time" | "count" => Self::Unreachable(detail),
            _ => Self::Changed(detail),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sioul_extension::ical;

    #[test]
    fn ids_and_times_read() {
        // A Firebase push id calendar: lower-cased, its leading `-` trimmed.
        assert_eq!(agenda_id("-Binvented0000000000"), "binvented0000000000");
        assert_eq!(agenda_id("Chair A / Salon"), "chair-a-salon");
        let long = agenda_id(&"X".repeat(80));
        assert!(long.len() <= 48 && long.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'), "{long}");
        let bare = agenda_id("----");
        assert!(bare.starts_with("planity-") && !bare.ends_with('-'), "{bare}");
        assert_eq!(parse_date("2026-11-25 00:00"), Some((2026, 11, 25)));
        // 14:00 Paris in November (winter, +01:00) is 13:00 UTC.
        let paris = jiff::tz::db().get("Europe/Paris").unwrap();
        assert_eq!(parse_local("2026-11-20 14:00", &paris), Some(ical::days(2026, 11, 20) * 86_400 + 13 * 3600));
        // 14:00 Paris in July (summer, +02:00) is 12:00 UTC.
        assert_eq!(parse_local("2026-07-20 14:00", &paris), Some(ical::days(2026, 7, 20) * 86_400 + 12 * 3600));
        assert_eq!(local_string(ical::days(2026, 11, 20) * 86_400 + 13 * 3600, &paris).unwrap(), "2026-11-20 14:00");
    }

    fn event(uid: String, start: i64) -> Event {
        Event { uid, start, end: start + 1800, summary: "Coupe".into(), location: None, description: None, cancelled: false, transparent: false, all_day: false, categories: Vec::new(), person: None }
    }

    /// More events than an agenda holds in Sioul: the oldest of the past
    /// left out until it holds them, every one of the window kept; an agenda
    /// within every bound written as it is.
    #[test]
    fn an_agenda_keeps_within_its_events_the_oldest_of_the_past_left_out() {
        let window = 2_000_000_000;
        let mut events: Vec<Event> = (0..EVENTS_MOST as i64 + 100).map(|i| event(format!("past-{i:06}"), window - 1_000_000 + i * 60)).collect();
        events.extend((0..5).map(|i| event(format!("window-{i}"), window + i * 3600)));
        let calendars = BTreeMap::from([("-Cinvented0000000000".to_string(), events.clone())]);
        let names = vec![("-Cinvented0000000000".to_string(), "Institut Exemple".to_string())];
        let (texts, left_out) = written(calendars, &names, window, 0);
        assert_eq!(left_out, 105, "five over, and the window's five never count as the past's");
        let text = &texts[0].2;
        assert_eq!(text.matches("BEGIN:VEVENT").count(), EVENTS_MOST);
        assert!(!text.contains("UID:past-000104\r\n") && text.contains("UID:past-000105\r\n"), "the oldest first");
        assert!((0..5).all(|i| text.contains(&format!("UID:window-{i}\r\n"))));
        // Within every bound: as it is.
        let few = BTreeMap::from([("-Cinvented0000000000".to_string(), events[EVENTS_MOST..].to_vec())]);
        let (texts, left_out) = written(few, &names, window, 0);
        assert_eq!((left_out, texts[0].2.matches("BEGIN:VEVENT").count()), (0, 105));
        assert_eq!(json_bytes("a\"b\\c\r\n\u{1}"), 8 + 1 + 1 + 1 + 1 + 5, "eight bytes, four escaped by two, one by six");
    }

    /// The keyring's entry: one kept by an older version reads (the whole
    /// past then due), the mark and the sign-in's time kept through a trim,
    /// and when the whole past is due.
    #[test]
    fn when_the_whole_past_is_due() {
        let old: Kept = serde_json::from_str(r#"{"r":"RT","b":"-B","l":"https://x"}"#).unwrap();
        assert_eq!((old.h, old.s), (None, None));
        assert!(old.whole_due(1_000_000_000), "never read on this device");
        let now = 1_800_000_000;
        let read = |h: i64, s: Option<i64>| Kept { h: Some(h), s, ..old.clone() }.trimmed();
        assert!(!read(now - 3600, None).whole_due(now), "read an hour ago");
        assert!(!read(now - HISTORY_AGAIN + 1, None).whole_due(now));
        assert!(read(now - HISTORY_AGAIN, None).whole_due(now), "a week ago");
        assert!(read(now + 60, None).whole_due(now), "at a time to come: a clock set back");
        assert!(read(now - 3600, Some(now - 3600 - 600)).whole_due(now), "read ten minutes after the sign-in: again");
        assert!(!read(now - 3600, Some(now - 3600 - SETTLING)).whole_due(now), "read an hour after the sign-in: done");
        let text = serde_json::to_string(&read(now, Some(now))).unwrap();
        assert!(text.ends_with(&format!(r#","h":{now},"s":{now}}}"#)), "{text}");
    }
}
