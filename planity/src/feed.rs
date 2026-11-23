// SPDX-License-Identifier: GPL-3.0-or-later

//! Planity's official iCalendar export of one calendar (its "sync with
//! another calendar" link, `getCalendarIcalEvents`), read leniently, and its
//! recurring series expanded into occurrences.
//!
//! The structure read is the one measured on the live feed (`api.md`, "The
//! official iCal export"): one `VEVENT` per booking, `DTSTART`/`DTEND` with
//! `TZID=Europe/Paris`, the service in `X-SERVICE-NAME` and
//! `X-PLANITY-SERVICE-ID`, the price in cents in `X-PRICE`, cancelled ones
//! kept as `STATUS:CANCELLED`, a booking of several services listing its
//! appointment ids in `X-PLANITY-APPOINTMENT-IDS`, and `X-PLANITY-DATA` a
//! JSON object `{sq, id, ca, uat, cuId?}`. No client name nor phone.
//!
//! Sioul keeps no `RRULE` (one extension event is one occurrence), so the
//! series are expanded here, in local wall-clock time: `FREQ=DAILY` or
//! `WEEKLY`, `INTERVAL`, `BYDAY`, `WKST`, `UNTIL` (inclusive, UTC, local or a
//! date) and `EXDATE` (matched on the local start), at most
//! [`MAX_OCCURRENCES`] a series. A rule with any other part keeps its first
//! occurrence only. A hand-written expander for these few shapes keeps the
//! extension's file small.

use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde_json::Value;
use sioul_extension::ical;

/// The most occurrences one series gives; more are cut (and logged).
pub const MAX_OCCURRENCES: usize = 400;
/// The zone of a time written without one, when the calendar names none.
const DEFAULT_ZONE: &str = "Europe/Paris";

/// A wall-clock moment: the civil date (days since 1970) and the seconds into that day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Local {
    pub day: i64,
    pub second: i64,
}

/// A wall clock in `zone` as its UTC instant (seconds since 1970); a time
/// that does not exist (a spring-forward gap) moves forward, as calendars do.
pub fn to_utc(local: Local, zone: &TimeZone) -> Option<i64> {
    let (year, month, day) = ical::civil(local.day);
    let clock = jiff::civil::DateTime::new(
        i16::try_from(year).ok()?,
        i8::try_from(month).ok()?,
        i8::try_from(day).ok()?,
        i8::try_from(local.second / 3600).ok()?,
        i8::try_from(local.second / 60 % 60).ok()?,
        i8::try_from(local.second % 60).ok()?,
        0,
    )
    .ok()?;
    Some(clock.to_zoned(zone.clone()).ok()?.timestamp().as_second())
}

/// A UTC instant as the wall clock in `zone`.
pub fn to_local(at: i64, zone: &TimeZone) -> Option<Local> {
    let clock = Timestamp::from_second(at).ok()?.to_zoned(zone.clone()).datetime();
    let day = ical::days(i64::from(clock.year()), u32::try_from(clock.month()).ok()?, u32::try_from(clock.day()).ok()?);
    Some(Local { day, second: i64::from(clock.hour()) * 3600 + i64::from(clock.minute()) * 60 + i64::from(clock.second()) })
}

/// Milliseconds since 1970 from a JSON value: a number, a string of digits,
/// or an RFC 3339 instant (Planity's `uat`, in the feed or in Firestore).
pub fn millis(value: &Value) -> Option<i64> {
    if let Some(number) = value.as_i64() {
        return Some(number);
    }
    if let Some(number) = value.as_f64() {
        return Some(number as i64);
    }
    let text = value.as_str()?.trim();
    if let Ok(number) = text.parse::<i64>() {
        return Some(number);
    }
    text.parse::<Timestamp>().ok().map(Timestamp::as_millisecond)
}

/// A JSON value as text: a string as it is, a number written out.
fn text_of(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.trim().to_string()).filter(|t| !t.is_empty()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

// ---------------------------------------------------------------- the calendar

/// The calendar read: its name (`X-WR-CALNAME`), its events, and how many
/// did not read (skipped, counted in the log).
#[derive(Debug, Clone)]
pub struct Feed {
    pub name: Option<String>,
    pub events: Vec<FeedEvent>,
    pub skipped: usize,
}

/// One `VEVENT` of the feed, its times resolved.
#[derive(Debug, Clone)]
pub struct FeedEvent {
    pub uid: String,
    /// The booking key: `X-PLANITY-DATA.sq`, else the `UID`.
    pub key: String,
    /// The zone its wall clock is in (`DTSTART`'s `TZID`, else the calendar's).
    pub zone: TimeZone,
    /// Its start's wall clock in `zone`.
    pub local: Local,
    /// Seconds since 1970, UTC; for a whole-day event, UTC midnight of its
    /// civil dates (as [`ical::Event`] wants them).
    pub start: i64,
    pub end: i64,
    pub all_day: bool,
    pub summary: Option<String>,
    pub cancelled: bool,
    pub rule: Option<String>,
    pub exdates: Vec<Exdate>,
    pub service_names: Vec<String>,
    pub service_ids: Vec<String>,
    /// `X-PRICE`, cents (several are added).
    pub price: Option<i64>,
    pub prepaid: bool,
    pub booked_online: bool,
    /// `X-PLANITY-APPOINTMENT-IDS`: the Firestore documents of the booking.
    pub appointment_ids: Vec<String>,
    /// `X-PLANITY-DATA.uat`, milliseconds.
    pub updated: Option<i64>,
    /// `X-PLANITY-DATA.cuId`: the client's id, never their name.
    pub client_id: Option<String>,
    /// `X-PLANITY-CALENDAR-ID`, else `X-PLANITY-DATA.ca`.
    pub calendar: Option<String>,
}

/// A date taken out of a series.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exdate {
    /// A wall-clock start in the series' zone.
    At(Local),
    /// A whole date.
    Day(i64),
}

/// A content line: its name (upper case), its parameters (names upper case,
/// values unquoted), its raw value.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Line {
    name: String,
    params: Vec<(String, String)>,
    value: String,
}

impl Line {
    fn param(&self, name: &str) -> Option<&str> {
        self.params.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }
}

/// The text's content lines, unfolded (a line that begins with a space or a
/// tab continues the one before), CRLF or LF.
fn unfolded(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        match (raw.strip_prefix([' ', '\t']), lines.last_mut()) {
            (Some(rest), Some(last)) => last.push_str(rest),
            _ if raw.trim().is_empty() => {}
            _ => lines.push(raw.to_string()),
        }
    }
    lines
}

/// `NAME;PARAM=value;PARAM="quoted":value`; none when there is no `:`.
fn line(text: &str) -> Option<Line> {
    let end = text.find([';', ':'])?;
    let name = text[..end].trim().to_ascii_uppercase();
    let mut rest = &text[end..];
    let mut params = Vec::new();
    while let Some(after) = rest.strip_prefix(';') {
        let equals = after.find(['=', ':', ';'])?;
        let param = after[..equals].trim().to_ascii_uppercase();
        let mut tail = &after[equals..];
        let mut value = String::new();
        if let Some(after_equals) = tail.strip_prefix('=') {
            if let Some(quoted) = after_equals.strip_prefix('"') {
                let close = quoted.find('"')?;
                value.push_str(&quoted[..close]);
                tail = &quoted[close + 1..];
            } else {
                let stop = after_equals.find([';', ':']).unwrap_or(after_equals.len());
                value.push_str(&after_equals[..stop]);
                tail = &after_equals[stop..];
            }
        }
        params.push((param, value));
        rest = tail;
    }
    let value = rest.strip_prefix(':')?.to_string();
    Some(Line { name, params, value })
}

/// A TEXT value unescaped (RFC 5545 §3.3.11): `\\`, `\;`, `\,`, `\n`/`\N`.
pub fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n' | 'N') => out.push('\n'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// The feed read; none when it is not an iCalendar at all.
pub fn read(text: &str) -> Option<Feed> {
    let lines: Vec<Line> = unfolded(text).iter().filter_map(|l| line(l)).collect();
    if !lines.iter().any(|l| l.name == "BEGIN" && l.value.trim().eq_ignore_ascii_case("VCALENDAR")) {
        return None;
    }
    // The calendar's own lines (outside any component but the calendar), and each event's
    // lines; a component inside an event (an alarm) is passed over.
    let mut name = None;
    let mut zone_name = None;
    let mut raw_events: Vec<Vec<Line>> = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    for l in lines {
        match l.name.as_str() {
            "BEGIN" => {
                let component = l.value.trim().to_ascii_uppercase();
                if component == "VEVENT" && stack.len() == 1 {
                    raw_events.push(Vec::new());
                }
                stack.push(component);
            }
            "END" => {
                stack.pop();
            }
            _ if stack.len() == 1 => match l.name.as_str() {
                "X-WR-CALNAME" => name = Some(unescape(&l.value).trim().to_string()).filter(|n| !n.is_empty()),
                "X-WR-TIMEZONE" => zone_name = Some(l.value.trim().to_string()),
                _ => {}
            },
            _ if stack.len() == 2 && stack[1] == "VEVENT" => {
                if let Some(event) = raw_events.last_mut() {
                    event.push(l);
                }
            }
            _ => {}
        }
    }
    let db = jiff::tz::db();
    let zone = zone_name.and_then(|n| db.get(&n).ok()).or_else(|| db.get(DEFAULT_ZONE).ok()).unwrap_or(TimeZone::UTC);
    let mut events = Vec::new();
    let mut skipped = 0;
    for raw in &raw_events {
        match event(raw, &zone) {
            Some(event) => events.push(event),
            None => skipped += 1,
        }
    }
    Some(Feed { name, events, skipped })
}

/// A time value as written: an instant (`…Z`), a wall clock (with the zone
/// its `TZID` names, if any), or a whole date.
enum Written {
    Utc(i64),
    Clock(Local, Option<TimeZone>),
    Date(i64),
}

/// `YYYYMMDD`, `YYYYMMDDTHHMMSS` or `YYYYMMDDTHHMMSSZ`, with the line's `TZID` and `VALUE`.
fn written(value: &str, tzid: Option<&str>, date_only: bool) -> Option<Written> {
    let value = value.trim();
    let digits = |range: std::ops::Range<usize>| -> Option<i64> {
        let part = value.get(range)?;
        part.bytes().all(|b| b.is_ascii_digit()).then(|| part.parse().ok())?
    };
    let (year, month, day) = (digits(0..4)?, digits(4..6)?, digits(6..8)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let days = ical::days(year, month as u32, day as u32);
    if date_only || value.len() == 8 {
        return Some(Written::Date(days));
    }
    if value.as_bytes().get(8) != Some(&b'T') {
        return None;
    }
    let (hour, minute, second) = (digits(9..11)?, digits(11..13)?, digits(13..15)?);
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let local = Local { day: days, second: hour * 3600 + minute * 60 + second.min(59) };
    match &value[15..] {
        "Z" | "z" => Some(Written::Utc(days * 86_400 + local.second)),
        "" => Some(Written::Clock(local, tzid.and_then(|name| jiff::tz::db().get(name.trim()).ok()))),
        _ => None,
    }
}

fn written_of(line: &Line, value: &str) -> Option<Written> {
    written(value, line.param("TZID"), line.param("VALUE").is_some_and(|v| v.eq_ignore_ascii_case("DATE")))
}

/// A time written, as the wall clock in `zone`.
fn local_in(written: &Written, zone: &TimeZone) -> Option<Local> {
    match written {
        Written::Utc(at) => to_local(*at, zone),
        Written::Clock(local, None) => Some(*local),
        Written::Clock(local, Some(own)) => to_local(to_utc(*local, own)?, zone),
        Written::Date(day) => Some(Local { day: *day, second: 0 }),
    }
}

/// A truthy `X-` value (`TRUE`, `true`, `1`, `yes`).
fn truthy(value: &str) -> bool {
    matches!(value.trim().to_ascii_lowercase().as_str(), "true" | "1" | "yes")
}

/// A price in cents: an integer as cents, a decimal (`45.00`, `45,00`) as euros.
fn cents(value: &str) -> Option<i64> {
    let value = value.trim();
    if let Ok(cents) = value.parse::<i64>() {
        return Some(cents);
    }
    let euros: f64 = value.replace(',', ".").parse().ok()?;
    euros.is_finite().then(|| (euros * 100.0).round() as i64)
}

/// One event's lines as a [`FeedEvent`]; none when it has no UID (nor booking
/// key) or no start that reads.
fn event(lines: &[Line], calendar_zone: &TimeZone) -> Option<FeedEvent> {
    let first = |name: &str| lines.iter().find(|l| l.name == name);
    let data: Option<Value> = first("X-PLANITY-DATA").and_then(|l| serde_json::from_str(l.value.trim()).ok().or_else(|| serde_json::from_str(&unescape(l.value.trim())).ok()));
    let data_text = |field: &str| data.as_ref().and_then(|d| d.get(field)).and_then(text_of);
    let uid = first("UID").map(|l| unescape(&l.value).trim().to_string()).filter(|u| !u.is_empty());
    let key = data_text("sq").or_else(|| uid.clone())?;
    let uid = uid.unwrap_or_else(|| key.clone());

    let start_line = first("DTSTART")?;
    let start_written = written_of(start_line, &start_line.value)?;
    let (zone, all_day) = match &start_written {
        Written::Clock(_, Some(own)) => (own.clone(), false),
        Written::Date(_) => (calendar_zone.clone(), true),
        _ => (calendar_zone.clone(), false),
    };
    let local = local_in(&start_written, &zone)?;
    let end_written = first("DTEND").and_then(|l| written_of(l, &l.value));
    let (start, end) = if all_day {
        let after = match end_written.as_ref().and_then(|w| local_in(w, &zone)) {
            Some(end) if end.day > local.day => end.day,
            _ => local.day + 1,
        };
        (local.day * 86_400, after * 86_400)
    } else {
        // An instant stays itself (a wall clock in the repeated autumn hour would not).
        let start = match &start_written {
            Written::Utc(at) => *at,
            _ => to_utc(local, &zone)?,
        };
        let end = match &end_written {
            Some(Written::Utc(at)) => Some(*at),
            Some(Written::Clock(clock, own)) => to_utc(*clock, own.as_ref().unwrap_or(&zone)),
            Some(Written::Date(day)) => to_utc(Local { day: *day, second: 0 }, &zone),
            None => None,
        };
        (start, end.filter(|end| *end >= start).unwrap_or(start))
    };

    let mut exdates = Vec::new();
    for l in lines.iter().filter(|l| l.name == "EXDATE") {
        for value in l.value.split(',') {
            match written_of(l, value) {
                Some(Written::Date(day)) => exdates.push(Exdate::Day(day)),
                Some(other) => {
                    if let Some(at) = local_in(&other, &zone) {
                        exdates.push(Exdate::At(at));
                    }
                }
                None => {}
            }
        }
    }
    let all = |name: &str| -> Vec<String> { lines.iter().filter(|l| l.name == name).map(|l| unescape(&l.value).trim().to_string()).filter(|v| !v.is_empty()).collect() };
    let prices: Vec<i64> = lines.iter().filter(|l| l.name == "X-PRICE").filter_map(|l| cents(&l.value)).collect();
    let appointment_ids = lines
        .iter()
        .filter(|l| l.name == "X-PLANITY-APPOINTMENT-IDS")
        .flat_map(|l| unescape(&l.value).split(',').map(|id| id.trim_matches(|c: char| c.is_whitespace() || c == '[' || c == ']' || c == '"').to_string()).collect::<Vec<_>>())
        .filter(|id| !id.is_empty())
        .collect();
    Some(FeedEvent {
        uid,
        key,
        zone,
        local,
        start,
        end,
        all_day,
        summary: first("SUMMARY").map(|l| unescape(&l.value).trim().to_string()).filter(|s| !s.is_empty()),
        cancelled: first("STATUS").is_some_and(|l| l.value.trim().eq_ignore_ascii_case("CANCELLED")),
        rule: first("RRULE").map(|l| l.value.trim().to_string()).filter(|r| !r.is_empty()),
        exdates,
        service_names: all("X-SERVICE-NAME"),
        service_ids: all("X-PLANITY-SERVICE-ID"),
        price: (!prices.is_empty()).then(|| prices.iter().sum()),
        prepaid: first("X-PREPAID").is_some_and(|l| truthy(&l.value)),
        booked_online: first("X-BOOKED-ONLINE").is_some_and(|l| truthy(&l.value)),
        appointment_ids,
        updated: data.as_ref().and_then(|d| d.get("uat")).and_then(millis),
        client_id: data_text("cuId"),
        calendar: first("X-PLANITY-CALENDAR-ID").map(|l| unescape(&l.value).trim().to_string()).filter(|c| !c.is_empty()).or_else(|| data_text("ca")),
    })
}

// ---------------------------------------------------------------- the series

/// One occurrence of a series: its local start date (for its UID) and its times.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Occurrence {
    pub day: i64,
    pub start: i64,
    pub end: i64,
}

/// What an expansion says beside its occurrences, for the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expansion {
    Whole,
    /// Cut at [`MAX_OCCURRENCES`].
    Cut,
    /// A rule part not read: the first occurrence only.
    FirstOnly(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Frequency {
    Daily,
    Weekly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Until {
    Utc(i64),
    Clock(Local),
    Day(i64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Rule {
    frequency: Frequency,
    interval: i64,
    /// Week days, Monday 0 … Sunday 6.
    by_day: Vec<i64>,
    week_start: i64,
    until: Option<Until>,
}

/// Monday 0 … Sunday 6, of a date in days since 1970 (a Thursday).
fn weekday(day: i64) -> i64 {
    (day + 3).rem_euclid(7)
}

fn weekday_code(code: &str) -> Option<i64> {
    ["MO", "TU", "WE", "TH", "FR", "SA", "SU"].iter().position(|c| code.eq_ignore_ascii_case(c)).map(|p| p as i64)
}

impl Rule {
    /// The rule read; the part it does not read, otherwise.
    fn read(text: &str) -> Result<Self, String> {
        let mut rule = Self { frequency: Frequency::Daily, interval: 1, by_day: Vec::new(), week_start: 0, until: None };
        let mut frequency = None;
        for part in text.split(';').map(str::trim).filter(|p| !p.is_empty()) {
            let (name, value) = part.split_once('=').ok_or_else(|| part.to_string())?;
            let unread = || part.to_string();
            match name.trim().to_ascii_uppercase().as_str() {
                "FREQ" => {
                    frequency = Some(match value.trim().to_ascii_uppercase().as_str() {
                        "DAILY" => Frequency::Daily,
                        "WEEKLY" => Frequency::Weekly,
                        _ => return Err(unread()),
                    })
                }
                "INTERVAL" => rule.interval = value.trim().parse::<i64>().ok().filter(|n| (1..=1000).contains(n)).ok_or_else(unread)?,
                "BYDAY" => rule.by_day = value.split(',').map(|code| weekday_code(code.trim())).collect::<Option<Vec<_>>>().ok_or_else(unread)?,
                "WKST" => rule.week_start = weekday_code(value.trim()).ok_or_else(unread)?,
                "UNTIL" => {
                    rule.until = Some(match written(value, None, false).ok_or_else(unread)? {
                        Written::Utc(at) => Until::Utc(at),
                        Written::Clock(local, _) => Until::Clock(local),
                        Written::Date(day) => Until::Day(day),
                    })
                }
                _ => return Err(unread()),
            }
        }
        rule.frequency = frequency.ok_or_else(|| "no FREQ".to_string())?;
        Ok(rule)
    }
}

/// A series' occurrences that start within `from..=until` (seconds since
/// 1970), in order, at most [`MAX_OCCURRENCES`]; an event with no rule gives
/// none (it is not a series). Each keeps the first's wall-clock time of day
/// in its zone (summer or winter time alike) and its duration. `DTSTART` is
/// always the first occurrence (RFC 5545 §3.8.5.3), unless an `EXDATE`
/// takes it out.
pub fn expand(event: &FeedEvent, from: i64, until: i64) -> (Vec<Occurrence>, Expansion) {
    let Some(text) = event.rule.as_deref() else { return (Vec::new(), Expansion::Whole) };
    let duration = (event.end - event.start).max(0);
    let first = Occurrence { day: event.local.day, start: event.start, end: event.end };
    let within = |o: &Occurrence| (from..=until).contains(&o.start);
    let rule = match Rule::read(text) {
        Ok(rule) => rule,
        Err(part) => return (Some(first).filter(within).into_iter().collect(), Expansion::FirstOnly(part)),
    };
    let excluded = |local: Local| event.exdates.iter().any(|x| match x {
        Exdate::At(at) => *at == local,
        Exdate::Day(day) => *day == local.day,
    });
    let after_until = |local: Local, start: i64| match rule.until {
        Some(Until::Utc(at)) => start > at,
        Some(Until::Clock(clock)) => local > clock,
        Some(Until::Day(day)) => local.day > day,
        None => false,
    };
    let occurrence = |day: i64| -> Option<(Local, Occurrence)> {
        let local = Local { day, second: event.local.second };
        let start = if event.all_day { day * 86_400 } else { to_utc(local, &event.zone)? };
        Some((local, Occurrence { day, start, end: start + duration }))
    };

    // The local dates the window spans, a day of margin each side.
    let (from_day, until_day) = match (to_local(from, &event.zone), to_local(until, &event.zone)) {
        (Some(a), Some(b)) => (a.day - 1, b.day + 1),
        _ => (from.div_euclid(86_400) - 1, until.div_euclid(86_400) + 1),
    };
    // One period of the rule, and the day offsets in it from its first day.
    let (period, base, offsets) = match rule.frequency {
        Frequency::Daily => (rule.interval, event.local.day, vec![0]),
        Frequency::Weekly => {
            let base = event.local.day - (weekday(event.local.day) - rule.week_start).rem_euclid(7);
            let days = if rule.by_day.is_empty() { vec![weekday(event.local.day)] } else { rule.by_day.clone() };
            let mut offsets: Vec<i64> = days.iter().map(|d| (d - rule.week_start).rem_euclid(7)).collect();
            offsets.sort_unstable();
            offsets.dedup();
            (7 * rule.interval, base, offsets)
        }
    };
    let filter_days = rule.frequency == Frequency::Daily && !rule.by_day.is_empty();

    let mut out: Vec<Occurrence> = Vec::new();
    let mut note = Expansion::Whole;
    let mut given_first = false;
    // Start at the period just before the window, never before the first.
    let first_period = ((from_day - base).div_euclid(period) - 1).max(0);
    let mut k = first_period;
    'periods: loop {
        let period_day = base + k * period;
        // The window's end; and a bound of steps, should a date never convert.
        if period_day > until_day || k - first_period > 200_000 {
            break;
        }
        for offset in &offsets {
            let day = period_day + offset;
            if day < event.local.day || (filter_days && !rule.by_day.contains(&weekday(day))) {
                continue;
            }
            if day > until_day {
                break 'periods;
            }
            let (local, o) = if day == event.local.day {
                // The first is DTSTART itself, whatever UNTIL says.
                given_first = true;
                (event.local, first)
            } else {
                let Some((local, o)) = occurrence(day) else { continue };
                if after_until(local, o.start) {
                    break 'periods;
                }
                (local, o)
            };
            if excluded(local) || !within(&o) {
                continue;
            }
            if out.len() == MAX_OCCURRENCES {
                note = Expansion::Cut;
                break 'periods;
            }
            out.push(o);
        }
        k += 1;
    }
    // DTSTART counts even when the rule's days do not hold it.
    if !given_first && within(&first) && !excluded(event.local) {
        out.insert(0, first);
        if out.len() > MAX_OCCURRENCES {
            out.pop();
            note = Expansion::Cut;
        }
    }
    (out, note)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paris() -> TimeZone {
        jiff::tz::db().get("Europe/Paris").unwrap()
    }

    /// A one-event feed around the given lines.
    fn one(lines: &str) -> FeedEvent {
        let text = format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nX-WR-CALNAME:Institut Exemple\r\nX-WR-TIMEZONE:Europe/Paris\r\nBEGIN:VEVENT\r\n{lines}END:VEVENT\r\nEND:VCALENDAR\r\n");
        let feed = read(&text).unwrap();
        assert_eq!(feed.skipped, 0, "{text}");
        feed.events.into_iter().next().unwrap()
    }

    fn at(y: i64, m: u32, d: u32, h: i64, mi: i64) -> i64 {
        to_utc(Local { day: ical::days(y, m, d), second: h * 3600 + mi * 60 }, &paris()).unwrap()
    }

    #[test]
    fn lines_unfold_and_values_unescape() {
        let lines = unfolded("BEGIN:VCALENDAR\r\nSUMMARY:Institut Ex\r\n emple - Coupe\\, couleur\r\n\tet soin\nEND:VCALENDAR");
        assert_eq!(lines[1], "SUMMARY:Institut Exemple - Coupe\\, couleuret soin");
        assert_eq!(unescape("a\\,b\\;c\\nd\\\\e\\N"), "a,b;c\nd\\e\n");
        let l = line("DTSTART;TZID=\"Europe/Paris\";X-A=b:20261125T143000").unwrap();
        assert_eq!((l.name.as_str(), l.param("TZID"), l.param("X-A"), l.value.as_str()), ("DTSTART", Some("Europe/Paris"), Some("b"), "20261125T143000"));
        assert_eq!(line("RRULE:FREQ=WEEKLY;INTERVAL=2;BYDAY=TU,TH").unwrap().value, "FREQ=WEEKLY;INTERVAL=2;BYDAY=TU,TH");
        assert_eq!(read("not a calendar").map(|f| f.events.len()), None);
    }

    #[test]
    fn an_event_reads_its_times_services_and_data() {
        let event = one(concat!(
            "UID:-Vinvented0000000001\r\n",
            "DTSTART;TZID=Europe/Paris:20261125T143000\r\n",
            "DTEND;TZID=Europe/Paris:20261125T150000\r\n",
            "SUMMARY:Institut Exemple - Coupe\r\n",
            "X-SERVICE-NAME:Coupe\r\n",
            "X-PLANITY-SERVICE-ID:-Sinvented0000000001\r\n",
            "X-PRICE:4500\r\n",
            "X-PREPAID:TRUE\r\n",
            "X-BOOKED-ONLINE:false\r\n",
            "X-PLANITY-DATA:{\"sq\":\"-Vinvented0000000001\",\"id\":\"-Vinvented0000000001\",\"ca\":\"-Cinvented0000000000\",\"uat\":1763700000000,\"cuId\":\"clientinvented0000000000001a\"}\r\n",
            "BEGIN:VALARM\r\nSUMMARY:not the event's\r\nEND:VALARM\r\n",
        ));
        assert_eq!((event.start, event.end), (at(2026, 11, 25, 14, 30), at(2026, 11, 25, 15, 0)));
        assert_eq!(event.start, ical::days(2026, 11, 25) * 86_400 + 13 * 3600 + 30 * 60, "14:30 Paris in winter is 13:30 UTC");
        assert_eq!(event.summary.as_deref(), Some("Institut Exemple - Coupe"), "an alarm's lines are not the event's");
        assert_eq!((event.service_names.clone(), event.price, event.prepaid, event.booked_online), (vec!["Coupe".to_string()], Some(4500), true, false));
        assert_eq!((event.key.as_str(), event.updated, event.client_id.as_deref(), event.calendar.as_deref()), ("-Vinvented0000000001", Some(1_763_700_000_000), Some("clientinvented0000000000001a"), Some("-Cinvented0000000000")));
        // An escaped JSON value and a comma list of ids read too; a date event is whole-day.
        let event = one(concat!(
            "UID:-Vinvented0000000002\r\n",
            "DTSTART;VALUE=DATE:20261224\r\n",
            "X-PLANITY-APPOINTMENT-IDS:-Vinvented0000000002\\,-Vinvented0000000003\r\n",
            "X-PLANITY-DATA:{\"sq\":\"-Qinvented0000000002\"\\,\"uat\":\"2026-11-20T09:00:00Z\"}\r\n",
        ));
        assert!(event.all_day);
        assert_eq!((event.start, event.end), (ical::days(2026, 12, 24) * 86_400, ical::days(2026, 12, 25) * 86_400));
        assert_eq!(event.appointment_ids, vec!["-Vinvented0000000002", "-Vinvented0000000003"]);
        assert_eq!(event.key, "-Qinvented0000000002");
        assert_eq!(event.updated, Some(1_795_165_200_000));
        // No start, or no UID nor key: skipped and counted.
        let feed = read("BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:x\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nDTSTART:20261125T100000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n").unwrap();
        assert_eq!((feed.events.len(), feed.skipped), (0, 2));
    }

    #[test]
    fn a_daily_series_every_seven_days_until_a_date_without_its_exdate() {
        let event = one(concat!(
            "UID:-Vinvented0000000010\r\n",
            "DTSTART;TZID=Europe/Paris:20261007T100000\r\n",
            "DTEND;TZID=Europe/Paris:20261007T110000\r\n",
            "RRULE:FREQ=DAILY;INTERVAL=7;UNTIL=20261230T225959Z\r\n",
            "EXDATE;TZID=Europe/Paris:20261111T100000\r\n",
        ));
        let (all, note) = expand(&event, at(2026, 9, 1, 0, 0), at(2027, 3, 1, 0, 0));
        assert_eq!(note, Expansion::Whole);
        let days: Vec<String> = all.iter().map(|o| ical::date_stamp(o.day)).collect();
        // 7 Oct to 30 Dec every week is 13 Wednesdays, less 11 Nov.
        assert_eq!(days.len(), 12, "{days:?}");
        assert!(!days.contains(&"20261111".to_string()) && days[0] == "20261007" && days[11] == "20261230", "{days:?}");
        // The wall clock stays 10:00 across the change to winter time (25 Oct).
        assert_eq!(all[0].start, at(2026, 10, 7, 10, 0));
        assert_eq!(all[0].start, ical::days(2026, 10, 7) * 86_400 + 8 * 3600, "summer: 08:00 UTC");
        assert_eq!(all[3].start, ical::days(2026, 10, 28) * 86_400 + 9 * 3600, "winter: 09:00 UTC");
        assert!(all.iter().all(|o| o.end - o.start == 3600));
        // The window clips both ends.
        let (inside, _) = expand(&event, at(2026, 11, 1, 0, 0), at(2026, 12, 1, 0, 0));
        assert_eq!(inside.iter().map(|o| ical::date_stamp(o.day)).collect::<Vec<_>>(), vec!["20261104", "20261118", "20261125"]);
    }

    #[test]
    fn a_weekly_series_every_other_week_on_two_days_without_end() {
        // Tuesday 3 Nov 2026, every other week on Tuesday and Thursday, no UNTIL.
        let event = one(concat!(
            "UID:-Vinvented0000000020\r\n",
            "DTSTART;TZID=Europe/Paris:20261103T090000\r\n",
            "DTEND;TZID=Europe/Paris:20261103T093000\r\n",
            "RRULE:FREQ=WEEKLY;INTERVAL=2;BYDAY=TU,TH\r\n",
        ));
        let (all, note) = expand(&event, at(2026, 11, 1, 0, 0), at(2026, 12, 31, 0, 0));
        assert_eq!(note, Expansion::Whole);
        let days: Vec<String> = all.iter().map(|o| ical::date_stamp(o.day)).collect();
        assert_eq!(days, vec!["20261103", "20261105", "20261117", "20261119", "20261201", "20261203", "20261215", "20261217", "20261229"]);
        // Far into the series, the stepping stays on the same weeks.
        let (later, _) = expand(&event, at(2027, 6, 1, 0, 0), at(2027, 6, 30, 0, 0));
        assert_eq!(later.iter().map(|o| ical::date_stamp(o.day)).collect::<Vec<_>>(), vec!["20270601", "20270603", "20270615", "20270617", "20270629"]);
        // A Wednesday start with BYDAY on other days still counts its start first.
        let event = one("UID:x\r\nDTSTART;TZID=Europe/Paris:20261104T090000\r\nRRULE:FREQ=WEEKLY;BYDAY=FR;UNTIL=20261113\r\n");
        let (all, _) = expand(&event, at(2026, 11, 1, 0, 0), at(2026, 12, 1, 0, 0));
        assert_eq!(all.iter().map(|o| ical::date_stamp(o.day)).collect::<Vec<_>>(), vec!["20261104", "20261106", "20261113"], "UNTIL as a date is inclusive");
    }

    #[test]
    fn a_series_is_cut_at_four_hundred_and_an_unread_rule_keeps_its_first() {
        let event = one("UID:-Vinvented0000000030\r\nDTSTART;TZID=Europe/Paris:20200101T080000\r\nRRULE:FREQ=DAILY\r\n");
        let (all, note) = expand(&event, at(2026, 1, 1, 0, 0), at(2028, 1, 1, 0, 0));
        assert_eq!((all.len(), note), (MAX_OCCURRENCES, Expansion::Cut));
        assert_eq!(ical::date_stamp(all[0].day), "20260101", "counted from the window, not from the series' first date");
        for rule in ["FREQ=MONTHLY", "FREQ=WEEKLY;COUNT=5", "FREQ=WEEKLY;BYDAY=1MO", "FREQ=DAILY;BYHOUR=9", "INTERVAL=2"] {
            let event = one(&format!("UID:-Vinvented0000000031\r\nDTSTART;TZID=Europe/Paris:20261110T080000\r\nRRULE:{rule}\r\n"));
            let (all, note) = expand(&event, at(2026, 11, 1, 0, 0), at(2027, 3, 1, 0, 0));
            assert_eq!(all.iter().map(|o| ical::date_stamp(o.day)).collect::<Vec<_>>(), vec!["20261110"], "{rule}");
            assert!(matches!(note, Expansion::FirstOnly(_)), "{rule}: {note:?}");
        }
        // An event without a rule is no series.
        let event = one("UID:x\r\nDTSTART:20261110T080000Z\r\n");
        assert_eq!(expand(&event, 0, i64::MAX / 2).0, Vec::new());
    }
}
