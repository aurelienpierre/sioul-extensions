// SPDX-License-Identifier: GPL-3.0-or-later

//! The extension against recorded answers only, never a live server: the
//! answers in `tests/recorded/` are written from the request/response shapes
//! validated in the publisher's design notes (`api.md`, `extension-spec.md`,
//! `merge-design.md`), with an invented business (`-Binvented0000000000`), an
//! invented calendar ("Institut Exemple", `-Cinvented0000000000`), invented
//! clients ("Marie Exemple", `marie@example.org`, `+33123456789`; "Paul
//! Exemple"), invented appointment and service ids, short fake tokens and a
//! fake feed key. Nothing here is real but the Firebase API key and the
//! Planity hostnames, both public. The stand-in host answers only the hosts
//! the repository's manifest names for this extension, as Sioul does.
//!
//! The recorded feed (`feed.ics`) and documents (`vevents.json`) hold, by id
//! (`-Vinvented00000000NN`):
//! - 01: one service; the feed's record fresher than Firestore's (which holds 14:00, not 14:30);
//! - 02, 03, 04: one booking of three services, as the live feed writes it: three VEVENTs
//!   (one per service, each its own UID and times, each listing the three ids), one `sq` 02;
//!   Firestore's 03 says 35 minutes where the feed's event spans 30;
//! - 05: cancelled in the feed, fresher than Firestore's live copy;
//! - 06: moved to the next day and cancelled in Firestore, fresher than the feed;
//! - 07: `DAILY;INTERVAL=7;UNTIL=…` with one `EXDATE`, the feed alone;
//! - 08: `WEEKLY;INTERVAL=2;BYDAY=TU,TH` with no end and no client, the feed alone;
//! - 09: a block with no client nor service, the feed alone;
//! - 10: past the read window, the feed alone;
//! - 11: older than the feed's window, Firestore alone, its service named by the feed's map;
//! - 12: an all-day block, Firestore alone;
//! - 13: a cancelled (`dat`) booking, Firestore alone;
//! - 14, 15: a `DAILY;INTERVAL=7` and a `WEEKLY;INTERVAL=2` series, each cancelled as a whole.
//!
//! Their clients: "Marie Exemple" (`clientinvented0000000000001a`) in 01, 02, 03, 04, 06 and 11;
//! "Paul Exemple" (`clientinvented0000000000002b`) in 05 and 13; the block 12 has none. The
//! contacts' tests add documents of their own (31 onwards, clients `Cinvented…`).

use crate::Planity;
use serde_json::{Value, json};
use sioul_extension::standin::StandIn;
use sioul_extension::{Answer, Request, run};
use std::collections::BTreeSet;

/// 2026-11-22T10:00:00Z.
const NOW: i64 = 1_795_341_600;
const BUSINESS: &str = "-Binvented0000000000";
const REFRESH_B: &str = "RT-custom-B-invented-000000000000";
const PASSWORD: &str = "motdepasse-invented";
const KEY: &str = "KEYinvented000000000000000000000";
const MARIE: &str = "Marie Exemple";

/// The calendar's invented export link.
fn link() -> String {
    format!("https://product.api.euwest1.prod.planityapp.com/getCalendarIcalEvents?calendarId=-Cinvented0000000000&businessId={BUSINESS}&key={KEY}")
}

fn v(n: u32) -> String {
    format!("-Vinvented{n:010}")
}

fn recorded(name: &str) -> String {
    std::fs::read_to_string(format!("{}/tests/recorded/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// The repository's manifest entry for the planity extension.
fn manifest_entry() -> String {
    let manifest = std::fs::read_to_string(format!("{}/../manifest.toml", env!("CARGO_MANIFEST_DIR"))).unwrap();
    manifest.split("id = \"planity\"").nth(1).expect("planity in the manifest").split("[[extension]]").next().unwrap().to_string()
}

/// A list of strings of the manifest entry (`hosts`, `credentials`).
fn manifest_list(name: &str) -> Vec<String> {
    let entry = manifest_entry();
    let list = entry.split(&format!("{name} = [")).nth(1).unwrap().split(']').next().unwrap();
    list.split(',').map(|h| h.trim().trim_matches('"').to_string()).filter(|h| !h.is_empty()).collect()
}

/// What the answers say, as each test sets it.
#[derive(Clone)]
struct Scenario {
    /// The password sign-in answers a bad-credentials error.
    bad_password: bool,
    /// The token refresh answers an expired-refresh error.
    expired_refresh: bool,
    /// The token refresh hands a new refresh token.
    rotated: Option<String>,
    /// The `vevents` documents the salon holds (the inner `{name, fields}`),
    /// unsorted; the Firestore stand-in sorts, filters and pages them.
    documents: Vec<Value>,
    /// A status Firestore answers in place of the documents.
    firestore_status: Option<u16>,
    /// The feed's status and body.
    feed: (u16, String),
}

impl Default for Scenario {
    fn default() -> Self {
        Self { bad_password: false, expired_refresh: false, rotated: None, documents: recorded_documents(), firestore_status: None, feed: (200, recorded("feed.ics")) }
    }
}

/// The recorded documents: the elements of `vevents.json` that carry a document.
fn recorded_documents() -> Vec<Value> {
    let page: Value = serde_json::from_str(&recorded("vevents.json")).unwrap();
    page.as_array().unwrap().iter().filter_map(|element| element.get("document").cloned()).collect()
}

fn doc_s(document: &Value) -> String {
    document["fields"]["s"]["stringValue"].as_str().unwrap_or("").to_string()
}

fn doc_name(document: &Value) -> String {
    document["name"].as_str().unwrap_or("").to_string()
}

/// The Firestore `:runQuery` stand-in: it sorts the salon's documents by
/// `s` then name, keeps those with `s >= from` (the only server-side filter),
/// starts strictly after the cursor when one is given, and answers at most
/// the query's limit — every page led by a read-time marker, as Firestore's.
fn run_query(scenario: &Scenario, body: &str) -> Answer {
    let query: Value = serde_json::from_str(body).unwrap();
    let structured = &query["structuredQuery"];
    assert_eq!(structured["from"][0]["collectionId"], "vevents");
    assert_eq!(structured["orderBy"][0]["field"]["fieldPath"], "s");
    assert_eq!(structured["orderBy"][1]["field"]["fieldPath"], "__name__");
    let from = structured["where"]["fieldFilter"]["value"]["stringValue"].as_str().unwrap().to_string();
    assert_eq!(structured["where"]["fieldFilter"]["op"], "GREATER_THAN_OR_EQUAL");
    let limit = structured["limit"].as_i64().unwrap() as usize;
    let cursor = structured.get("startAt").map(|at| {
        assert_eq!(at["before"], false);
        (at["values"][0]["stringValue"].as_str().unwrap().to_string(), at["values"][1]["referenceValue"].as_str().unwrap().to_string())
    });

    let mut sorted = scenario.documents.clone();
    sorted.sort_by(|a, b| (doc_s(a), doc_name(a)).cmp(&(doc_s(b), doc_name(b))));
    let page: Vec<Value> = sorted
        .into_iter()
        .filter(|document| doc_s(document) >= from)
        .filter(|document| match &cursor {
            Some((s, name)) => (doc_s(document), doc_name(document)) > (s.clone(), name.clone()),
            None => true,
        })
        .take(limit)
        .map(|document| json!({ "document": document, "readTime": "2026-11-22T10:00:00.000000Z" }))
        .collect();
    // A read-time-only marker leads the page, as a Firestore stream does.
    let mut elements = vec![json!({ "readTime": "2026-11-22T10:00:00.000000Z" })];
    elements.extend(page);
    Answer::new(200, Value::Array(elements).to_string())
}

fn web(scenario: Scenario) -> impl FnMut(&Request) -> Answer {
    move |request: &Request| {
        let url = request.url.as_str();
        let body = request.body.clone().unwrap_or_default();
        if url.contains("getCalendarIcalEvents") {
            assert_eq!(request.method, "GET", "the feed is read, never written");
            assert_eq!(url, link(), "the feed is read at the link kept");
            return Answer::new(scenario.feed.0, scenario.feed.1.clone());
        }
        assert!(url.contains(&format!("key={}", crate::API_KEY)) || url.contains("firestore.googleapis.com") || url.contains("getProAuthToken"), "the API key rides the identitytoolkit and securetoken calls: {url}");
        if url.contains("identitytoolkit.googleapis.com") && url.contains("signInWithPassword") {
            assert!(body.contains("\"returnSecureToken\":true") && body.contains("CLIENT_TYPE_WEB"), "{body}");
            return if scenario.bad_password { Answer::new(400, recorded("signin-password-bad.json")) } else { Answer::new(200, recorded("signin-password.json")) };
        }
        if url.contains("getProAuthToken") {
            assert!(body.contains("\"source\":\"web\"") && body.contains("\"isBusinessSharded\":true"), "{body}");
            return Answer::new(200, recorded("pro-token.json"));
        }
        if url.contains("identitytoolkit.googleapis.com") && url.contains("signInWithCustomToken") {
            assert!(body.contains("\"returnSecureToken\":true"), "{body}");
            return Answer::new(200, recorded("signin-custom.json"));
        }
        if url.contains("securetoken.googleapis.com") {
            assert!(body.contains("grant_type=refresh_token") && body.contains("refresh_token="), "{body}");
            if scenario.expired_refresh {
                return Answer::new(400, recorded("securetoken-expired.json"));
            }
            let mut answer: Value = serde_json::from_str(&recorded("securetoken.json")).unwrap();
            if let Some(rotated) = &scenario.rotated {
                answer["refresh_token"] = json!(rotated);
            }
            return Answer::new(200, answer.to_string());
        }
        if url.contains("firestore.googleapis.com") && url.ends_with(":runQuery") {
            assert!(request.headers.iter().any(|(n, v)| n.eq_ignore_ascii_case("Authorization") && v.starts_with("Bearer ") && v.len() > 7), "the id token rides as a bearer: {:?}", request.headers);
            assert!(url.contains(&format!("/businesses/{BUSINESS}:runQuery")), "{url}");
            if let Some(status) = scenario.firestore_status {
                return Answer::new(status, "{}");
            }
            return run_query(&scenario, &body);
        }
        panic!("not recorded: {url}");
    }
}

fn stand_in(scenario: Scenario) -> StandIn {
    let mut host = StandIn::new(NOW);
    host.hosts = manifest_list("hosts");
    host.web = Box::new(web(scenario));
    host
}

fn call(host: &mut StandIn, text: Value) -> Value {
    serde_json::from_str(&run::<Planity>(host, text.to_string().as_bytes())).unwrap()
}

fn refresh(host: &mut StandIn, lang: &str) -> Value {
    call(host, json!({ "interface": 1, "call": "refresh", "lang": lang }))
}

/// A sign-in kept, as `sign-in` keeps it.
fn signed_in(host: &mut StandIn) {
    host.secret = Some(json!({ "r": REFRESH_B, "b": BUSINESS, "l": link() }).to_string());
}

fn sign_in(host: &mut StandIn, address: &str) -> Value {
    host.sign_in_with::<Planity>("en", None, &[("email", "marie@example.org"), ("password", PASSWORD), ("web-address", address)])
}

/// The agenda's calendar text with its folded lines joined.
fn unfolded(agenda: &Value) -> String {
    agenda["calendar"].as_str().unwrap().replace("\r\n ", "")
}

/// Each VEVENT of a calendar, by its UID.
fn vevents(calendar: &str) -> Vec<(String, String)> {
    calendar.split("BEGIN:VEVENT\r\n").skip(1).map(|event| (event.split("\r\n").next().unwrap().trim_start_matches("UID:").to_string(), event.to_string())).collect()
}

/// One VEVENT of a calendar, by its UID.
fn vevent(calendar: &str, uid: &str) -> String {
    vevents(calendar).into_iter().find(|(u, _)| u == uid).unwrap_or_else(|| panic!("{uid} in {calendar}")).1
}

fn has(event: &str, line: &str) -> bool {
    event.contains(&format!("{line}\r\n"))
}

// ---------------------------------------------------------------- the sign-in

#[test]
fn the_sign_in_keeps_the_refresh_token_the_business_and_the_link_never_the_password() {
    let mut host = stand_in(Scenario::default());
    let reply = sign_in(&mut host, &format!("  {}\n", link()));
    assert_eq!(reply, json!({ "signed_in": true }), "{:?}", host.log);
    let secret = host.secret.clone().unwrap();
    let kept: Value = serde_json::from_str(&secret).unwrap();
    assert_eq!((kept["r"].as_str(), kept["b"].as_str(), kept["l"].as_str()), (Some(REFRESH_B), Some(BUSINESS), Some(link().as_str())));
    assert_eq!(kept.as_object().unwrap().len(), 3, "only r, b and l: {kept}");
    assert!(secret.len() <= 2400, "the keyring keeps 2400 bytes at most");
    assert!(!secret.contains(PASSWORD) && !secret.contains("marie@example.org"), "never the password nor the email");
    // The password, every token, the link and its key are hidden and absent from the log.
    for hidden in [PASSWORD, "marie@example.org", "AT-password-A-invented-000000000000", "RT-password-A-invented-000000", "CT-invented-custom-token-000000000000", REFRESH_B, KEY, link().as_str()] {
        assert!(host.hidden.iter().any(|h| h == hidden), "{hidden} not hidden");
        assert!(!host.log_shows(hidden), "{hidden} in the log {:?}", host.log);
    }
    // The dance is three calls, in order; the feed is not read at sign-in.
    let urls: Vec<&str> = host.asked.iter().map(|r| r.url.as_str()).collect();
    assert_eq!(urls.len(), 3, "{urls:?}");
    assert!(urls[0].contains("signInWithPassword") && urls[1].contains("getProAuthToken") && urls[2].contains("signInWithCustomToken"), "{urls:?}");
}

#[test]
fn a_bad_password_or_a_missing_field_is_a_sign_in_problem() {
    let mut host = stand_in(Scenario { bad_password: true, ..Scenario::default() });
    let reply = host.sign_in_with::<Planity>("fr", None, &[("email", "marie@example.org"), ("password", "mauvais-mot-de-passe"), ("web-address", &link())]);
    assert_eq!(reply["problem"]["kind"], "sign-in", "{:?}", host.log);
    assert_eq!(host.secret, None, "nothing kept on a bad password");
    assert!(!host.asked.iter().any(|r| r.url.contains("getProAuthToken")), "it stops at the password step");
    // No credentials at all, or one of the three missing or blank: a sign-in problem, with no call.
    let mut host = stand_in(Scenario::default());
    let reply = call(&mut host, json!({ "interface": 1, "call": "sign-in", "lang": "en", "choice": null }));
    assert_eq!(reply["problem"]["kind"], "sign-in");
    for fields in [
        vec![("email", "marie@example.org"), ("password", PASSWORD)],
        vec![("email", "marie@example.org"), ("web-address", "https://x")],
        vec![("email", "marie@example.org"), ("password", PASSWORD), ("web-address", "   ")],
    ] {
        let reply = host.sign_in_with::<Planity>("en", None, &fields);
        assert_eq!(reply["problem"]["kind"], "sign-in", "{fields:?}");
    }
    assert!(host.asked.is_empty(), "{:?}", host.asked);
    assert_eq!(host.secret, None);
}

#[test]
fn the_link_is_checked_before_anything_and_against_the_business() {
    // A link that is not Planity's export, or that lacks its key, is refused before any call.
    for wrong in [
        link().replacen("https://", "http://", 1),
        link().replacen("product.api.euwest1.prod.planityapp.com", "calendar.example.org", 1),
        link().replacen("/getCalendarIcalEvents", "/getProAuthToken", 1),
        link().replacen(&format!("&key={KEY}"), "", 1),
    ] {
        let mut host = stand_in(Scenario::default());
        let reply = sign_in(&mut host, &wrong);
        assert_eq!(reply["problem"]["kind"], "sign-in", "{wrong}");
        assert!(host.asked.is_empty(), "no call with a wrong link: {wrong}");
        assert_eq!(host.secret, None);
        assert!(!host.log_shows(KEY), "{:?}", host.log);
    }
    // Another business's calendar: the dance runs, the business differs, nothing is kept.
    let mut host = stand_in(Scenario::default());
    let reply = sign_in(&mut host, &link().replacen(BUSINESS, "-Binvented9999999999", 1));
    assert_eq!(reply["problem"]["kind"], "sign-in", "{:?}", host.log);
    assert!(reply["problem"]["detail"].as_str().unwrap().contains("another business"), "{reply}");
    assert_eq!(host.secret, None);
    // A sign-in longer than the keyring keeps is a sign-in problem, nothing kept.
    let mut host = stand_in(Scenario::default());
    let reply = sign_in(&mut host, &link().replacen(KEY, &"K".repeat(2400), 1));
    assert_eq!(reply["problem"]["kind"], "sign-in", "{:?}", host.log);
    assert_eq!(host.secret, None);
}

// ---------------------------------------------------------------- the merge

#[test]
fn a_refresh_merges_both_sources_into_one_busy_agenda() {
    let mut host = stand_in(Scenario::default());
    signed_in(&mut host);
    let reply = refresh(&mut host, "en");
    assert_eq!(reply["problem"], Value::Null, "{:?}", host.log);
    let agendas = reply["agendas"].as_array().unwrap();
    assert_eq!(agendas.len(), 1, "one calendar, one agenda");
    let agenda = &agendas[0];
    assert_eq!(agenda["id"], "cinvented0000000000", "a stable id from the calendar id, the same in both sources");
    assert_eq!(agenda["name"], "Institut Exemple", "the feed's calendar name");
    assert_eq!(agenda["busy"], true, "the salon's own appointments are busy time");
    assert_eq!(agenda["period"], json!({ "from": "2026-09-23T10:00:00Z", "until": "2027-03-22T10:00:00Z" }), "both answered: the window read");
    let calendar = unfolded(agenda);
    assert!(calendar.contains("X-WR-CALNAME:Institut Exemple\r\n"));
    // 5 one-off bookings of the feed in the window, 12 + 20 occurrences of its two series, 3 from Firestore alone.
    let uids: Vec<String> = vevents(&calendar).into_iter().map(|(uid, _)| uid).collect();
    assert_eq!(uids.len(), 40, "{uids:?}");
    let mut unique = uids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), uids.len(), "one UID per event");
    assert!(!uids.contains(&v(10)), "a booking past the window is left out");
    // Both sources were read, the token minted first.
    let urls: Vec<&str> = host.asked.iter().map(|r| r.url.as_str()).collect();
    assert!(urls[0].contains("securetoken.googleapis.com") && urls.iter().any(|u| u.ends_with(":runQuery")) && urls.iter().any(|u| u.contains("getCalendarIcalEvents")), "{urls:?}");
    // Nothing secret in the log; no TRANSP written (the door decides).
    for hidden in [REFRESH_B, KEY] {
        assert!(host.hidden.iter().any(|h| h == hidden) && !host.log_shows(hidden), "{hidden}: {:?}", host.log);
    }
    assert!(!calendar.contains("TRANSP:"));
    // The same answers make the same bytes.
    let again = refresh(&mut host, "en");
    assert_eq!(again["agendas"][0]["calendar"], reply["agendas"][0]["calendar"]);
}

#[test]
fn a_booking_of_three_services_is_one_event_with_each_services_minutes() {
    let mut host = stand_in(Scenario::default());
    signed_in(&mut host);
    let calendar = unfolded(&refresh(&mut host, "en")["agendas"][0]);
    // The feed writes one VEVENT per service (02, 03, 04); the event is the booking's (its sq 02), once.
    let booking = vevent(&calendar, &v(2));
    assert!(!calendar.contains(&format!("UID:{}\r\n", v(3))) && !calendar.contains(&format!("UID:{}\r\n", v(4))), "the services are one booking, not repeated");
    assert!(has(&booking, &format!("SUMMARY:{MARIE} — Couleur\\, Coupe\\, Brushing")), "{booking}");
    // 10:00–12:00 Paris in winter: the first service's start to the last's end.
    assert!(has(&booking, "DTSTART:20261202T090000Z") && has(&booking, "DTEND:20261202T110000Z"), "{booking}");
    // Joined, each service's minutes are its document's `d` (Coupe: 35, where the feed spans 30);
    // the feed's prices added, prepaid, the phone.
    assert!(booking.contains("DESCRIPTION:Couleur (60 min)\\nCoupe (35 min)\\nBrushing (30 min)\\n120\\,00\u{a0}€\\nPrepaid\\n+33123456789\r\n"), "{booking}");
    assert!(has(&booking, "STATUS:CONFIRMED"));
    // The feed alone: each service's minutes are its own VEVENT's span.
    let mut host = stand_in(Scenario::default());
    host.secret = Some(json!({ "b": BUSINESS, "l": link() }).to_string());
    let calendar = unfolded(&refresh(&mut host, "en")["agendas"][0]);
    let booking = vevent(&calendar, &v(2));
    assert!(booking.contains("DESCRIPTION:Couleur (60 min)\\nCoupe (30 min)\\nBrushing (30 min)\\n120\\,00\u{a0}€\\nPrepaid\r\n"), "{booking}");
    assert!(has(&booking, "DTSTART:20261202T090000Z") && has(&booking, "DTEND:20261202T110000Z"), "{booking}");
}

#[test]
fn the_fresher_record_gives_the_times_and_the_cancelled_state_both_ways() {
    let mut host = stand_in(Scenario::default());
    signed_in(&mut host);
    let calendar = unfolded(&refresh(&mut host, "en")["agendas"][0]);
    // 01: the feed is fresher: its 14:30, not Firestore's 14:00; the client and note from Firestore.
    let one = vevent(&calendar, &v(1));
    assert!(has(&one, "DTSTART:20261125T133000Z") && has(&one, "DTEND:20261125T140000Z"), "{one}");
    assert!(has(&one, &format!("SUMMARY:{MARIE} — Coupe")), "{one}");
    assert!(one.contains("DESCRIPTION:Coupe (30 min)\\n45\\,00\u{a0}€\\nBooked online\\n+33123456789\\nCheveux longs\r\n"), "{one}");
    assert!(has(&one, "STATUS:CONFIRMED"));
    // 05: cancelled in the feed, fresher than Firestore's live copy: cancelled.
    let five = vevent(&calendar, &v(5));
    assert!(has(&five, "STATUS:CANCELLED") && has(&five, "SUMMARY:Paul Exemple — Soin visage"), "{five}");
    // 06: Firestore is fresher: moved to the 10th at 16:00 Paris and cancelled; still named by the feed.
    let six = vevent(&calendar, &v(6));
    assert!(has(&six, "DTSTART:20261210T150000Z") && has(&six, "DTEND:20261210T154500Z"), "{six}");
    assert!(has(&six, "STATUS:CANCELLED") && has(&six, &format!("SUMMARY:{MARIE} — Manucure")), "{six}");
}

#[test]
fn bookings_firestore_alone_holds_are_named_by_the_feeds_map() {
    let mut host = stand_in(Scenario::default());
    signed_in(&mut host);
    let calendar = unfolded(&refresh(&mut host, "en")["agendas"][0]);
    // 11: older than the feed's window; its service id named through the feed.
    let older = vevent(&calendar, &v(11));
    assert!(has(&older, &format!("SUMMARY:{MARIE} — Coupe")) && has(&older, "DTSTART:20261015T090000Z"), "{older}");
    assert!(older.contains("DESCRIPTION:Coupe (30 min)\\n45\\,00\u{a0}€\\n+33123456789\\nPremière visite\r\n"), "{older}");
    // 13: cancelled (`dat`), Firestore alone.
    let cancelled = vevent(&calendar, &v(13));
    assert!(has(&cancelled, "STATUS:CANCELLED") && has(&cancelled, "SUMMARY:Paul Exemple — Soin visage"), "{cancelled}");
    // 12: the all-day block stays all-day, "Unavailable", its note shown; « Indisponible » in French.
    let block = vevent(&calendar, &v(12));
    assert!(has(&block, "DTSTART;VALUE=DATE:20261224") && has(&block, "DTEND;VALUE=DATE:20261225") && has(&block, "SUMMARY:Unavailable"), "{block}");
    assert!(block.contains("Congés"), "{block}");
    let french = unfolded(&refresh(&mut host, "fr")["agendas"][0]);
    assert!(has(&vevent(&french, &v(12)), "SUMMARY:Indisponible"));
    assert!(vevent(&french, &v(2)).contains("Prépayé") && vevent(&french, &v(1)).contains("Réservé en ligne"), "{french}");
    // 09: the feed's block, no client nor service: titled by its summary less the calendar's name.
    let pause = vevent(&calendar, &v(9));
    assert!(has(&pause, "SUMMARY:Pause déjeuner") && !pause.contains("Institut Exemple"), "{pause}");
}

#[test]
fn series_expand_over_the_window_in_wall_clock_time() {
    let mut host = stand_in(Scenario::default());
    signed_in(&mut host);
    let calendar = unfolded(&refresh(&mut host, "en")["agendas"][0]);
    let of = |key: &str| -> Vec<(String, String)> { vevents(&calendar).into_iter().filter(|(uid, _)| uid.starts_with(&format!("{key}-"))).collect() };
    // 07: every seven days from 7 October to 30 December, less 11 November: twelve.
    let massage = of(&v(7));
    let dates: Vec<&str> = massage.iter().map(|(uid, _)| &uid[uid.len() - 8..]).collect();
    assert_eq!(dates.len(), 12, "{dates:?}");
    assert_eq!((dates[0], dates[11]), ("20261007", "20261230"));
    assert!(!dates.contains(&"20261111"), "the EXDATE is taken out");
    // 10:00 Paris: 08:00 UTC in summer time, 09:00 UTC in winter time; an hour long; the client from the map.
    let first = vevent(&calendar, &format!("{}-20261007", v(7)));
    assert!(has(&first, "DTSTART:20261007T080000Z") && has(&first, "DTEND:20261007T090000Z"), "{first}");
    let winter = vevent(&calendar, &format!("{}-20261202", v(7)));
    assert!(has(&winter, "DTSTART:20261202T090000Z") && has(&winter, &format!("SUMMARY:{MARIE} — Massage")), "{winter}");
    assert!(winter.contains("Massage (60 min)") && winter.contains("70\\,00"), "{winter}");
    // 08: every other week on Tuesday and Thursday, no end: twenty up to the window's end; no client.
    let coiffage = of(&v(8));
    let dates: Vec<&str> = coiffage.iter().map(|(uid, _)| &uid[uid.len() - 8..]).collect();
    assert_eq!(dates.len(), 20, "{dates:?}");
    assert_eq!(&dates[..4], &["20261103", "20261105", "20261117", "20261119"]);
    assert_eq!(dates[19], "20270311");
    assert!(coiffage.iter().all(|(_, event)| has(event, "SUMMARY:Coiffage")), "{coiffage:?}");
}

#[test]
fn a_series_cancelled_as_a_whole_hands_back_no_occurrence() {
    let count = |calendar: &str, key: &str| vevents(calendar).into_iter().filter(|(uid, _)| uid.starts_with(&format!("{key}-"))).count();
    let mut host = stand_in(Scenario::default());
    signed_in(&mut host);
    let calendar = unfolded(&refresh(&mut host, "en")["agendas"][0]);
    for key in [v(14), v(15)] {
        assert!(!calendar.contains(&format!("UID:{key}")), "{key}: no cancelled occurrence per date");
    }
    assert!(host.log.iter().any(|l| l.contains("2 series cancelled as a whole left out")), "{:?}", host.log);
    // The series beside them still expand; a cancelled one-off booking is still said cancelled.
    assert_eq!((count(&calendar, &v(7)), count(&calendar, &v(8))), (12, 20));
    assert!(has(&vevent(&calendar, &v(5)), "STATUS:CANCELLED"));
    // The feed alone leaves them out too.
    let mut host = stand_in(Scenario::default());
    host.secret = Some(json!({ "b": BUSINESS, "l": link() }).to_string());
    let calendar = unfolded(&refresh(&mut host, "en")["agendas"][0]);
    assert_eq!((count(&calendar, &v(14)), count(&calendar, &v(15))), (0, 0));
    // Not cancelled, the same series comes: every Thursday from 8 October to the window's end.
    let feed = recorded("feed.ics").replacen("RRULE:FREQ=DAILY;INTERVAL=7\r\nSTATUS:CANCELLED\r\n", "RRULE:FREQ=DAILY;INTERVAL=7\r\n", 1);
    let mut host = stand_in(Scenario { feed: (200, feed), ..Scenario::default() });
    signed_in(&mut host);
    let calendar = unfolded(&refresh(&mut host, "en")["agendas"][0]);
    assert_eq!(count(&calendar, &v(14)), 24, "{calendar}");
    assert!(has(&vevent(&calendar, &format!("{}-20261008", v(14))), "STATUS:CONFIRMED"));
    assert_eq!(count(&calendar, &v(15)), 0);
}

#[test]
fn a_rule_that_does_not_read_keeps_its_first_occurrence_and_says_so() {
    let feed = recorded("feed.ics").replace("RRULE:FREQ=WEEKLY;INTERVAL=2;BYDAY=TU,TH", "RRULE:FREQ=MONTHLY;BYMONTHDAY=3");
    let mut host = stand_in(Scenario { feed: (200, feed), ..Scenario::default() });
    signed_in(&mut host);
    let reply = refresh(&mut host, "en");
    let calendar = unfolded(&reply["agendas"][0]);
    let coiffage: Vec<String> = vevents(&calendar).into_iter().map(|(uid, _)| uid).filter(|uid| uid.starts_with(&v(8))).collect();
    assert_eq!(coiffage, vec![format!("{}-20261103", v(8))]);
    assert!(host.log.iter().any(|l| l.contains("does not read") && l.contains("FREQ=MONTHLY")), "{:?}", host.log);
    assert!(reply["agendas"][0]["period"].is_object(), "both sources answered");
}

#[test]
fn a_booking_keeps_its_uid_whichever_source_holds_it() {
    let both = {
        let mut host = stand_in(Scenario::default());
        signed_in(&mut host);
        unfolded(&refresh(&mut host, "en")["agendas"][0])
    };
    let firestore_alone = {
        let mut host = stand_in(Scenario { feed: (503, String::new()), ..Scenario::default() });
        signed_in(&mut host);
        unfolded(&refresh(&mut host, "en")["agendas"][0])
    };
    let feed_alone = {
        let mut host = stand_in(Scenario::default());
        host.secret = Some(json!({ "b": BUSINESS, "l": link() }).to_string());
        unfolded(&refresh(&mut host, "en")["agendas"][0])
    };
    for uid in [v(1), v(2), v(5)] {
        let (a, b, c) = (vevent(&both, &uid), vevent(&firestore_alone, &uid), vevent(&feed_alone, &uid));
        assert!(a.starts_with(&format!("UID:{uid}\r\n")) && b.starts_with(&format!("UID:{uid}\r\n")) && c.starts_with(&format!("UID:{uid}\r\n")));
    }
    // From Firestore alone, the three services' documents are still one booking, keyed by their sq.
    assert!(!firestore_alone.contains(&format!("UID:{}\r\n", v(3))), "{firestore_alone}");
    let booking = vevent(&firestore_alone, &v(2));
    assert!(has(&booking, "DTSTART:20261202T090000Z") && has(&booking, "DTEND:20261202T110000Z") && has(&booking, &format!("SUMMARY:{MARIE}")), "{booking}");
}

// ---------------------------------------------------------------- degraded, never destructive

#[test]
fn a_feed_that_fails_leaves_firestores_bookings_and_removes_nothing() {
    for (status, body) in [(500, String::new()), (200, "<html>not a calendar</html>".to_string())] {
        let mut host = stand_in(Scenario { feed: (status, body.clone()), ..Scenario::default() });
        signed_in(&mut host);
        let reply = refresh(&mut host, "en");
        assert_eq!(reply["problem"], Value::Null, "only logged: {status} {body}");
        let agenda = &reply["agendas"][0];
        assert_eq!(agenda.get("period"), None, "no period: nothing removed ({status})");
        assert_eq!(agenda["id"], "cinvented0000000000");
        assert_eq!(agenda["name"], "Planity", "no feed, no calendar name");
        let calendar = unfolded(agenda);
        // The documents' bookings: 01, 02 (three documents), 05, 06, 11, 12, 13.
        assert_eq!(vevents(&calendar).len(), 7, "{calendar}");
        assert!(has(&vevent(&calendar, &v(1)), "DTSTART:20261125T130000Z"), "Firestore's own time when it is alone");
        assert!(host.log.iter().any(|l| l.contains("the feed not read")), "{:?}", host.log);
    }
    // A feed whose key is refused asks to sign in again, Firestore's bookings still there.
    let mut host = stand_in(Scenario { feed: (403, String::new()), ..Scenario::default() });
    signed_in(&mut host);
    let reply = refresh(&mut host, "en");
    assert_eq!(reply["problem"]["kind"], "sign-in");
    assert_eq!(reply["agendas"][0].get("period"), None);
    assert_eq!(vevents(&unfolded(&reply["agendas"][0])).len(), 7);
}

#[test]
fn without_a_firebase_session_the_feed_comes_alone_and_asks_to_sign_in() {
    // No refresh token kept (a secret of the business and the link only).
    let mut host = stand_in(Scenario::default());
    host.secret = Some(json!({ "b": BUSINESS, "l": link() }).to_string());
    let reply = refresh(&mut host, "en");
    assert_eq!(reply["problem"]["kind"], "sign-in", "{:?}", host.log);
    let agenda = &reply["agendas"][0];
    assert_eq!(agenda.get("period"), None, "no period: nothing removed");
    assert_eq!(agenda["name"], "Institut Exemple");
    let calendar = unfolded(agenda);
    // The feed's 5 one-off bookings in the window and 32 occurrences; no client names.
    assert_eq!(vevents(&calendar).len(), 37, "{calendar}");
    assert!(!calendar.contains(MARIE) && !calendar.contains("+33123456789"), "no client without Firestore");
    assert!(has(&vevent(&calendar, &v(1)), "SUMMARY:Coupe"));
    assert!(has(&vevent(&calendar, &v(2)), "SUMMARY:Couleur\\, Coupe\\, Brushing"));
    assert!(host.asked.iter().all(|r| r.url.contains("getCalendarIcalEvents")), "no Firebase call: {:?}", host.asked);
    // A session refused (an expired refresh): the same, and the kept sign-in left as it was.
    let mut host = stand_in(Scenario { expired_refresh: true, ..Scenario::default() });
    signed_in(&mut host);
    let before = host.secret.clone();
    let reply = refresh(&mut host, "en");
    assert_eq!(reply["problem"]["kind"], "sign-in");
    assert_eq!(reply["agendas"][0].get("period"), None);
    assert_eq!(vevents(&unfolded(&reply["agendas"][0])).len(), 37);
    assert_eq!(host.secret, before, "the sign-in kept is left for the person to renew");
    // Firestore refusing the business is said too.
    let mut host = stand_in(Scenario { firestore_status: Some(403), ..Scenario::default() });
    signed_in(&mut host);
    let reply = refresh(&mut host, "en");
    assert_eq!(reply["problem"]["kind"], "refused");
    assert_eq!(reply["agendas"][0].get("period"), None);
    // Firestore unreachable: the feed alone, only logged.
    let mut host = stand_in(Scenario { firestore_status: Some(503), ..Scenario::default() });
    signed_in(&mut host);
    let reply = refresh(&mut host, "en");
    assert_eq!(reply["problem"], Value::Null);
    assert_eq!(reply["agendas"][0].get("period"), None);
    assert_eq!(vevents(&unfolded(&reply["agendas"][0])).len(), 37);
}

#[test]
fn both_failing_is_the_problem_and_an_older_sign_in_asks_for_the_link() {
    // An expired session and a feed that fails: the sign-in to renew.
    let mut host = stand_in(Scenario { expired_refresh: true, feed: (500, String::new()), ..Scenario::default() });
    signed_in(&mut host);
    let reply = refresh(&mut host, "en");
    assert_eq!((reply["problem"]["kind"].as_str(), reply["agendas"].clone()), (Some("sign-in"), json!([])));
    // Both unreachable: Firestore's problem, as before.
    let mut host = stand_in(Scenario { firestore_status: Some(503), feed: (500, String::new()), ..Scenario::default() });
    signed_in(&mut host);
    let reply = refresh(&mut host, "en");
    assert_eq!((reply["problem"]["kind"].as_str(), reply["agendas"].clone()), (Some("unreachable"), json!([])));
    // No sign-in kept at all: a sign-in problem, with no call.
    let mut host = stand_in(Scenario::default());
    let reply = refresh(&mut host, "en");
    assert_eq!(reply["problem"]["kind"], "sign-in");
    assert!(host.asked.is_empty());
    // A sign-in kept by 0.1.0 (no link): Firestore's bookings, nothing removed, and a sign-in to add the link.
    let mut host = stand_in(Scenario::default());
    host.secret = Some(json!({ "r": REFRESH_B, "b": BUSINESS }).to_string());
    let reply = refresh(&mut host, "en");
    assert_eq!(reply["problem"]["kind"], "sign-in");
    assert_eq!(reply["agendas"][0].get("period"), None);
    assert_eq!(vevents(&unfolded(&reply["agendas"][0])).len(), 7);
    assert!(!host.asked.iter().any(|r| r.url.contains("getCalendarIcalEvents")));
}

#[test]
fn a_rotated_refresh_token_is_kept_with_the_link() {
    let mut host = stand_in(Scenario { rotated: Some("RT-rotated-invented-0000000000".into()), ..Scenario::default() });
    signed_in(&mut host);
    let reply = refresh(&mut host, "en");
    assert_eq!(reply["problem"], Value::Null, "{:?}", host.log);
    let kept: Value = serde_json::from_str(host.secret.as_deref().unwrap()).unwrap();
    assert_eq!(kept, json!({ "r": "RT-rotated-invented-0000000000", "b": BUSINESS, "l": link() }));
    assert!(!host.log_shows("RT-rotated-invented-0000000000"));
}

// ---------------------------------------------------------------- the window, the pages, the hosts

#[test]
fn the_window_is_bounded_both_ends() {
    // A document just before the window (server-side) and one just past it (client-side) are both left out.
    let mut documents = recorded_documents();
    documents.push(json!({ "name": format!("projects/planity-production/databases/(default)/documents/businesses/{BUSINESS}/vevents/-Vbefore000000000000"), "fields": { "s": { "stringValue": "2026-08-01 10:00" }, "ca": { "stringValue": "-Cinvented0000000000" } } }));
    documents.push(json!({ "name": format!("projects/planity-production/databases/(default)/documents/businesses/{BUSINESS}/vevents/-Vafter0000000000000"), "fields": { "s": { "stringValue": "2027-04-01 10:00" }, "ca": { "stringValue": "-Cinvented0000000000" } } }));
    let mut host = stand_in(Scenario { documents, ..Scenario::default() });
    signed_in(&mut host);
    let calendar = unfolded(&refresh(&mut host, "en")["agendas"][0]);
    assert!(!calendar.contains("-Vbefore"), "a document before the window is not returned by the server");
    assert!(!calendar.contains("-Vafter"), "a document past `until` is dropped client-side");
    assert!(!calendar.contains(&format!("UID:{}\r\n", v(10))), "a feed booking past the window is dropped too");
    // The series stop at the window's end (22 March 2027).
    assert!(!calendar.contains(&format!("UID:{}-20270323", v(8))));
    // The window's `from` is sixty days back, as a Europe/Paris wall-clock string.
    let query = host.asked.iter().find(|r| r.url.ends_with(":runQuery")).unwrap();
    assert!(query.body.as_deref().unwrap().contains("\"stringValue\":\"2026-09-23 12:00\""), "from = now-60d in Paris: {:?}", query.body);
}

#[test]
fn pagination_reads_every_page() {
    // More documents than one Firestore page: all are read over two pages.
    let documents: Vec<Value> = (0..350)
        .map(|i| json!({
            "name": format!("projects/planity-production/databases/(default)/documents/businesses/{BUSINESS}/vevents/-Vgen{i:016}"),
            "fields": { "s": { "stringValue": format!("2026-11-01 {:02}:{:02}", i / 60, i % 60) }, "d": { "integerValue": "15" }, "ca": { "stringValue": "-Cinvented0000000000" } }
        }))
        .collect();
    let empty_feed = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nX-WR-CALNAME:Institut Exemple\r\nEND:VCALENDAR\r\n".to_string();
    let mut host = stand_in(Scenario { documents, feed: (200, empty_feed), ..Scenario::default() });
    signed_in(&mut host);
    let reply = refresh(&mut host, "en");
    assert_eq!(reply["problem"], Value::Null, "{:?}", host.log);
    let calendar = unfolded(&reply["agendas"][0]);
    assert_eq!(vevents(&calendar).len(), 350, "every document became an event");
    let pages: Vec<&Request> = host.asked.iter().filter(|r| r.url.ends_with(":runQuery")).collect();
    assert_eq!(pages.len(), 2, "two pages were read");
    assert!(pages[1].body.as_deref().unwrap().contains("\"startAt\""), "the second page paged on a cursor: {:?}", pages[1].body);
}

#[test]
fn every_request_goes_to_a_host_the_manifest_names_and_the_manifest_asks_three_fields() {
    let mut host = stand_in(Scenario::default());
    sign_in(&mut host, &link());
    refresh(&mut host, "en");
    let hosts = manifest_list("hosts");
    assert!(host.asked.len() > 4);
    for request in &host.asked {
        let named = sioul_extension::url::https_host(&request.url).unwrap();
        assert!(hosts.contains(&named), "{named} is not in the manifest");
    }
    for named in ["identitytoolkit.googleapis.com", "securetoken.googleapis.com", "firestore.googleapis.com", "product.api.euwest1.prod.planityapp.com"] {
        assert!(hosts.contains(&named.to_string()), "{named} missing from the manifest");
    }
    assert_eq!(manifest_list("credentials"), vec!["email", "password", "web-address"]);
    assert_eq!(manifest_list("brings"), vec!["agenda", "contacts"], "the manifest says it brings what the reply carries");
    let entry = manifest_entry();
    assert!(entry.contains(&format!("version = \"{}\"", env!("CARGO_PKG_VERSION"))), "the manifest names this version: {entry}");
    assert!(entry.contains(&format!("planity-{0}/planity-{0}.wasm", env!("CARGO_PKG_VERSION"))), "{entry}");
}

// ---------------------------------------------------------------- the clients, as contacts

const MARIE_ID: &str = "clientinvented0000000000001a";
const PAUL_ID: &str = "clientinvented0000000000002b";

/// An invented client id, 28 characters as Planity's.
fn client(n: u32) -> String {
    format!("Cinvented{n:019}")
}

/// Plain JSON as a Firestore typed value.
fn typed(value: &Value) -> Value {
    match value {
        Value::String(text) => json!({ "stringValue": text }),
        Value::Bool(flag) => json!({ "booleanValue": flag }),
        Value::Number(number) => json!({ "integerValue": number.to_string() }),
        Value::Object(map) => json!({ "mapValue": { "fields": map.iter().map(|(k, v)| (k.clone(), typed(v))).collect::<serde_json::Map<_, _>>() } }),
        _ => json!({ "nullValue": null }),
    }
}

/// A `vevents` document of the invented salon (`v(n)`): its start, its `uat`, its client (`cu`,
/// plain JSON), cancelled (`dat`) or not.
fn document(n: u32, start: &str, updated: i64, client: Option<Value>, cancelled: bool) -> Value {
    let mut fields = json!({ "s": { "stringValue": start }, "d": { "integerValue": "30" }, "ca": { "stringValue": "-Cinvented0000000000" }, "uat": { "integerValue": updated.to_string() } });
    if let Some(client) = client {
        fields["cu"] = typed(&client);
    }
    if cancelled {
        fields["dat"] = json!({ "timestampValue": "2026-11-21T18:00:00Z" });
    }
    json!({ "name": format!("projects/planity-production/databases/(default)/documents/businesses/{BUSINESS}/vevents/{}", v(n)), "fields": fields })
}

/// The recorded documents and `more`, refreshed with both sources answering.
fn refreshed_with(more: Vec<Value>) -> (Value, StandIn) {
    let mut documents = recorded_documents();
    documents.extend(more);
    let mut host = stand_in(Scenario { documents, ..Scenario::default() });
    signed_in(&mut host);
    let reply = refresh(&mut host, "en");
    assert_eq!(reply["problem"], Value::Null, "{:?}", host.log);
    within_sioul_bounds(&reply);
    (reply, host)
}

/// The one person of the reply's set with this id.
fn person<'a>(reply: &'a Value, id: &str) -> &'a Value {
    reply["contacts"][0]["people"].as_array().unwrap().iter().find(|p| p["id"] == id).unwrap_or_else(|| panic!("{id} in {}", reply["contacts"]))
}

/// A mark that turns a line around, which Sioul takes out of a text.
fn direction(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// The reply's contacts within every bound Sioul counts before writing any
/// (its `reply::contact_sets`): past one, the whole reply would be refused.
fn within_sioul_bounds(reply: &Value) {
    let Some(sets) = reply.get("contacts") else { return };
    let sets = sets.as_array().unwrap();
    assert!(sets.len() <= 8, "8 sets a reply");
    let text = |value: &Value, most: usize, lines: bool| {
        let text = value.as_str().unwrap_or_else(|| panic!("a text: {value}"));
        assert!(text.chars().count() <= most, "{} characters, {most} at most", text.chars().count());
        assert_eq!(text, text.trim(), "trimmed");
        assert!(!text.chars().any(|c| (c.is_control() && !(lines && c == '\n')) || direction(c)), "cleaned: {text:?}");
    };
    for set in sets {
        let id = set["id"].as_str().unwrap();
        assert!(!id.is_empty() && id.len() <= 48 && !id.starts_with('-') && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'), "{id}");
        text(&set["name"], 256, false);
        let people = set["people"].as_array().unwrap();
        assert!(people.len() <= 20_000, "20 000 people a set");
        let mut ids = BTreeSet::new();
        for person in people {
            for key in person.as_object().unwrap().keys() {
                assert!(["id", "name", "given", "family", "phones", "emails", "org", "note"].contains(&key.as_str()), "{key} is not the core's");
            }
            assert!(ids.insert(person["id"].as_str().unwrap().to_string()), "a person handed twice");
            for key in ["id", "name"] {
                text(&person[key], 256, false);
                assert!(!person[key].as_str().unwrap().is_empty(), "{key} required");
            }
            for key in ["given", "family", "org"] {
                if let Some(value) = person.get(key) {
                    text(value, 256, false);
                }
            }
            if let Some(note) = person.get("note") {
                text(note, 2_000, true);
            }
            for key in ["phones", "emails"] {
                if let Some(all) = person.get(key) {
                    let all = all.as_array().unwrap();
                    assert!(all.len() <= 8, "8 {key} a person");
                    all.iter().for_each(|one| text(one, 256, false));
                }
            }
        }
    }
}

#[test]
fn the_clients_of_the_appointments_come_back_as_one_set_one_person_each() {
    let (reply, mut host) = refreshed_with(Vec::new());
    // Marie in six documents (01, 02, 03, 04, 06 cancelled, 11), Paul in two (05, 13 cancelled):
    // one person each; the block 12 is nobody. Given and family names never guessed.
    assert_eq!(
        reply["contacts"],
        json!([{
            "id": "clients",
            "name": "Clients",
            "people": [
                { "id": MARIE_ID, "name": MARIE, "phones": ["+33123456789"], "emails": ["marie@example.org"] },
                { "id": PAUL_ID, "name": "Paul Exemple" }
            ]
        }])
    );
    // The same word in French, the same people.
    let french = refresh(&mut host, "fr");
    assert_eq!(french["contacts"], reply["contacts"]);
    // The log counts them, never names them.
    assert!(host.log.iter().any(|l| l == "refresh: 2 client(s) handed back as contacts"), "{:?}", host.log);
    for private in [MARIE, "Paul Exemple", "marie@example.org", "+33123456789", MARIE_ID] {
        assert!(!host.log_shows(private), "{private} in the log {:?}", host.log);
    }
}

#[test]
fn the_latest_appointment_names_the_client_and_every_number_is_kept_latest_first() {
    let jeanne = client(31);
    let (reply, _) = refreshed_with(vec![
        document(31, "2026-11-03 10:00", 1_795_000_000_000, Some(json!({ "id": jeanne, "name": "Jeanne Exemple", "phone": "+33 1 00 00 00 01", "email": "Jeanne@Example.org" })), false),
        document(32, "2026-11-10 10:00", 1_795_200_000_000, Some(json!({ "id": jeanne, "name": "Jeanne Exemple-Martin", "phone": "+33100000002", "email": "jeanne@example.org" })), false),
        // Later in the agenda, but updated before 32: it does not name the client.
        document(33, "2026-11-30 10:00", 1_795_100_000_000, Some(json!({ "id": jeanne, "name": "J. Exemple", "phone": "+33100000001", "email": "jeanne.martin@example.org" })), false),
    ]);
    let jeanne = person(&reply, &jeanne);
    assert_eq!(jeanne["name"], "Jeanne Exemple-Martin", "the name of the appointment updated last, as written");
    // Two numbers, the latest first; 31's number is 33's in another spelling, kept once in its latest.
    assert_eq!(jeanne["phones"], json!(["+33100000002", "+33100000001"]));
    // An address seen in two cases is one address.
    assert_eq!(jeanne["emails"], json!(["jeanne@example.org", "jeanne.martin@example.org"]));
    assert_eq!((jeanne.get("given"), jeanne.get("family")), (None, None), "one string is never split");
}

#[test]
fn a_block_and_a_nameless_client_give_nobody_and_a_cancelled_appointments_client_counts() {
    let (reply, host) = refreshed_with(vec![
        // A client with no name anywhere, a name without an id, a block without any client.
        document(34, "2026-11-04 10:00", 1_795_000_000_000, Some(json!({ "id": client(34), "phone": "+33100000034" })), false),
        document(35, "2026-11-05 10:00", 1_795_000_000_000, Some(json!({ "name": "Sans Identifiant" })), false),
        document(36, "2026-11-06 10:00", 1_795_000_000_000, None, false),
        document(37, "2026-11-07 10:00", 1_795_000_000_000, Some(json!({ "d": true })), false),
        // A cancelled appointment's client is still a client.
        document(38, "2026-11-08 10:00", 1_795_000_000_000, Some(json!({ "id": client(38), "name": "Lucie Exemple", "email": "lucie@example.org" })), true),
        // Named on an older appointment only: that name, and the newer one's number.
        document(39, "2026-11-09 10:00", 1_795_000_000_000, Some(json!({ "id": client(39), "name": "Hugo Exemple" })), false),
        document(40, "2026-11-12 10:00", 1_795_300_000_000, Some(json!({ "id": client(39), "phone": "+33100000039" })), false),
    ]);
    let people = reply["contacts"][0]["people"].as_array().unwrap();
    let ids: Vec<&str> = people.iter().map(|p| p["id"].as_str().unwrap()).collect();
    assert_eq!(ids.len(), 4, "Marie, Paul, Lucie, Hugo: {ids:?}");
    assert!(!ids.contains(&client(34).as_str()) && !people.iter().any(|p| p["name"] == "Sans Identifiant"), "{ids:?}");
    assert_eq!(*person(&reply, &client(38)), json!({ "id": client(38), "name": "Lucie Exemple", "emails": ["lucie@example.org"] }));
    assert_eq!(*person(&reply, &client(39)), json!({ "id": client(39), "name": "Hugo Exemple", "phones": ["+33100000039"] }));
    assert!(host.log.iter().any(|l| l == "refresh: 1 client(s) without a name or a usable id left out"), "{:?}", host.log);
}

#[test]
fn without_firestores_answer_the_reply_carries_no_contacts() {
    let no_contacts = |host: &mut StandIn, what: &str| {
        let reply = refresh(host, "en");
        assert!(reply["agendas"].as_array().is_some_and(|a| !a.is_empty()) || reply["problem"].is_object(), "{what}: {reply}");
        assert_eq!(reply.get("contacts"), None, "{what}: no key at all");
    };
    // No Firebase session: the feed alone, which holds no client.
    let mut host = stand_in(Scenario::default());
    host.secret = Some(json!({ "b": BUSINESS, "l": link() }).to_string());
    no_contacts(&mut host, "no session");
    // A session refused, Firestore refusing, Firestore unreachable: the feed alone too.
    for scenario in [Scenario { expired_refresh: true, ..Scenario::default() }, Scenario { firestore_status: Some(403), ..Scenario::default() }, Scenario { firestore_status: Some(503), ..Scenario::default() }] {
        let mut host = stand_in(scenario);
        signed_in(&mut host);
        no_contacts(&mut host, "Firestore not read");
    }
    // Both failing: nothing at all.
    let mut host = stand_in(Scenario { firestore_status: Some(503), feed: (500, String::new()), ..Scenario::default() });
    signed_in(&mut host);
    no_contacts(&mut host, "both failing");
    // The feed failing, Firestore answering: the clients still come.
    let mut host = stand_in(Scenario { feed: (500, String::new()), ..Scenario::default() });
    signed_in(&mut host);
    let reply = refresh(&mut host, "en");
    assert_eq!(reply["contacts"][0]["people"].as_array().map(Vec::len), Some(2), "{reply}");
    // Firestore answering with no client at all: the set, empty, so that its address book can be chosen.
    let mut host = stand_in(Scenario { documents: Vec::new(), ..Scenario::default() });
    signed_in(&mut host);
    assert_eq!(refresh(&mut host, "en")["contacts"], json!([{ "id": "clients", "name": "Clients", "people": [] }]));
}

#[test]
fn the_contacts_stay_within_sioul_s_bounds_a_long_text_cut_never_the_reply_refused() {
    let long = format!("Marie \u{202E}{}\u{0007}", "é".repeat(300));
    let documents = (0..10u32)
        .map(|i| {
            let email = match i {
                0 => format!("{}@example.org", "m".repeat(300)),
                9 => "marie.longue@example.org".to_string(),
                _ => String::new(),
            };
            let mut cu = json!({ "id": client(41), "name": long, "phone": format!("+3310000{i:04}") });
            if !email.is_empty() {
                cu["email"] = json!(email);
            }
            document(41 + i, &format!("2026-11-{:02} 09:00", 10 + i), 1_795_000_000_000 + i64::from(i) * 1000, Some(cu), false)
        })
        .collect();
    let (reply, _) = refreshed_with(documents);
    let marie = person(&reply, &client(41));
    let name = marie["name"].as_str().unwrap();
    assert!(name.starts_with("Marie é") && name.chars().count() == 256, "cut to 256 characters, cleaned: {}", name.chars().count());
    // Ten numbers: the eight latest; an address too long is left out, never cut into another.
    let phones: Vec<String> = (2..10).rev().map(|i| format!("+3310000{i:04}")).collect();
    assert_eq!(marie["phones"], json!(phones));
    assert_eq!(marie["emails"], json!(["marie.longue@example.org"]));
}

#[test]
fn one_clients_json_is_the_shape_sioul_reads() {
    let only = document(51, "2026-11-25 14:00", 1_794_900_000_000, Some(json!({ "id": client(51), "name": MARIE, "email": "marie@example.org", "phone": "+33123456789" })), false);
    let mut host = stand_in(Scenario { documents: vec![only], ..Scenario::default() });
    signed_in(&mut host);
    let reply = refresh(&mut host, "en");
    within_sioul_bounds(&reply);
    // Sioul's docs/extensions.md, "The calls": {"contacts": [{"id", "name", "people": [{"id", "name",
    // "given", "family", "phones", "emails", "org", "note"}]}]}, only the id and the name required.
    assert_eq!(
        reply["contacts"],
        json!([{ "id": "clients", "name": "Clients", "people": [{ "id": "Cinvented0000000000000000051", "name": "Marie Exemple", "phones": ["+33123456789"], "emails": ["marie@example.org"] }] }])
    );
    assert_eq!(reply.as_object().unwrap().keys().collect::<Vec<_>>(), vec!["agendas", "contacts", "problem"]);
}
