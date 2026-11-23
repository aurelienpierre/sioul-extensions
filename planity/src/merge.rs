// SPDX-License-Identifier: GPL-3.0-or-later

//! One event per booking, from the two sources that each hold half of it
//! (`merge-design.md`): the official feed (times, services, prices,
//! cancellations, recurrences) and the Firestore `vevents` documents (the
//! client's name and phone, the note, the all-day blocks, the bookings
//! older than the feed's window).
//!
//! - **The booking key** is `sq`: the feed's `X-PLANITY-DATA.sq`, a
//!   document's `sq` field, else its id. A booking keeps one UID whichever
//!   source holds it (a recurring occurrence: the key, `-`, its local date).
//! - **The join**: a feed booking takes the documents whose id is one of its
//!   appointment ids or its `UID`, and those of the same `sq`.
//! - **The fresher record by `uat`** gives the times and the cancelled state;
//!   a tie, or a time unknown on either side, goes to the feed.
//! - **A series cancelled as a whole** hands back no occurrence (a cancelled
//!   one-off booking is handed back cancelled).
//! - **Maps from everything read**: a client id to the client's name and
//!   phone (the documents' `cu`), a service id to its name (the feed's), so
//!   that a booking only one source holds is named too.

use crate::feed::{self, Expansion, Feed, FeedEvent};
use crate::planityapi::Document;
use serde_json::Value;
use sioul_extension::ical::{self, Event};
use std::collections::{BTreeMap, BTreeSet};

/// A whole day of minutes (an all-day block is `d` ≈ 1439).
const DAY_MINUTES: i64 = 1440;

/// A `vevents` document, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Appointment {
    /// The document id.
    pub id: String,
    /// Its booking key: its `sq`, else its id.
    pub key: String,
    /// Its calendar (`ca`, else `sca`), if it names one.
    pub calendar: Option<String>,
    /// Seconds since 1970, UTC; for an all-day block, UTC midnight of its dates.
    pub start: i64,
    pub end: i64,
    pub minutes: i64,
    pub all_day: bool,
    pub cancelled: bool,
    pub client_id: Option<String>,
    pub client_name: Option<String>,
    pub phone: Option<String>,
    pub note: Option<String>,
    /// Its service id (`se`).
    pub service: Option<String>,
    /// `p`, cents.
    pub price: Option<i64>,
    /// `uat`, milliseconds.
    pub updated: Option<i64>,
}

fn field_text(fields: &Value, name: &str) -> Option<String> {
    fields.get(name).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty()).map(str::to_string)
}

impl Appointment {
    /// A document as an appointment; none when its start does not read.
    pub fn read(document: &Document, zone: &jiff::tz::TimeZone) -> Option<Self> {
        let fields = &document.fields;
        let start_text = fields.get("s").and_then(Value::as_str)?;
        let minutes = fields.get("d").and_then(Value::as_i64).unwrap_or(0).max(0);
        let all_day = fields.get("ad").and_then(Value::as_bool).unwrap_or(false);
        let (start, end) = if all_day {
            let (year, month, day) = crate::parse_date(start_text)?;
            let first = ical::days(year, month, day) * 86_400;
            let span_days = ((minutes + DAY_MINUTES - 1) / DAY_MINUTES).max(1);
            (first, first + span_days * 86_400)
        } else {
            let start = crate::parse_local(start_text, zone)?;
            (start, start + minutes * 60)
        };
        let client = fields.get("cu").filter(|cu| cu.is_object());
        let client_text = |name: &str| client.and_then(|cu| field_text(cu, name));
        Some(Self {
            id: document.id.clone(),
            key: field_text(fields, "sq").unwrap_or_else(|| document.id.clone()),
            calendar: field_text(fields, "ca").or_else(|| field_text(fields, "sca")),
            start,
            end,
            minutes,
            all_day,
            cancelled: fields.get("dat").is_some_and(|value| !value.is_null()),
            client_id: client_text("id"),
            client_name: client_text("name"),
            phone: client_text("phone"),
            note: field_text(fields, "t"),
            service: field_text(fields, "se"),
            price: fields.get("p").and_then(Value::as_i64),
            updated: fields.get("uat").and_then(feed::millis),
        })
    }
}

/// What a merge gives: the events of each calendar, and what to say in the log.
#[derive(Debug, Default)]
pub struct Merged {
    pub calendars: BTreeMap<String, Vec<Event>>,
    pub log: Vec<String>,
}

/// One booking, before it is an event.
#[derive(Debug, Clone, Default)]
struct Booking {
    uid: String,
    calendar: String,
    start: i64,
    end: i64,
    all_day: bool,
    cancelled: bool,
    client: Option<String>,
    phone: Option<String>,
    /// A client is named by id, whether or not their name is known.
    has_client: bool,
    /// Each service, with its minutes when known.
    services: Vec<(String, Option<i64>)>,
    price: Option<i64>,
    prepaid: bool,
    booked_online: bool,
    notes: Vec<String>,
}

impl Booking {
    fn event(&self, french: bool) -> Event {
        let services = self.services.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>().join(", ");
        let summary = match (self.client.as_deref(), services.is_empty()) {
            (Some(client), false) => format!("{client} — {services}"),
            (Some(client), true) => client.to_string(),
            (None, false) => services,
            (None, true) if self.has_client => if french { "Rendez-vous" } else { "Appointment" }.to_string(),
            (None, true) => if french { "Indisponible" } else { "Unavailable" }.to_string(),
        };
        let mut parts: Vec<String> = Vec::new();
        for (name, minutes) in &self.services {
            parts.push(match minutes {
                Some(minutes) => format!("{name} ({minutes} min)"),
                None => name.clone(),
            });
        }
        let minutes = (self.end - self.start) / 60;
        if self.services.is_empty() && !self.all_day && minutes > 0 {
            parts.push(if french { format!("Durée\u{a0}: {minutes} min") } else { format!("Duration: {minutes} min") });
        }
        if let Some(cents) = self.price {
            let (euros, remainder) = (cents / 100, (cents % 100).abs());
            parts.push(format!("{euros},{remainder:02}\u{a0}€"));
        }
        if self.prepaid {
            parts.push(if french { "Prépayé" } else { "Prepaid" }.to_string());
        }
        if self.booked_online {
            parts.push(if french { "Réservé en ligne" } else { "Booked online" }.to_string());
        }
        if let Some(phone) = &self.phone {
            parts.push(phone.clone());
        }
        parts.extend(self.notes.iter().cloned());
        Event {
            uid: self.uid.clone(),
            start: self.start,
            end: self.end,
            summary,
            location: None,
            description: (!parts.is_empty()).then(|| parts.join("\n")),
            cancelled: self.cancelled,
            // The agenda declares busy; the door writes the transparency from the person's choice.
            transparent: false,
            all_day: self.all_day,
        }
    }
}

/// The feed's events of one booking key (one, unless the feed splits a booking).
struct FeedBooking<'a> {
    events: Vec<&'a FeedEvent>,
}

impl FeedBooking<'_> {
    fn first(&self) -> &FeedEvent {
        self.events[0]
    }
    fn start(&self) -> i64 {
        self.events.iter().map(|e| e.start).min().unwrap_or(0)
    }
    fn end(&self) -> i64 {
        self.events.iter().map(|e| e.end).max().unwrap_or(0)
    }
    fn updated(&self) -> Option<i64> {
        self.events.iter().filter_map(|e| e.updated).max()
    }
    fn names(&self) -> Vec<(&str, Option<&str>)> {
        // A name with its service id, when the event pairs them one to one.
        self.events
            .iter()
            .flat_map(|e| {
                let paired = e.service_ids.len() == e.service_names.len();
                e.service_names.iter().enumerate().map(move |(i, name)| (name.as_str(), if paired { e.service_ids.get(i).map(String::as_str) } else { None }))
            })
            .collect()
    }
    fn price(&self) -> Option<i64> {
        let prices: Vec<i64> = self.events.iter().filter_map(|e| e.price).collect();
        (!prices.is_empty()).then(|| prices.iter().sum())
    }
}

/// The merge. `feed` is none when it was not read; `window` the read
/// window, which the feed's bookings are clipped to (Firestore's come from
/// it already); `calendar` the feed's own calendar id (the link's), for a
/// feed event that names none.
pub fn merge(feed: Option<&Feed>, appointments: &[Appointment], window: (i64, i64), calendar: Option<&str>, french: bool) -> Merged {
    let mut merged = Merged::default();
    let calname = feed.and_then(|f| f.name.as_deref());
    let events: &[FeedEvent] = feed.map(|f| f.events.as_slice()).unwrap_or(&[]);

    // Maps from everything read.
    let mut services: BTreeMap<&str, &str> = BTreeMap::new();
    for event in events {
        if event.service_ids.len() == event.service_names.len() {
            for (id, name) in event.service_ids.iter().zip(&event.service_names) {
                services.entry(id.as_str()).or_insert(name.as_str());
            }
        }
    }
    // A client's name and phone from their most recently updated document.
    let mut clients: BTreeMap<&str, (&Appointment, i64)> = BTreeMap::new();
    for appointment in appointments.iter().filter(|a| a.client_name.is_some()) {
        if let Some(id) = appointment.client_id.as_deref() {
            let updated = appointment.updated.unwrap_or(i64::MIN);
            if clients.get(id).is_none_or(|(_, seen)| updated > *seen) {
                clients.insert(id, (appointment, updated));
            }
        }
    }
    let mut by_id: BTreeMap<&str, usize> = BTreeMap::new();
    let mut by_key: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, appointment) in appointments.iter().enumerate() {
        by_id.entry(appointment.id.as_str()).or_insert(i);
        by_key.entry(appointment.key.as_str()).or_default().push(i);
    }
    let mut used = vec![false; appointments.len()];

    // The feed's one-off events by booking key (the same UID twice is one event), and its series.
    let mut one_offs: Vec<(String, FeedBooking)> = Vec::new();
    let mut one_off_at: BTreeMap<&str, usize> = BTreeMap::new();
    let mut series: Vec<&FeedEvent> = Vec::new();
    for event in events {
        if event.rule.is_some() {
            series.push(event);
            continue;
        }
        match one_off_at.get(event.key.as_str()) {
            Some(&at) if one_offs[at].1.events.iter().any(|e| e.uid == event.uid) => {}
            Some(&at) => one_offs[at].1.events.push(event),
            None => {
                one_off_at.insert(event.key.as_str(), one_offs.len());
                one_offs.push((event.key.clone(), FeedBooking { events: vec![event] }));
            }
        }
    }

    let mut bookings: Vec<Booking> = Vec::new();
    let (mut joined_count, mut feed_only_count, mut firestore_wins) = (0usize, 0usize, 0usize);
    let (from, until) = window;
    for (key, fb) in &one_offs {
        let joined = join(fb.events.iter().flat_map(|e| e.appointment_ids.iter().map(String::as_str).chain([e.uid.as_str()])), key, &by_id, &by_key, &mut used, appointments);
        if joined.is_empty() {
            feed_only_count += 1;
        } else {
            joined_count += 1;
        }
        let considered = considered(&joined);
        // The fresher record by `uat` gives the times and the cancelled state; a tie goes to the feed.
        let firestore_fresher = match (fb.updated(), joined.iter().filter_map(|a| a.updated).max()) {
            (Some(feed_at), Some(firestore_at)) => firestore_at > feed_at,
            _ => false,
        };
        let mut booking = if firestore_fresher {
            firestore_wins += 1;
            Booking {
                start: considered.iter().map(|a| a.start).min().unwrap_or(0),
                end: considered.iter().map(|a| a.end).max().unwrap_or(0),
                all_day: considered.iter().all(|a| a.all_day),
                cancelled: joined.iter().all(|a| a.cancelled),
                ..Booking::default()
            }
        } else {
            let (start, end, all_day) = feed_times(fb.first().local.day, fb.start(), fb.end(), fb.events.iter().all(|e| e.all_day), &considered);
            Booking { start, end, all_day, cancelled: fb.events.iter().all(|e| e.cancelled), ..Booking::default() }
        };
        if !(from..=until).contains(&booking.start) {
            continue;
        }
        booking.uid = key.clone();
        fill(&mut booking, fb, &joined, &considered, &services, &clients, calname, calendar);
        bookings.push(booking);
    }

    let mut cancelled_series = 0usize;
    for event in &series {
        let fb = FeedBooking { events: vec![event] };
        // The documents of a series are its own, whatever becomes of it.
        let joined = join(event.appointment_ids.iter().map(String::as_str).chain([event.uid.as_str()]), &event.key, &by_id, &by_key, &mut used, appointments);
        // A series cancelled as a whole hands back no occurrence: none of its dates holds a
        // booking any more (a cancelled one-off booking, by contrast, is said cancelled).
        if event.cancelled {
            cancelled_series += 1;
            continue;
        }
        let considered = considered(&joined);
        let (occurrences, expansion) = feed::expand(event, from, until);
        match &expansion {
            Expansion::Whole => {}
            Expansion::Cut => merged.log.push(format!("a series was cut at {} occurrences", feed::MAX_OCCURRENCES)),
            Expansion::FirstOnly(part) => merged.log.push(format!("a series' rule part does not read ({part}): its first occurrence only")),
        }
        for occurrence in occurrences {
            let (start, end, all_day) = feed_times(occurrence.day, occurrence.start, occurrence.end, event.all_day, &considered);
            let mut booking = Booking { uid: format!("{}-{}", event.key, ical::date_stamp(occurrence.day)), start, end, all_day, cancelled: event.cancelled, ..Booking::default() };
            fill(&mut booking, &fb, &joined, &considered, &services, &clients, calname, calendar);
            bookings.push(booking);
        }
    }

    // What the feed did not take: one booking per key, from Firestore alone.
    let mut rest: Vec<(&str, Vec<&Appointment>)> = Vec::new();
    let mut rest_at: BTreeMap<&str, usize> = BTreeMap::new();
    for (i, appointment) in appointments.iter().enumerate() {
        if used[i] {
            continue;
        }
        match rest_at.get(appointment.key.as_str()) {
            Some(&at) => rest[at].1.push(appointment),
            None => {
                rest_at.insert(appointment.key.as_str(), rest.len());
                rest.push((appointment.key.as_str(), vec![appointment]));
            }
        }
    }
    let firestore_only = rest.len();
    for (key, group) in &rest {
        let considered = considered(group);
        let mut booking = Booking {
            uid: (*key).to_string(),
            start: considered.iter().map(|a| a.start).min().unwrap_or(0),
            end: considered.iter().map(|a| a.end).max().unwrap_or(0),
            all_day: considered.iter().all(|a| a.all_day),
            cancelled: group.iter().all(|a| a.cancelled),
            ..Booking::default()
        };
        fill(&mut booking, &FeedBooking { events: Vec::new() }, group, &considered, &services, &clients, calname, calendar);
        bookings.push(booking);
    }

    merged.log.push(format!(
        "merged: {} one-off booking(s) from the feed ({joined_count} joined, {feed_only_count} feed only, {firestore_wins} with fresher Firestore times), {} series, {firestore_only} from Firestore alone",
        one_offs.len(),
        series.len() - cancelled_series
    ));
    if cancelled_series > 0 {
        merged.log.push(format!("{cancelled_series} series cancelled as a whole left out"));
    }

    // One event per UID in each calendar, in order.
    let mut doubles = 0;
    let mut given: BTreeSet<(&str, &str)> = BTreeSet::new();
    for booking in &bookings {
        if !given.insert((booking.calendar.as_str(), booking.uid.as_str())) {
            doubles += 1;
            continue;
        }
        merged.calendars.entry(booking.calendar.clone()).or_default().push(booking.event(french));
    }
    if doubles > 0 {
        merged.log.push(format!("{doubles} booking(s) with a UID already given were left out"));
    }
    for events in merged.calendars.values_mut() {
        events.sort_by(|a, b| (a.start, &a.uid).cmp(&(b.start, &b.uid)));
    }
    merged
}

/// The documents a feed booking takes and has not been taken: those whose id
/// it names, then those of its key, in the order of their start.
fn join<'a, 'b>(ids: impl Iterator<Item = &'b str>, key: &str, by_id: &BTreeMap<&str, usize>, by_key: &BTreeMap<&str, Vec<usize>>, used: &mut [bool], appointments: &'a [Appointment]) -> Vec<&'a Appointment> {
    let mut taken: Vec<usize> = Vec::new();
    for id in ids {
        if let Some(&i) = by_id.get(id) {
            if !used[i] {
                used[i] = true;
                taken.push(i);
            }
        }
    }
    for &i in by_key.get(key).map(Vec::as_slice).unwrap_or(&[]) {
        if !used[i] {
            used[i] = true;
            taken.push(i);
        }
    }
    let mut joined: Vec<&Appointment> = taken.into_iter().map(|i| &appointments[i]).collect();
    joined.sort_by(|a, b| (a.start, &a.id).cmp(&(b.start, &b.id)));
    joined
}

/// The documents a booking's times, services and price come from: those not
/// cancelled, or all when every one is.
fn considered<'a>(joined: &[&'a Appointment]) -> Vec<&'a Appointment> {
    let live: Vec<&Appointment> = joined.iter().copied().filter(|a| !a.cancelled).collect();
    if live.is_empty() { joined.to_vec() } else { live }
}

/// The feed's times; a block Firestore holds as all-day stays all-day, on the feed's local date.
fn feed_times(local_day: i64, start: i64, end: i64, all_day: bool, considered: &[&Appointment]) -> (i64, i64, bool) {
    if all_day {
        return (start, end, true);
    }
    if !considered.is_empty() && considered.iter().all(|a| a.all_day) {
        let days = ((end - start) + 86_399) / 86_400;
        let first = local_day * 86_400;
        return (first, first + days.max(1) * 86_400, true);
    }
    (start, end, false)
}

/// A booking's client, services, price, flags, notes and calendar, from what was joined.
#[allow(clippy::too_many_arguments)]
fn fill(booking: &mut Booking, fb: &FeedBooking, joined: &[&Appointment], considered: &[&Appointment], services: &BTreeMap<&str, &str>, clients: &BTreeMap<&str, (&Appointment, i64)>, calname: Option<&str>, calendar: Option<&str>) {
    // The client: the documents' own `cu`, else the feed's `cuId` through the map.
    let own = joined.iter().find(|a| a.client_name.is_some());
    let feed_client = fb.events.iter().find_map(|e| e.client_id.as_deref());
    let mapped = feed_client.and_then(|id| clients.get(id)).map(|(a, _)| *a);
    let source = own.copied().or(mapped);
    booking.client = source.and_then(|a| a.client_name.clone());
    booking.phone = source.and_then(|a| a.phone.clone()).or_else(|| joined.iter().find_map(|a| a.phone.clone()));
    booking.has_client = feed_client.is_some() || joined.iter().any(|a| a.client_id.is_some());

    // The services: the feed's names, else its summary without the calendar's name, else the map.
    let names = fb.names();
    let span = (!booking.all_day).then(|| (booking.end - booking.start) / 60).filter(|m| *m > 0);
    if !names.is_empty() {
        let mut taken = vec![false; considered.len()];
        for (name, id) in &names {
            let minutes = id.and_then(|id| {
                let at = considered.iter().enumerate().position(|(i, a)| !taken[i] && a.service.as_deref() == Some(id))?;
                taken[at] = true;
                Some(considered[at].minutes).filter(|m| *m > 0)
            });
            booking.services.push(((*name).to_string(), minutes));
        }
        if booking.services.len() == 1 && booking.services[0].1.is_none() {
            booking.services[0].1 = span;
        }
    } else if let Some(title) = fb.events.iter().find_map(|e| stripped(e.summary.as_deref()?, calname)) {
        booking.services.push((title, span));
    } else {
        for appointment in considered {
            if let Some(name) = appointment.service.as_deref().and_then(|id| services.get(id)) {
                booking.services.push(((*name).to_string(), Some(appointment.minutes).filter(|m| *m > 0 && !appointment.all_day)));
            }
        }
    }

    // The price: the feed's, else the documents' added.
    booking.price = fb.price().or_else(|| {
        let prices: Vec<i64> = considered.iter().filter_map(|a| a.price).collect();
        (!prices.is_empty()).then(|| prices.iter().sum())
    });
    booking.prepaid = fb.events.iter().any(|e| e.prepaid);
    booking.booked_online = fb.events.iter().any(|e| e.booked_online);
    for note in joined.iter().filter_map(|a| a.note.as_deref()) {
        if !booking.notes.iter().any(|n| n == note) {
            booking.notes.push(note.to_string());
        }
    }
    booking.calendar = fb
        .events
        .iter()
        .find_map(|e| e.calendar.clone())
        .or_else(|| joined.iter().find_map(|a| a.calendar.clone()))
        .or_else(|| calendar.map(str::to_string))
        .unwrap_or_else(|| "planity".to_string());
}

/// The feed's summary with `"<calendar name> - "` taken off its front; none when nothing is left.
fn stripped(summary: &str, calname: Option<&str>) -> Option<String> {
    let summary = summary.trim();
    let rest = calname
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .and_then(|name| {
            let after = summary.strip_prefix(name)?.trim_start();
            if after.is_empty() { Some("") } else { after.strip_prefix('-').map(str::trim) }
        })
        .unwrap_or(summary);
    Some(rest.to_string()).filter(|r| !r.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_calendar_name_is_taken_off_the_summary() {
        assert_eq!(stripped("Institut Exemple - Coupe", Some(" Institut Exemple ")).as_deref(), Some("Coupe"));
        assert_eq!(stripped("Institut Exemple", Some("Institut Exemple")), None);
        assert_eq!(stripped("Pause", Some("Institut Exemple")).as_deref(), Some("Pause"));
        assert_eq!(stripped("Institut Exemple - ", Some("Institut Exemple")), None);
        assert_eq!(stripped("Institut Exemplaire", Some("Institut Exemple")).as_deref(), Some("Institut Exemplaire"), "a longer word is not the name");
        assert_eq!(stripped("  ", None), None);
    }
}
