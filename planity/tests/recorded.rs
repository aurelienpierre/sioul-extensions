// SPDX-License-Identifier: GPL-3.0-or-later

//! The extension against recorded answers only, never a live server: the
//! answers in `tests/recorded/` are written from the request/response shapes
//! validated in `~/Life/coding/sioul/planity/` (`api.md`, `extension-spec.md`),
//! with an invented business (`-Binvented0000000000`), an invented client
//! ("Marie Exemple", `marie@example.org`, `+33123456789`) and short fake
//! tokens. Nothing here is real but the Firebase API key and the Planity
//! hostnames, both public. The stand-in host answers only the hosts the
//! repository's manifest names for this extension, as Sioul does.

use crate::Planity;
use serde_json::{Value, json};
use sioul_extension::standin::StandIn;
use sioul_extension::{Answer, Request, run};

/// 2026-11-22T10:00:00Z.
const NOW: i64 = 1_795_341_600;
const BUSINESS: &str = "-Binvented0000000000";
const REFRESH_B: &str = "RT-custom-B-invented-000000000000";
const PASSWORD: &str = "motdepasse-invented";

fn recorded(name: &str) -> String {
    std::fs::read_to_string(format!("{}/tests/recorded/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// The hosts the repository's manifest names for the planity extension.
fn manifest_hosts() -> Vec<String> {
    let manifest = std::fs::read_to_string(format!("{}/../manifest.toml", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let section = manifest.split("id = \"planity\"").nth(1).expect("planity in the manifest");
    let list = section.split("hosts = [").nth(1).unwrap().split(']').next().unwrap();
    list.split(',').map(|h| h.trim().trim_matches('"').to_string()).filter(|h| !h.is_empty()).collect()
}

/// What the answers say, as each test sets it.
#[derive(Clone)]
struct Scenario {
    /// The password sign-in answers a bad-credentials error.
    bad_password: bool,
    /// The token refresh answers an expired-refresh error.
    expired_refresh: bool,
    /// The `vevents` documents the salon holds (the inner `{name, fields}`),
    /// unsorted; the Firestore stand-in sorts, filters and pages them.
    documents: Vec<Value>,
}

impl Default for Scenario {
    fn default() -> Self {
        Self { bad_password: false, expired_refresh: false, documents: recorded_documents() }
    }
}

/// The three recorded documents (the service appointment, the all-day block,
/// the cancelled one): the elements of `vevents.json` that carry a document.
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
            return if scenario.expired_refresh { Answer::new(400, recorded("securetoken-expired.json")) } else { Answer::new(200, recorded("securetoken.json")) };
        }
        if url.contains("firestore.googleapis.com") && url.ends_with(":runQuery") {
            assert!(request.headers.iter().any(|(n, v)| n.eq_ignore_ascii_case("Authorization") && v.starts_with("Bearer ") && v.len() > 7), "the id token rides as a bearer: {:?}", request.headers);
            assert!(url.contains(&format!("/businesses/{BUSINESS}:runQuery")), "{url}");
            return run_query(&scenario, &body);
        }
        panic!("not recorded: {url}");
    }
}

fn stand_in(scenario: Scenario) -> StandIn {
    let mut host = StandIn::new(NOW);
    host.hosts = manifest_hosts();
    host.web = Box::new(web(scenario));
    host
}

fn call(host: &mut StandIn, text: Value) -> Value {
    serde_json::from_str(&run::<Planity>(host, text.to_string().as_bytes())).unwrap()
}

/// A sign-in kept, as `sign-in` keeps it.
fn signed_in(host: &mut StandIn) {
    host.secret = Some(json!({ "r": REFRESH_B, "b": BUSINESS }).to_string());
}

/// The agenda's calendar text with its folded lines joined.
fn unfolded(agenda: &Value) -> String {
    agenda["calendar"].as_str().unwrap().replace("\r\n ", "")
}

/// One VEVENT of a calendar, by its UID.
fn vevent(calendar: &str, uid: &str) -> String {
    calendar.split("BEGIN:VEVENT\r\n").find(|event| event.starts_with(&format!("UID:{uid}\r\n"))).unwrap_or_else(|| panic!("{uid} in {calendar}")).to_string()
}

#[test]
fn the_full_sign_in_dance_keeps_only_the_refresh_token_and_business() {
    let mut host = stand_in(Scenario::default());
    let reply = host.sign_in_with::<Planity>("en", None, &[("email", "marie@example.org"), ("password", PASSWORD)]);
    assert_eq!(reply, json!({ "signed_in": true }), "{:?}", host.log);
    // The secret holds the refresh token B and the business id decoded from token B — never the password.
    let kept: Value = serde_json::from_str(host.secret.as_deref().unwrap()).unwrap();
    assert_eq!((kept["r"].as_str(), kept["b"].as_str()), (Some(REFRESH_B), Some(BUSINESS)));
    assert_eq!(kept.as_object().unwrap().len(), 2, "only r and b: {kept}");
    assert!(!host.secret.as_deref().unwrap().contains(PASSWORD), "the password is never kept");
    assert!(!host.secret.as_deref().unwrap().contains("marie@example.org"), "the email is never kept");
    // The password and every token are in the hidden set (the engine and the extension both hide them).
    for secret in [PASSWORD, "marie@example.org", "AT-password-A-invented-000000000000", "RT-password-A-invented-000000", "CT-invented-custom-token-000000000000", REFRESH_B] {
        assert!(host.hidden.iter().any(|h| h == secret), "{secret} not hidden");
        assert!(!host.log_shows(secret), "{secret} in the log {:?}", host.log);
    }
    // The dance is four calls, in order.
    let urls: Vec<&str> = host.asked.iter().map(|r| r.url.as_str()).collect();
    assert!(urls[0].contains("signInWithPassword") && urls[1].contains("getProAuthToken") && urls[2].contains("signInWithCustomToken"), "{urls:?}");
}

#[test]
fn a_bad_password_is_a_sign_in_problem() {
    let mut host = stand_in(Scenario { bad_password: true, ..Scenario::default() });
    let reply = host.sign_in_with::<Planity>("fr", None, &[("email", "marie@example.org"), ("password", "mauvais-mot-de-passe")]);
    assert_eq!(reply["problem"]["kind"], "sign-in", "{:?}", host.log);
    assert_eq!(host.secret, None, "nothing kept on a bad password");
    // It stops at the password step: no Pro-token exchange.
    assert!(!host.asked.iter().any(|r| r.url.contains("getProAuthToken")));
    // No credentials given at all is also a sign-in problem, with no call.
    let mut host = stand_in(Scenario::default());
    let reply = call(&mut host, json!({ "interface": 1, "call": "sign-in", "lang": "en", "choice": null }));
    assert_eq!(reply["problem"]["kind"], "sign-in");
    assert!(host.asked.is_empty());
}

#[test]
fn a_refresh_mints_a_token_and_brings_one_busy_agenda() {
    let mut host = stand_in(Scenario::default());
    signed_in(&mut host);
    let reply = call(&mut host, json!({ "interface": 1, "call": "refresh", "lang": "en" }));
    assert_eq!(reply["problem"], Value::Null, "{:?}", host.log);
    let agendas = reply["agendas"].as_array().unwrap();
    assert_eq!(agendas.len(), 1, "one calendar, one agenda");
    let agenda = &agendas[0];
    assert_eq!(agenda["id"], "cinvented0000000000", "a stable id from the calendar ca");
    assert_eq!(agenda["name"], "Planity");
    assert_eq!(agenda["busy"], true, "the salon's own appointments are busy time");
    assert!(agenda["period"]["from"].is_string() && agenda["period"]["until"].is_string(), "the window read");
    // A fresh id token was minted through securetoken first.
    assert!(host.asked.iter().any(|r| r.url.contains("securetoken.googleapis.com")));
    // The refresh token was hidden; nothing of it in the log.
    assert!(host.hidden.iter().any(|h| h == REFRESH_B));
    assert!(!host.log_shows(REFRESH_B));
}

#[test]
fn the_mapping_titles_prices_all_day_and_cancelled() {
    let mut host = stand_in(Scenario::default());
    signed_in(&mut host);
    let reply = call(&mut host, json!({ "interface": 1, "call": "refresh", "lang": "en" }));
    let calendar = unfolded(&reply["agendas"][0]);

    // A service appointment: titled by the client, timed, with duration, price, phone and note.
    let service = vevent(&calendar, "SQinvented0000000000002");
    for line in [
        "DTSTART:20261125T133000Z",
        "DTEND:20261125T140000Z",
        "SUMMARY:Marie Exemple",
        "STATUS:CONFIRMED",
    ] {
        assert!(service.contains(&format!("{line}\r\n")), "{line} in {service}");
    }
    assert!(service.contains("Duration: 30 min"), "{service}");
    // 4500 cents as euros; the comma is iCalendar-escaped (RFC 5545 §3.3.11).
    assert!(service.contains("45\\,00") && service.contains('€'), "the price 4500 cents as euros: {service}");
    assert!(service.contains("+33123456789") && service.contains("Coupe et brushing"), "{service}");
    assert!(!service.contains("TRANSP:"), "a busy event writes no TRANSP; the door decides: {service}");

    // An all-day block: a floating DATE event, no client → "Unavailable".
    let block = vevent(&calendar, "SQinvented0000000000003");
    assert!(block.contains("DTSTART;VALUE=DATE:20261224\r\n") && block.contains("DTEND;VALUE=DATE:20261225\r\n"), "{block}");
    assert!(block.contains("SUMMARY:Unavailable\r\n"), "{block}");
    assert!(block.contains("Congés"), "the note still shows: {block}");

    // A cancelled appointment: STATUS:CANCELLED, still titled by its client.
    let cancelled = vevent(&calendar, "SQinvented0000000000001");
    assert!(cancelled.contains("STATUS:CANCELLED\r\n") && cancelled.contains("SUMMARY:Client Annulé\r\n"), "{cancelled}");
    assert!(cancelled.contains("DTSTART:20261120T080000Z\r\n"), "09:00 Paris winter is 08:00 UTC: {cancelled}");

    // In French, the shown words change; the block reads « Indisponible ».
    let reply = call(&mut host, json!({ "interface": 1, "call": "refresh", "lang": "fr" }));
    let calendar = unfolded(&reply["agendas"][0]);
    assert!(vevent(&calendar, "SQinvented0000000000003").contains("SUMMARY:Indisponible\r\n"), "{calendar}");
    assert!(vevent(&calendar, "SQinvented0000000000002").contains("Durée"), "a French duration label: {calendar}");

    // The same answers make the same bytes.
    let again = call(&mut host, json!({ "interface": 1, "call": "refresh", "lang": "fr" }));
    assert_eq!(again["agendas"][0]["calendar"], reply["agendas"][0]["calendar"]);
}

#[test]
fn the_window_is_bounded_both_ends() {
    // A document just before the window (server-side) and one just past it (client-side) are both left out.
    let mut documents = recorded_documents();
    documents.push(json!({ "name": "projects/planity-production/databases/(default)/documents/businesses/-Binvented0000000000/vevents/SQbefore00000000000000", "fields": { "s": { "stringValue": "2026-08-01 10:00" }, "ca": { "stringValue": "-Cinvented0000000000" } } }));
    documents.push(json!({ "name": "projects/planity-production/databases/(default)/documents/businesses/-Binvented0000000000/vevents/SQafter000000000000000", "fields": { "s": { "stringValue": "2027-04-01 10:00" }, "ca": { "stringValue": "-Cinvented0000000000" } } }));
    let mut host = stand_in(Scenario { documents, ..Scenario::default() });
    signed_in(&mut host);
    let reply = call(&mut host, json!({ "interface": 1, "call": "refresh", "lang": "en" }));
    let calendar = unfolded(&reply["agendas"][0]);
    assert!(!calendar.contains("SQbefore"), "a document before the window is not returned by the server");
    assert!(!calendar.contains("SQafter"), "a document past `until` is dropped client-side");
    assert!(calendar.contains("SQinvented0000000000002"), "an in-window document is kept");
    // The window's `from` is sixty days back, as a Europe/Paris wall-clock string.
    let query = host.asked.iter().find(|r| r.url.ends_with(":runQuery")).unwrap();
    assert!(query.body.as_deref().unwrap().contains("\"stringValue\":\"2026-09-23"), "from ~ now-60d in Paris: {:?}", query.body);
}

#[test]
fn pagination_reads_every_page() {
    // More documents than one Firestore page: all are read over two pages.
    let documents: Vec<Value> = (0..350)
        .map(|i| json!({
            "name": format!("projects/planity-production/databases/(default)/documents/businesses/-Binvented0000000000/vevents/SQgen{i:04}"),
            "fields": { "s": { "stringValue": format!("2026-11-01 {:02}:{:02}", i / 60, i % 60) }, "d": { "integerValue": "15" }, "ca": { "stringValue": "-Cinvented0000000000" } }
        }))
        .collect();
    let mut host = stand_in(Scenario { documents, ..Scenario::default() });
    signed_in(&mut host);
    let reply = call(&mut host, json!({ "interface": 1, "call": "refresh", "lang": "en" }));
    assert_eq!(reply["problem"], Value::Null, "{:?}", host.log);
    let calendar = unfolded(&reply["agendas"][0]);
    assert_eq!(calendar.matches("BEGIN:VEVENT\r\n").count(), 350, "every document became an event");
    let pages = host.asked.iter().filter(|r| r.url.ends_with(":runQuery")).count();
    assert_eq!(pages, 2, "two pages were read");
    // The second page started strictly after the first page's last document.
    let second = host.asked.iter().filter(|r| r.url.ends_with(":runQuery")).nth(1).unwrap();
    assert!(second.body.as_deref().unwrap().contains("\"startAt\""), "the second page paged on a cursor: {:?}", second.body);
}

#[test]
fn an_expired_refresh_asks_to_sign_in_again() {
    let mut host = stand_in(Scenario { expired_refresh: true, ..Scenario::default() });
    signed_in(&mut host);
    let before = host.secret.clone();
    let reply = call(&mut host, json!({ "interface": 1, "call": "refresh", "lang": "en" }));
    assert_eq!(reply["problem"]["kind"], "sign-in");
    assert_eq!(reply["agendas"], json!([]));
    assert_eq!(host.secret, before, "the sign-in kept is left for the person to renew");
    // No sign-in kept at all is also a sign-in problem, with no call.
    let mut host = stand_in(Scenario::default());
    let reply = call(&mut host, json!({ "interface": 1, "call": "refresh", "lang": "en" }));
    assert_eq!(reply["problem"]["kind"], "sign-in");
    assert!(host.asked.is_empty());
}

#[test]
fn every_request_goes_to_a_host_the_manifest_names() {
    let mut host = stand_in(Scenario::default());
    host.sign_in_with::<Planity>("en", None, &[("email", "marie@example.org"), ("password", PASSWORD)]);
    signed_in(&mut host);
    call(&mut host, json!({ "interface": 1, "call": "refresh", "lang": "en" }));
    let hosts = manifest_hosts();
    assert!(!host.asked.is_empty());
    for request in &host.asked {
        let named = sioul_extension::url::https_host(&request.url).unwrap();
        assert!(hosts.contains(&named), "{named} is not in the manifest");
    }
    // The four hosts the dance and the read use are all named.
    for host_named in ["identitytoolkit.googleapis.com", "securetoken.googleapis.com", "firestore.googleapis.com", "product.api.euwest1.prod.planityapp.com"] {
        assert!(hosts.contains(&host_named.to_string()), "{host_named} missing from the manifest");
    }
}
