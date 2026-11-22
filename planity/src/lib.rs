// SPDX-License-Identifier: GPL-3.0-or-later

//! A Planity Pro salon agenda, read-only, for Sioul. **Unofficial**: not
//! Planity's, not endorsed by it.
//!
//! Planity Pro is a Firebase web app (project `planity-production`). This
//! extension signs in with the salon's email and password (the credentials
//! sign-in Sioul collects in its own form), does Firebase's own dance to a
//! Pro-claimed token, keeps only the resulting refresh token and the business
//! id, and reads the `vevents` appointment documents over the Firestore REST
//! API. Each appointment becomes one busy event; the salon's working time
//! lands in a calendar the person chooses.
//!
//! The data model and the request/response shapes are validated from a
//! consented live capture, written down in `~/Life/coding/sioul/planity/`
//! (`api.md`, `extension-spec.md`). No real account, business, client or
//! token appears in this code or its tests; the Firebase API key and the
//! Planity hostnames are the only real strings, both public.

mod planityapi;
#[cfg(test)]
#[path = "../tests/recorded.rs"]
mod recorded;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sioul_extension::ical::{self, Calendar, Event};
use sioul_extension::{Agenda, Call, Extension, Host, Period, Problem, ProblemKind, RefreshReply, Refused, SignInReply};
use std::collections::BTreeMap;

/// The extension.
pub struct Planity;

sioul_extension::export!(Planity);

/// Planity's Firebase web API key: public, embedded in the web app (`api.md`),
/// as a community client's vendor secret is. It only names the Firebase
/// project; it grants nothing without a sign-in.
pub const API_KEY: &str = "AIzaSyDrSE1PzMwLKyhEvh2x8eV7s1NYwKGRC5Q";

const PRODUCT: &str = concat!("planity ", env!("CARGO_PKG_VERSION"));
/// The window read each refresh: sixty days back, a hundred and twenty ahead
/// (within Planity's four-hundred-day interface cap).
const BEFORE_DAYS: i64 = 60;
const AHEAD_DAYS: i64 = 120;
/// The salon's zone: its `s` strings are Europe/Paris wall-clock, no zone.
const ZONE: &str = "Europe/Paris";
/// A whole day of minutes (an all-day block is `d` ≈ 1439).
const DAY_MINUTES: i64 = 1440;
/// The most the keyring keeps for an extension (docs/extensions.md).
const KEPT_BYTES: usize = 2400;

impl Extension for Planity {
    fn sign_in(host: &mut dyn Host, call: &Call) -> SignInReply {
        let Some(credentials) = call.credentials.as_ref() else {
            return SignInReply::Problem(Problem::new(ProblemKind::SignIn, "no credentials were given"));
        };
        let (Some(email), Some(password)) = (
            credentials.email.as_deref().filter(|value| !value.is_empty()),
            credentials.password.as_deref().filter(|value| !value.is_empty()),
        ) else {
            return SignInReply::Problem(Problem::new(ProblemKind::SignIn, "an email and a password are needed"));
        };
        // Defensive: never let a credential reach the log, even though Sioul already hides them.
        host.hide(&[email, password]);
        match planityapi::sign_in(host, email, password).and_then(|(refresh, business)| keep(host, &refresh, &business)) {
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
            Ok(Some(text)) => match serde_json::from_str::<Kept>(&text) {
                Ok(kept) if !kept.r.is_empty() && !kept.b.is_empty() => kept,
                _ => return RefreshReply::problem(ProblemKind::SignIn, "the sign-in kept does not read"),
            },
            Ok(None) => return RefreshReply::problem(ProblemKind::SignIn, "no sign-in kept"),
            Err(refused) => return RefreshReply { agendas: Vec::new(), problem: Some(Fault::from(refused).problem()) },
        };
        match read(host, &kept, call.french()) {
            Ok(reply) => reply,
            Err(fault) => {
                host.log(&format!("refresh: {}", fault.detail()));
                RefreshReply { agendas: Vec::new(), problem: Some(fault.problem()) }
            }
        }
    }
}

// ---------------------------------------------------------------- what is kept

/// What the keyring holds: the refresh token B and the business id, never the
/// password. Short keys so it fits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Kept {
    /// The refresh token B.
    r: String,
    /// The business id.
    b: String,
}

/// Keeps the refresh token and the business id (never the password).
fn keep(host: &mut dyn Host, refresh: &str, business: &str) -> Result<(), Fault> {
    let text = serde_json::to_string(&Kept { r: refresh.to_string(), b: business.to_string() }).unwrap_or_default();
    if text.len() > KEPT_BYTES {
        return Err(Fault::Changed("the sign-in kept is longer than the keyring keeps".into()));
    }
    host.keep_secret(&text).map_err(Fault::from)
}

// ---------------------------------------------------------------- the refresh

/// Mints a fresh id token, reads the window's appointments, and builds one
/// busy agenda per calendar.
fn read(host: &mut dyn Host, kept: &Kept, french: bool) -> Result<RefreshReply, Fault> {
    host.hide(&[&kept.r]);
    let (id_token, rotated) = planityapi::mint(host, &kept.r)?;
    host.hide(&[&id_token]);
    if let Some(new_refresh) = rotated.filter(|new| new != &kept.r) {
        host.hide(&[&new_refresh]);
        keep(host, &new_refresh, &kept.b)?;
    }

    let zone = jiff::tz::db().get(ZONE).unwrap_or(TimeZone::UTC);
    let now = host.now();
    let (from_ts, until_ts) = (now - BEFORE_DAYS * 86_400, now + AHEAD_DAYS * 86_400);
    let from = local_string(from_ts, &zone)?;
    let until = local_string(until_ts, &zone)?;

    let documents = planityapi::read_vevents(host, &kept.b, &id_token, &from, &until)?;
    host.log(&format!("refresh: {} appointment(s)", documents.len()));

    // One agenda per distinct calendar `ca` (a multi-chair salon has several).
    let mut by_calendar: BTreeMap<String, Vec<Event>> = BTreeMap::new();
    for document in &documents {
        let calendar = document
            .fields
            .get("ca")
            .and_then(Value::as_str)
            .or_else(|| document.fields.get("sca").and_then(Value::as_str))
            .filter(|value| !value.is_empty())
            .unwrap_or("planity")
            .to_string();
        if let Some(event) = event(document, &zone, french) {
            by_calendar.entry(calendar).or_default().push(event);
        }
    }

    let period = Some(Period::between(from_ts, until_ts));
    let mut agendas: Vec<Agenda> = by_calendar
        .into_iter()
        .map(|(calendar, mut events)| {
            events.sort_by(|a, b| (a.start, &a.uid).cmp(&(b.start, &b.uid)));
            let text = Calendar { product: PRODUCT.into(), name: "Planity".into(), events }.text();
            Agenda { id: agenda_id(&calendar), name: "Planity".into(), calendar: text, period: period.clone(), unread: Vec::new(), busy: true }
        })
        .collect();

    // No appointment at all in the window: one empty agenda still carries the period, so the
    // door clears what vanished. Its id is the default key, which a one-calendar salon's
    // appointments never take while there are any (noted in the handoff).
    if agendas.is_empty() {
        let text = Calendar { product: PRODUCT.into(), name: "Planity".into(), events: Vec::new() }.text();
        agendas.push(Agenda { id: "planity".into(), name: "Planity".into(), calendar: text, period, unread: Vec::new(), busy: true });
    }

    Ok(RefreshReply { agendas, problem: None })
}

/// Seconds since 1970 (UTC) as a Europe/Paris wall-clock string `"YYYY-MM-DD HH:MM"`.
fn local_string(at: i64, zone: &TimeZone) -> Result<String, Fault> {
    let instant = Timestamp::from_second(at).map_err(|_| Fault::Changed("the time is out of range".into()))?;
    Ok(instant.to_zoned(zone.clone()).strftime("%Y-%m-%d %H:%M").to_string())
}

// ---------------------------------------------------------------- a vevent → an event

/// A `vevents` document as a busy event. A client appointment (`cu.id`
/// present) is titled by the client's name; a block or closure (no `cu.id`),
/// "Unavailable" / « Indisponible ». An all-day block (`ad`) is a floating
/// whole-day event. A cancelled one (`dat` present) is cancelled. The
/// description carries the duration, the price, the phone and the note. The
/// event is left busy (no `TRANSP`): the agenda declares `busy: true` and the
/// door writes the final transparency from the person's choice.
fn event(document: &planityapi::Document, zone: &TimeZone, french: bool) -> Option<Event> {
    let fields = &document.fields;
    let start_text = fields.get("s").and_then(Value::as_str)?;
    let duration = fields.get("d").and_then(Value::as_i64).unwrap_or(0).max(0);
    let all_day = fields.get("ad").and_then(Value::as_bool).unwrap_or(false);
    let cancelled = fields.get("dat").is_some_and(|value| !value.is_null());
    let client = fields.get("cu").filter(|cu| cu.is_object());
    let client_id = client.and_then(|cu| cu.get("id")).and_then(Value::as_str).filter(|id| !id.is_empty());

    let (start, end) = if all_day {
        let (year, month, day) = parse_date(start_text)?;
        let first = ical::days(year, month, day) * 86_400;
        let span_days = ((duration + DAY_MINUTES - 1) / DAY_MINUTES).max(1);
        (first, first + span_days * 86_400)
    } else {
        let start = parse_local(start_text, zone)?;
        (start, start + duration * 60)
    };

    let summary = match client_id {
        Some(_) => client
            .and_then(|cu| cu.get("name"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(if french { "Rendez-vous" } else { "Appointment" })
            .to_string(),
        None => if french { "Indisponible" } else { "Unavailable" }.to_string(),
    };

    let mut parts: Vec<String> = Vec::new();
    if duration > 0 && !all_day {
        parts.push(if french { format!("Durée\u{a0}: {duration} min") } else { format!("Duration: {duration} min") });
    }
    if let Some(cents) = fields.get("p").and_then(Value::as_i64) {
        let (euros, remainder) = (cents / 100, (cents % 100).abs());
        parts.push(format!("{euros},{remainder:02}\u{a0}€"));
    }
    if let Some(phone) = client.and_then(|cu| cu.get("phone")).and_then(Value::as_str).map(str::trim).filter(|phone| !phone.is_empty()) {
        parts.push(phone.to_string());
    }
    if let Some(note) = fields.get("t").and_then(Value::as_str).map(str::trim).filter(|note| !note.is_empty()) {
        parts.push(note.to_string());
    }
    let description = (!parts.is_empty()).then(|| parts.join("\n"));

    Some(Event {
        uid: document.id.clone(),
        start,
        end,
        summary,
        location: None,
        description,
        cancelled,
        // The agenda declares busy; the door decides the transparency.
        transparent: false,
        all_day,
    })
}

/// The date part of `"YYYY-MM-DD HH:MM"` as `(year, month, day)`.
fn parse_date(text: &str) -> Option<(i64, u32, u32)> {
    let mut fields = text.get(0..10)?.split('-');
    let year = fields.next()?.parse().ok()?;
    let month = fields.next()?.parse().ok()?;
    let day = fields.next()?.parse().ok()?;
    Some((year, month, day))
}

/// `"YYYY-MM-DD HH:MM"` read as a Europe/Paris wall clock, to its UTC instant
/// in seconds (summer or winter time, DST gaps and folds disambiguated by jiff).
fn parse_local(text: &str, zone: &TimeZone) -> Option<i64> {
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
