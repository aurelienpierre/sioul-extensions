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
//! The data model and the request/response shapes are validated from a
//! consented live capture, written down in the publisher's design notes
//! (`api.md`, `extension-spec.md`, `merge-design.md`). No real account,
//! business, client, token or key appears in this code or its tests; the
//! Firebase API key and the Planity hostnames are the only real strings, both
//! public.

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
use sioul_extension::ical::Calendar;
use sioul_extension::{Agenda, Call, Extension, Host, Period, Problem, ProblemKind, RefreshReply, Refused, SignInReply};

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
/// The salon's zone: its `s` strings are Europe/Paris wall-clock, no zone.
const ZONE: &str = "Europe/Paris";
/// The most the keyring keeps for an extension (docs/extensions.md).
const KEPT_BYTES: usize = 2400;
/// An agenda's name when the feed gives none.
const DEFAULT_NAME: &str = "Planity";

impl Extension for Planity {
    fn sign_in(host: &mut dyn Host, call: &Call) -> SignInReply {
        let Some(credentials) = call.credentials.as_ref() else {
            return SignInReply::Problem(Problem::new(ProblemKind::SignIn, "no credentials were given"));
        };
        let present = |value: &Option<String>| value.as_deref().map(str::trim).filter(|value| !value.is_empty()).map(str::to_string);
        let (Some(email), Some(password), Some(address)) = (present(&credentials.email), present(&credentials.password), present(&credentials.web_address)) else {
            return SignInReply::Problem(Problem::new(ProblemKind::SignIn, "an email, a password and the calendar's web address are needed"));
        };
        // Defensive: never let a credential reach the log, even though Sioul already hides them.
        host.hide(&[&email, &password, &address]);
        // The link is checked before anything is asked of the network.
        let link = match Link::read(&address) {
            Ok(link) => link,
            Err(why) => {
                host.log(&format!("sign-in: {why}"));
                return SignInReply::Problem(Problem::new(ProblemKind::SignIn, why));
            }
        };
        host.hide(&[&link.key, &link.address]);
        let signed = planityapi::sign_in(host, &email, &password).and_then(|(refresh, business)| {
            if business != link.business {
                return Err(Fault::SignIn("the web address is another business's calendar".into()));
            }
            keep(host, &Kept { r: Some(refresh), b: business, l: Some(link.address.clone()) })
        });
        match signed {
            Ok(()) => {
                host.log("signed in to Planity");
                SignInReply::SignedIn
            }
            Err(fault) => {
                host.log(&format!("sign-in: {}", fault.detail()));
                SignInReply::Problem(fault.problem())
            }
        }
    }

    fn refresh(host: &mut dyn Host, call: &Call) -> RefreshReply {
        let kept = match host.secret() {
            Ok(Some(text)) => match serde_json::from_str::<Kept>(&text).map(Kept::trimmed) {
                Ok(kept) if kept.r.is_some() || kept.l.is_some() => kept,
                _ => return RefreshReply::problem(ProblemKind::SignIn, "the sign-in kept does not read"),
            },
            Ok(None) => return RefreshReply::problem(ProblemKind::SignIn, "no sign-in kept"),
            Err(refused) => return RefreshReply { problem: Some(Fault::from(refused).problem()), ..RefreshReply::default() },
        };
        read(host, kept, call.french())
    }
}

// ---------------------------------------------------------------- what is kept

/// What the keyring holds: the refresh token B, the business id and the
/// calendar's link, never the password. Short keys so it fits.
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
}

impl Kept {
    /// An empty value read as none.
    fn trimmed(self) -> Self {
        let some = |value: Option<String>| value.filter(|v| !v.trim().is_empty());
        Self { r: some(self.r), b: self.b.trim().to_string(), l: some(self.l) }
    }
}

/// Keeps the sign-in (never the password), when it fits the keyring.
fn keep(host: &mut dyn Host, kept: &Kept) -> Result<(), Fault> {
    let text = serde_json::to_string(kept).unwrap_or_default();
    if text.len() > KEPT_BYTES {
        return Err(Fault::SignIn("the sign-in is longer than the keyring keeps".into()));
    }
    host.keep_secret(&text).map_err(Fault::from)
}

// ---------------------------------------------------------------- the refresh

/// Reads both sources, then one event per booking: one busy agenda per
/// calendar. The period read goes with the agendas only when both answered;
/// a sign-in that ended, on either side, is said with what still came.
fn read(host: &mut dyn Host, mut kept: Kept, french: bool) -> RefreshReply {
    if let Some(r) = kept.r.clone() {
        host.hide(&[&r]);
    }
    if let Some(l) = kept.l.clone() {
        host.hide(&[&l]);
    }
    let link = kept.l.as_deref().map(Link::read);
    if let Some(Ok(link)) = &link {
        host.hide(&[&link.key]);
        if kept.b.is_empty() {
            kept.b = link.business.clone();
        }
    }

    let zone = jiff::tz::db().get(ZONE).unwrap_or(TimeZone::UTC);
    let now = host.now();
    let (from_ts, until_ts) = (now - BEFORE_DAYS * 86_400, now + AHEAD_DAYS * 86_400);

    // The Firestore half.
    let firestore = firestore(host, &mut kept, &zone, from_ts, until_ts);
    match &firestore {
        Ok(appointments) => host.log(&format!("refresh: {} appointment(s) from Firestore", appointments.len())),
        Err(fault) => host.log(&format!("refresh: Firestore not read: {}", fault.detail())),
    }
    // The feed half.
    let feed = match &link {
        None => Err(Fault::SignIn("no calendar web address kept".into())),
        Some(Err(why)) => Err(Fault::SignIn((*why).into())),
        Some(Ok(link)) => planityapi::read_feed(host, link).and_then(|text| feed::read(&text).ok_or_else(|| Fault::Changed("the calendar feed is not an iCalendar".into()))),
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
    // The period, which lets the door remove what vanished, only when both answered.
    let period = (appointments.is_ok() && feed.is_ok()).then(|| Period::between(from_ts, until_ts));
    // A side that failed only says so when the person can mend it (a sign-in, a refusal).
    let said = |fault: &Fault| Some(fault.problem()).filter(|p| matches!(p.kind, ProblemKind::SignIn | ProblemKind::Refused));
    let problem = match (&appointments, &feed) {
        (Err(fault), _) | (_, Err(fault)) => said(fault),
        _ => None,
    };
    let appointments = appointments.unwrap_or_default();
    let feed = feed.ok();

    let calendar = match &link {
        Some(Ok(link)) => Some(link.calendar.clone()),
        _ => None,
    };
    let merged = merge::merge(feed.as_ref(), &appointments, (from_ts, until_ts), calendar.as_deref(), french);
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
    let agendas = calendars
        .into_iter()
        .map(|(id, events)| {
            let name = named.clone().filter(|_| feed_calendars.contains(&id.as_str())).unwrap_or_else(|| DEFAULT_NAME.into());
            let text = Calendar { product: PRODUCT.into(), name: name.clone(), events }.text();
            Agenda { id: agenda_id(&id), name, calendar: text, period: period.clone(), unread: Vec::new(), busy: true }
        })
        .collect();
    RefreshReply { agendas, problem, ..RefreshReply::default() }
}

/// Mints a fresh id token (keeping a rotated refresh token) and reads the
/// window's appointment documents.
fn firestore(host: &mut dyn Host, kept: &mut Kept, zone: &TimeZone, from_ts: i64, until_ts: i64) -> Result<Vec<merge::Appointment>, Fault> {
    let refresh = kept.r.clone().ok_or_else(|| Fault::SignIn("no Firebase sign-in kept".into()))?;
    if kept.b.is_empty() {
        return Err(Fault::SignIn("no business kept".into()));
    }
    let (id_token, rotated) = planityapi::mint(host, &refresh)?;
    host.hide(&[&id_token]);
    if let Some(new_refresh) = rotated.filter(|new| *new != refresh) {
        host.hide(&[&new_refresh]);
        kept.r = Some(new_refresh);
        keep(host, kept)?;
    }
    let from = local_string(from_ts, zone)?;
    let until = local_string(until_ts, zone)?;
    let documents = planityapi::read_vevents(host, &kept.b, &id_token, &from, &until)?;
    let appointments: Vec<merge::Appointment> = documents.iter().filter_map(|d| merge::Appointment::read(d, zone)).collect();
    if appointments.len() < documents.len() {
        host.log(&format!("refresh: {} appointment(s) whose start does not read", documents.len() - appointments.len()));
    }
    Ok(appointments)
}

/// Seconds since 1970 (UTC) as a Europe/Paris wall-clock string `"YYYY-MM-DD HH:MM"`.
fn local_string(at: i64, zone: &TimeZone) -> Result<String, Fault> {
    let instant = Timestamp::from_second(at).map_err(|_| Fault::Changed("the time is out of range".into()))?;
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

/// What went wrong, before it is a problem said to Sioul.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Fault {
    SignIn(String),
    Unreachable(String),
    Refused(String),
    Changed(String),
    /// A status the extension did not expect.
    Status(u16, String),
}

impl Fault {
    pub(crate) fn detail(&self) -> String {
        match self {
            Self::SignIn(d) | Self::Unreachable(d) | Self::Refused(d) | Self::Changed(d) => d.clone(),
            Self::Status(status, d) => format!("{d}: {status}"),
        }
    }

    pub(crate) fn problem(&self) -> Problem {
        let kind = match self {
            Self::SignIn(_) => ProblemKind::SignIn,
            Self::Unreachable(_) => ProblemKind::Unreachable,
            Self::Refused(_) => ProblemKind::Refused,
            Self::Changed(_) => ProblemKind::Changed,
            Self::Status(401, _) => ProblemKind::SignIn,
            Self::Status(403, _) => ProblemKind::Refused,
            Self::Status(status, _) if *status >= 500 => ProblemKind::Unreachable,
            Self::Status(_, _) => ProblemKind::Changed,
        };
        Problem::new(kind, self.detail())
    }
}

/// Sioul's refusal of an ask: a host or an address it would not reach means
/// Planity moved (an update is needed); the network or a limit, that it could
/// not be reached now.
impl From<Refused> for Fault {
    fn from(refused: Refused) -> Self {
        let detail = format!("Sioul refused: {}", refused.0);
        match refused.0.as_str() {
            "network" | "time" | "keyring" | "count" => Self::Unreachable(detail),
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
}
