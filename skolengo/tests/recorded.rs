// SPDX-License-Identifier: GPL-3.0-or-later

//! The extension against recorded answers only, never a real platform: the
//! answers in `tests/recorded/` are written from the field names the
//! community clients read (TS `models/`, HA `api.py`), with invented people,
//! schools and platform (`platforms::EXAMPLE`, which exists in the tests'
//! builds alone: this file is the crate's own `mod recorded`). The stand-in
//! host answers only the hosts the repository's manifest names, and the
//! invented platform's, as Sioul does.

use serde_json::{Value, json};
use sioul_extension::standin::StandIn;
use sioul_extension::{Answer, Request, SignIn, run};
use crate::{CLIENT_SECRET, RETURN, Skolengo};
use std::cell::RefCell;
use std::rc::Rc;

/// 2026-11-18T10:00:00Z, a Wednesday.
const NOW: i64 = 1_794_996_000;
const ALEX: &str = "ESKO-P-00000000-0000-4000-8000-000000000001";
const SAM: &str = "ESKO-P-00000000-0000-4000-8000-000000000002";
const SCHOOL_B: &str = "SKO-E-00000000-0000-4000-8000-00000000000b";

fn recorded(name: &str) -> String {
    std::fs::read_to_string(format!("{}/tests/recorded/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// The hosts the repository's manifest names for this extension.
fn manifest_hosts() -> Vec<String> {
    let manifest = std::fs::read_to_string(format!("{}/../manifest.toml", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let list = manifest.split("hosts = [").nth(1).unwrap().split(']').next().unwrap();
    list.split(',').map(|h| h.trim().trim_matches('"').to_string()).filter(|h| !h.is_empty()).collect()
}

/// What the platform answers, as recorded; what each test changes.
#[derive(Clone)]
struct Platform {
    /// The renewal: the answer's file and its status.
    renewal: (u16, &'static str),
    /// The gateway answers 401 to this many requests first.
    refuse_first: usize,
    /// A day whose ranges answer 500 for Alex.
    bad_day: Option<&'static str>,
}

impl Default for Platform {
    fn default() -> Self {
        Self { renewal: (200, "token-renewed.json"), refuse_first: 0, bad_day: Some("2026-11-20") }
    }
}

fn param(url: &str, name: &str) -> String {
    sioul_extension::url::query_value(url, name).unwrap_or_default()
}

/// A JSON:API document of the days of `file` from `from` to `until`, as the gateway gives a range.
fn days(file: &str, from: &str, until: &str) -> String {
    let mut document: Value = serde_json::from_str(&recorded(file)).unwrap();
    let kept: Vec<Value> = document["data"].as_array().unwrap().iter().filter(|d| d["attributes"]["date"].as_str().is_some_and(|date| date >= from && date <= until)).cloned().collect();
    document["data"] = Value::Array(kept);
    document.to_string()
}

fn web(platform: Platform) -> impl FnMut(&Request) -> Answer {
    let refused = Rc::new(RefCell::new(platform.refuse_first));
    move |request: &Request| {
        let url = request.url.as_str();
        let header = |name: &str| request.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone()).unwrap_or_default();
        if url == "https://sso.example.org/oidc/.well-known" {
            return Answer::new(200, recorded("discovery.json"));
        }
        if url == "https://sso.example.org/oidc/token" {
            let body = request.body.clone().unwrap_or_default();
            assert!(body.contains(&format!("client_secret={CLIENT_SECRET}")), "{body}");
            if body.contains("grant_type=authorization_code") {
                assert!(body.contains("code=OC-0000-recorded") && body.contains("redirect_uri=skoapp-prod%3A%2F%2Fsign-in-callback"), "{body}");
                return Answer::new(200, recorded("token-code.json"));
            }
            assert!(body.contains("grant_type=refresh_token"), "{body}");
            return Answer::new(platform.renewal.0, recorded(platform.renewal.1));
        }
        let api = url.strip_prefix("https://api.skolengo.com/api/v1/bff-sko-app").expect("only the gateway is asked");
        assert_eq!(header("X-Skolengo-Ems-Code"), "example");
        assert_eq!(header("X-Skolengo-Date-Format"), "utc");
        if *refused.borrow() > 0 {
            *refused.borrow_mut() -= 1;
            return Answer::new(401, "");
        }
        let token = header("Authorization");
        assert!(token == "Bearer AT-first-0000000000000000" || token == "Bearer AT-renewed-00000000000000" || token == "Bearer AT-kept-000000000000000", "{token}");
        if api.starts_with("/users-info/ESKO-U-00000000-0000-4000-8000-0000000000aa?") {
            assert!(param(url, "include").contains("students.school"));
            return Answer::new(200, recorded("users-info-parent.json"));
        }
        if api.starts_with("/agendas?") {
            assert!(!url.contains("page%5B"), "no page parameter (HA api.py:1147-1157)");
            let (pupil, from, until) = (param(url, "filter[student.id]"), param(url, "filter[date][GE]"), param(url, "filter[date][LE]"));
            assert!(from <= until && until.as_str() <= "2026-12-16" && from.as_str() >= "2026-11-11", "{from}..{until}");
            if pupil == ALEX {
                if platform.bad_day.is_some_and(|bad| from.as_str() <= bad && bad <= until.as_str()) {
                    return Answer::new(500, "{\"errors\":[{\"status\":\"500\"}]}");
                }
                return Answer::new(200, days("agendas-alex.json", &from, &until));
            }
            assert_eq!(pupil, SAM);
            assert_eq!(header("X-Skolengo-School-Id"), SCHOOL_B, "each pupil's own school");
            return Answer::new(200, days("agendas-sam.json", &from, &until));
        }
        panic!("not recorded: {url}");
    }
}

fn stand_in(platform: Platform) -> StandIn {
    let mut host = StandIn::new(NOW);
    host.hosts = manifest_hosts();
    host.hosts.push("sso.example.org".into());
    host.web = Box::new(web(platform));
    host
}

fn call(host: &mut StandIn, text: Value) -> Value {
    serde_json::from_str(&run::<Skolengo>(host, text.to_string().as_bytes())).unwrap()
}

/// A sign-in kept, as `sign-in` keeps it; the access token's end is `expires`.
fn signed_in(host: &mut StandIn, expires: i64) {
    host.secret = Some(
        json!({
            "platform": "example",
            "token_endpoint": "https://sso.example.org/oidc/token",
            "refresh": "RT-kept-000000000000000",
            "sub": "ESKO-U-00000000-0000-4000-8000-0000000000aa",
            "access": "AT-kept-000000000000000",
            "expires": expires
        })
        .to_string(),
    );
}

#[test]
fn the_sign_in_s_steps() {
    let mut host = stand_in(Platform::default());
    // First the platforms, each by its code.
    let reply = call(&mut host, json!({ "interface": 1, "call": "sign-in", "lang": "fr", "choice": null }));
    let choices = reply["choose"].as_array().unwrap();
    assert_eq!(choices.len(), 19);
    assert!(choices.iter().all(|c| c["id"].as_str().is_some_and(|i| !i.is_empty()) && c["name"].as_str().is_some_and(|n| !n.is_empty())));
    // Then the platform's page, and its return with the state the page was opened with.
    host.page = Some(Box::new(|open: &str| SignIn::Returned(format!("{RETURN}?code=OC-0000-recorded&state={}", param(open, "state")))));
    let reply = call(&mut host, json!({ "interface": 1, "call": "sign-in", "lang": "fr", "choice": "example" }));
    assert_eq!(reply, json!({ "signed_in": true }), "{:?}", host.log);
    let (open, watch) = &host.opened[0];
    assert_eq!(watch, RETURN);
    assert!(open.starts_with("https://sso.example.org/oidc/authorize?response_type=code&client_id=SkoApp.Prod.0d349217"), "{open}");
    assert!(open.contains("redirect_uri=skoapp-prod%3A%2F%2Fsign-in-callback") && !open.contains("code_challenge"), "{open}");
    assert!(open.contains("scope=openid+profile+email&"), "the scopes the platform lists among HA's: {open}");
    // E1: a state that cannot be guessed (the review's scratch test), another at each sign-in.
    let state = param(open, "state");
    assert!(state.len() >= 22 && state != format!("sioul-{NOW}"), "a state that cannot be guessed: {state}");
    host.page = Some(Box::new(|_: &str| SignIn::Cancelled));
    call(&mut host, json!({ "interface": 1, "call": "sign-in", "choice": "example" }));
    let other = param(&host.opened[1].0, "state");
    assert!(other.len() >= 22 && other != state, "a second sign-in at the same second sends another state: {other}");
    let kept: Value = serde_json::from_str(host.secret.as_deref().unwrap()).unwrap();
    assert_eq!((kept["platform"].as_str(), kept["refresh"].as_str(), kept["sub"].as_str()), (Some("example"), Some("RT-first-0000000000000000"), Some("ESKO-U-00000000-0000-4000-8000-0000000000aa")));
    assert_eq!(kept["expires"], NOW + 86_400);
    for secret in ["OC-0000-recorded", "RT-first-0000000000000000", "AT-first-0000000000000000"] {
        assert!(!host.log_shows(secret), "{secret} in {:?}", host.log);
    }
    // A return with another state, or none at all, keeps nothing new.
    let mut host = stand_in(Platform::default());
    host.sign_in = Some(SignIn::Returned(format!("{RETURN}?code=OC-0000-recorded&state=sioul-1")));
    let reply = call(&mut host, json!({ "interface": 1, "call": "sign-in", "choice": "example" }));
    assert_eq!(reply["problem"]["kind"], "sign-in");
    assert_eq!(host.secret, None);
    host.sign_in = Some(SignIn::Cancelled);
    let reply = call(&mut host, json!({ "interface": 1, "call": "sign-in", "choice": "example" }));
    assert_eq!(reply, json!({ "cancelled": true }));
    assert_eq!(host.secret, None);
}

#[test]
fn a_refresh_brings_one_agenda_per_child() {
    let mut host = stand_in(Platform::default());
    signed_in(&mut host, NOW + 3_600);
    let reply = call(&mut host, json!({ "interface": 1, "call": "refresh", "lang": "en" }));
    assert_eq!(reply["problem"], Value::Null, "{:?}", host.log);
    let agendas = reply["agendas"].as_array().unwrap();
    assert_eq!(agendas.len(), 2);
    assert_eq!((agendas[0]["id"].as_str(), agendas[0]["name"].as_str()), (Some("esko-p-00000000-0000-4000-8000-000000000001"), Some("Timetable: Alex")));
    assert_eq!((agendas[1]["id"].as_str(), agendas[1]["name"].as_str()), (Some("esko-p-00000000-0000-4000-8000-000000000002"), Some("Timetable: Sam")));
    let sam = agendas[1]["calendar"].as_str().unwrap();
    assert!(sam.contains("UID:L-S1@skolengo\r\n") && sam.contains("LOCATION:Labo 2\r\n") && !sam.contains("L-A1"), "{sam}");
    // A week back, four ahead, fifteen days a request: three requests a pupil when no day fails.
    let asked_sam = host.asked.iter().filter(|r| r.url.contains("ESKO-P-00000000-0000-4000-8000-000000000002")).count();
    assert_eq!(asked_sam, 3);
    // Nothing renewed: the access token kept was still good.
    assert!(!host.asked.iter().any(|r| r.url.ends_with("/oidc/token")));
    // In French, the names too.
    let reply = call(&mut host, json!({ "interface": 1, "call": "refresh", "lang": "fr" }));
    assert_eq!(reply["agendas"][1]["name"], "Emploi du temps\u{202f}: Sam");
}

#[test]
fn a_lesson_is_an_event_and_a_cancelled_one_cancelled() {
    let mut host = stand_in(Platform::default());
    signed_in(&mut host, NOW + 3_600);
    let reply = call(&mut host, json!({ "interface": 1, "call": "refresh", "lang": "en" }));
    let alex = reply["agendas"][0]["calendar"].as_str().unwrap().replace("\r\n ", "");
    let event = |uid: &str| alex.split("BEGIN:VEVENT\r\n").find(|e| e.starts_with(&format!("UID:{uid}@skolengo"))).unwrap_or_else(|| panic!("{uid} in {alex}")).to_string();
    let maths = event("L-A1");
    for line in ["DTSTART:20261119T080000Z", "DTEND:20261119T090000Z", "SUMMARY:Mathématiques", "LOCATION:B12", "DESCRIPTION:M. Jean Exemple", "STATUS:CONFIRMED", "TRANSP:TRANSPARENT"] {
        assert!(maths.contains(&format!("{line}\r\n")), "{line} in {maths}");
    }
    let history = event("L-A2");
    assert!(history.contains("STATUS:CANCELLED\r\n") && history.contains("SUMMARY:Histoire-géographie\r\n"), "{history}");
    assert!(event("L-A4").contains("DESCRIPTION:M. Jean Exemple\r\n") && !event("L-A4").contains("LOCATION:"));
    // The same answers make the same bytes: nothing to write on the next refresh.
    let again = call(&mut host, json!({ "interface": 1, "call": "refresh", "lang": "en" }));
    assert_eq!(again["agendas"][0]["calendar"], reply["agendas"][0]["calendar"]);
}

#[test]
fn a_day_answering_500_keeps_what_it_had() {
    let mut host = stand_in(Platform::default());
    signed_in(&mut host, NOW + 3_600);
    let reply = call(&mut host, json!({ "interface": 1, "call": "refresh", "lang": "en" }));
    assert_eq!(reply["problem"], Value::Null);
    let alex = &reply["agendas"][0];
    // 20 November alone, in Paris's day: what Sioul had there stays.
    assert_eq!(alex["unread"], json!([{ "from": "2026-11-19T23:00:00Z", "until": "2026-11-20T23:00:00Z" }]));
    let calendar = alex["calendar"].as_str().unwrap();
    assert!(calendar.contains("UID:L-A1@skolengo") && calendar.contains("UID:L-A4@skolengo") && !calendar.contains("UID:L-A3@skolengo"));
    assert!(host.log.iter().any(|l| l.contains("500 for 2026-11-20 alone")), "{:?}", host.log);
    assert_eq!(reply["agendas"][1]["unread"], json!([]), "the other child's days were all read");
    // Every day failing: no agenda for that child (the last copy stays whole), a problem said.
    let mut host = stand_in(Platform { bad_day: None, ..Platform::default() });
    signed_in(&mut host, NOW + 3_600);
    host.web = Box::new({
        let mut inner = web(Platform::default());
        move |request: &Request| if request.url.contains("ESKO-P-00000000-0000-4000-8000-000000000001") { Answer::new(500, "") } else { inner(request) }
    });
    let reply = call(&mut host, json!({ "interface": 1, "call": "refresh", "lang": "en" }));
    assert_eq!(reply["agendas"].as_array().unwrap().len(), 1);
    assert_eq!(reply["agendas"][0]["name"], "Timetable: Sam");
    assert_eq!(reply["problem"]["kind"], "unreachable");
    assert!(host.asked.len() < 40, "the halvings are capped: {}", host.asked.len());
}

#[test]
fn a_renewal_keeps_the_new_tokens() {
    // The access token kept has ended: renewed first, the new refresh token kept.
    let mut host = stand_in(Platform::default());
    signed_in(&mut host, NOW - 10);
    let reply = call(&mut host, json!({ "interface": 1, "call": "refresh", "lang": "en" }));
    assert_eq!(reply["problem"], Value::Null, "{:?}", host.log);
    assert_eq!(reply["agendas"].as_array().unwrap().len(), 2);
    let renewal = host.asked.iter().find(|r| r.url.ends_with("/oidc/token")).unwrap();
    assert!(renewal.body.as_deref().unwrap().contains("refresh_token=RT-kept-000000000000000"));
    let kept: Value = serde_json::from_str(host.secret.as_deref().unwrap()).unwrap();
    assert_eq!((kept["refresh"].as_str(), kept["access"].as_str()), (Some("RT-renewed-00000000000000"), Some("AT-renewed-00000000000000")));
    assert_eq!(kept["expires"], NOW + 86_400);
    for secret in ["RT-kept-000000000000000", "RT-renewed-00000000000000", "AT-renewed-00000000000000"] {
        assert!(!host.log_shows(secret), "{secret}");
    }
    // A token the gateway refuses (401) is renewed once, and the request asked again.
    let mut host = stand_in(Platform { refuse_first: 1, ..Platform::default() });
    signed_in(&mut host, NOW + 3_600);
    let reply = call(&mut host, json!({ "interface": 1, "call": "refresh", "lang": "en" }));
    assert_eq!(reply["problem"], Value::Null, "{:?}", host.log);
    assert_eq!(host.asked.iter().filter(|r| r.url.ends_with("/oidc/token")).count(), 1);
}

#[test]
fn a_refused_token_asks_to_sign_in_again() {
    let mut host = stand_in(Platform { renewal: (400, "token-refused.json"), ..Platform::default() });
    signed_in(&mut host, NOW - 10);
    let before = host.secret.clone();
    let reply = call(&mut host, json!({ "interface": 1, "call": "refresh", "lang": "en" }));
    assert_eq!(reply["problem"]["kind"], "sign-in");
    assert_eq!(reply["agendas"], json!([]), "nothing replaced");
    assert_eq!(host.secret, before, "the sign-in kept is left for the person to renew");
    // The gateway refusing even the renewed token: the same.
    let mut host = stand_in(Platform { refuse_first: 5, ..Platform::default() });
    signed_in(&mut host, NOW + 3_600);
    let reply = call(&mut host, json!({ "interface": 1, "call": "refresh", "lang": "en" }));
    assert_eq!(reply["problem"]["kind"], "sign-in");
    // No sign-in kept at all.
    let mut host = stand_in(Platform::default());
    let reply = call(&mut host, json!({ "interface": 1, "call": "refresh", "lang": "en" }));
    assert_eq!(reply["problem"]["kind"], "sign-in");
    assert!(host.asked.is_empty());
}

#[test]
fn a_school_that_does_not_subscribe_is_said_refused() {
    let mut host = stand_in(Platform::default());
    signed_in(&mut host, NOW + 3_600);
    host.web = Box::new(|_| Answer::new(403, recorded("refusal-not-subscribed.json")));
    let reply = call(&mut host, json!({ "interface": 1, "call": "refresh", "lang": "en" }));
    assert_eq!(reply["problem"]["kind"], "refused");
    assert!(reply["problem"]["detail"].as_str().unwrap().contains("SKO_APP_NOT_SUBSCRIBED"));
}

#[test]
fn every_request_goes_to_a_host_the_manifest_names() {
    let mut host = stand_in(Platform::default());
    signed_in(&mut host, NOW - 10);
    call(&mut host, json!({ "interface": 1, "call": "refresh", "lang": "en" }));
    host.sign_in = Some(SignIn::Returned(format!("{RETURN}?code=OC-0000-recorded&state=sioul-{NOW}")));
    call(&mut host, json!({ "interface": 1, "call": "sign-in", "choice": "example" }));
    // The manifest's hosts, and the invented platform's (which only the tests know).
    let mut hosts = manifest_hosts();
    hosts.push("sso.example.org".into());
    assert!(!host.asked.is_empty());
    for request in &host.asked {
        let named = sioul_extension::url::https_host(&request.url).unwrap();
        assert!(hosts.contains(&named), "{named}");
    }
    // And every real platform's sign-in service is named in the manifest.
    let hosts = manifest_hosts();
    for platform in crate::platforms::PLATFORMS {
        assert!(hosts.contains(&sioul_extension::url::https_host(platform.discovery).unwrap()), "{}", platform.code);
    }
}
