// SPDX-License-Identifier: GPL-3.0-or-later

//! The salon's clients as one set of contacts (Sioul's `docs/extensions.md`,
//! "What it hands back: contacts"), which Sioul writes into an address book
//! the person chooses, only ever adding to it. Each appointment document
//! carries its client as `cu = {id, name, email, phone}`, `id` a stable id
//! (`api.md`); the set holds every client seen in the documents read,
//! whatever became of the appointment (a cancelled booking's client is still
//! a client). Not yet the salon's whole client list: only the clients of the
//! appointments in the window read.
//!
//! - **One person per `cu.id`**, which Sioul hashes and never shows.
//! - **The name as written**, from the appointment updated last (`uat`) that
//!   carries one; never split into given and family names, which one string
//!   cannot give reliably.
//! - **The numbers and the addresses**: the distinct ones seen (a number by
//!   its digits, an address whatever its case), the latest first.
//! - A block (`cu = {d: true}`), a client without an id or without a name,
//!   gives nobody.
//! - **Within Sioul's bounds** ([`Contact::bounded`]): a long name cut, a
//!   number or an address too long left out, eight of each at most, and the
//!   most recently seen clients when there are more than a set holds.

use crate::merge::Appointment;
use sioul_extension::{Contact, ContactSet, MOST_PEOPLE};
use std::collections::{BTreeMap, BTreeSet};

/// The set's id, stable from one refresh to the next: the person's choice of
/// address book is kept by it.
pub const SET_ID: &str = "clients";
/// The set's name, the same word in English and in French.
pub const SET_NAME: &str = "Clients";

/// How recent an appointment is: its `uat`, then its start, then its id (one
/// without `uat` the oldest).
fn rank(appointment: &Appointment) -> (i64, i64, &str) {
    (appointment.updated.unwrap_or(i64::MIN), appointment.start, appointment.id.as_str())
}

/// A number as compared: its digits and its `+` (spaces, dots and dashes
/// aside), or the text itself when it holds none.
fn number_key(number: &str) -> String {
    let key: String = number.chars().filter(|c| c.is_ascii_digit() || *c == '+').collect();
    if key.is_empty() { number.trim().to_string() } else { key }
}

/// The values of `field` across a client's appointments, latest first, each
/// once by its key.
fn distinct<'a>(appointments: &[&'a Appointment], field: impl Fn(&'a Appointment) -> Option<&'a str>, key: impl Fn(&str) -> String) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    let mut out: Vec<String> = Vec::new();
    for value in appointments.iter().copied().filter_map(&field) {
        let key = key(value);
        if !keys.contains(&key) {
            keys.push(key);
            out.push(value.to_string());
        }
    }
    out
}

/// The set, from the appointments read, and what to say in the log (counts
/// only, never a name).
pub fn set(appointments: &[Appointment]) -> (ContactSet, Vec<String>) {
    let mut by_client: BTreeMap<&str, Vec<&Appointment>> = BTreeMap::new();
    for appointment in appointments {
        if let Some(id) = appointment.client_id.as_deref() {
            by_client.entry(id).or_default().push(appointment);
        }
    }
    let mut log = Vec::new();
    let mut people: Vec<(Contact, (i64, i64, &str))> = Vec::new();
    let mut ids: BTreeSet<String> = BTreeSet::new();
    let mut nameless = 0usize;
    for (id, mut seen) in by_client {
        seen.sort_by(|a, b| rank(b).cmp(&rank(a)));
        let Some(name) = seen.iter().find_map(|a| a.client_name.as_deref()) else {
            nameless += 1;
            continue;
        };
        let person = Contact {
            id: id.to_string(),
            name: name.to_string(),
            phones: distinct(&seen, |a| a.phone.as_deref(), number_key),
            emails: distinct(&seen, |a| a.email.as_deref(), str::to_lowercase),
            ..Contact::default()
        };
        match person.bounded() {
            // Two ids that are one once cleaned would be a person handed twice: the whole reply refused.
            Some(person) if !ids.insert(person.id.clone()) => nameless += 1,
            Some(person) => people.push((person, rank(seen[0]))),
            None => nameless += 1,
        }
    }
    if nameless > 0 {
        log.push(format!("{nameless} client(s) without a name or a usable id left out"));
    }
    if people.len() > MOST_PEOPLE {
        log.push(format!("{} client(s) past the {MOST_PEOPLE} a set holds left out, the least recently seen", people.len() - MOST_PEOPLE));
        people.sort_by(|a, b| b.1.cmp(&a.1));
        people.truncate(MOST_PEOPLE);
        people.sort_by(|a, b| a.0.id.cmp(&b.0.id));
    }
    log.push(format!("{} client(s) handed back as contacts", people.len()));
    (ContactSet { id: SET_ID.into(), name: SET_NAME.into(), people: people.into_iter().map(|(person, _)| person).collect() }, log)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(id: &str, client: &str, updated: i64) -> Appointment {
        Appointment { id: id.into(), key: id.into(), client_id: Some(client.into()), client_name: Some("Marie Exemple".into()), updated: Some(updated), ..Appointment::default() }
    }

    #[test]
    fn a_set_holds_at_most_what_sioul_takes_the_most_recently_seen() {
        let appointments: Vec<Appointment> = (0..=MOST_PEOPLE as i64).map(|i| seen(&format!("-V{i:019}"), &format!("Cinvented{i:019}"), 1_795_000_000_000 + i)).collect();
        let (set, log) = set(&appointments);
        assert_eq!(set.people.len(), MOST_PEOPLE);
        assert!(!set.people.iter().any(|p| p.id == format!("Cinvented{:019}", 0)), "the least recently seen is the one left out");
        assert!(set.people.windows(2).all(|w| w[0].id < w[1].id), "in the order of their ids");
        assert!(log.iter().any(|l| l.starts_with("1 client(s) past")), "{log:?}");
    }

    #[test]
    fn numbers_are_compared_by_their_digits() {
        assert_eq!(number_key("+33 1 23-45.67 89"), "+33123456789");
        assert_eq!(number_key(" n/a "), "n/a");
    }

    #[test]
    fn two_ids_that_clean_to_one_give_one_person() {
        // Sioul would refuse the whole reply for a person handed twice.
        let (set, log) = set(&[seen("-V1", "Cinvented0000000000000000061", 2), seen("-V2", "Cinvented0000000000000000061\u{200E}", 1)]);
        assert_eq!(set.people.len(), 1);
        assert_eq!(set.people[0].id, "Cinvented0000000000000000061");
        assert!(log.iter().any(|l| l.starts_with("1 client(s) without a name or a usable id")), "{log:?}");
    }
}
