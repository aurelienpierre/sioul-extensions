// SPDX-License-Identifier: GPL-3.0-or-later

//! The Planity Pro client: Firebase email/password sign-in, the Pro-token
//! exchange, the `securetoken` refresh, and the Firestore REST `:runQuery`
//! over the `vevents` collection, paginated. The data model and the exact
//! request/response shapes are validated in the publisher's design notes
//! (`api.md`, `extension-spec.md`), from a consented live capture; no real
//! value appears here. Every answer is read defensively: what does not read
//! is a problem for Sioul to say, never a panic.
//!
//! Planity is a Firebase web app on project `planity-production`. Its
//! Firebase API key is public (embedded in the web app), as a vendor secret
//! of a community client is: it is a constant in `lib.rs`.

use crate::{API_KEY, Fault};
use serde_json::{Map, Value, json};
use sioul_extension::{Host, Request, url};

const IDENTITY: &str = "https://identitytoolkit.googleapis.com/v1";
const PRO_TOKEN: &str = "https://product.api.euwest1.prod.planityapp.com/getProAuthToken";
const SECURETOKEN: &str = "https://securetoken.googleapis.com/v1/token";
/// The Firestore REST documents root of the salon's project.
const FIRESTORE: &str = "https://firestore.googleapis.com/v1/projects/planity-production/databases/(default)/documents";

/// Firestore's page size, and the most pages a window reads before it is a
/// problem (300 × 400 ≈ 120 000 appointments, far past any salon's window).
const LIMIT: i64 = 300;
const MAX_PAGES: usize = 400;

// ---------------------------------------------------------------- the sign-in dance

/// The email/password sign-in, the Pro-token exchange and the custom-token
/// sign-in (`api.md`): on success, the refresh token B to keep and the
/// business id read from token B's claims. Bad credentials are a sign-in
/// fault (the person signs in again). Every token is hidden from the log.
pub fn sign_in(host: &mut dyn Host, email: &str, password: &str) -> Result<(String, String), Fault> {
    // 1. Firebase email/password → id token A, uid, refresh token A.
    let url_pw = format!("{IDENTITY}/accounts:signInWithPassword?key={API_KEY}");
    let body = json!({ "email": email, "password": password, "returnSecureToken": true, "clientType": "CLIENT_TYPE_WEB" });
    let answer = host.web(&Request::post_json(&url_pw, &body).header("Accept", "application/json"))?;
    if answer.status != 200 {
        // EMAIL_NOT_FOUND / INVALID_PASSWORD / INVALID_LOGIN_CREDENTIALS: sign in again.
        if answer.status == 400 || bad_credentials(&answer.body) {
            return Err(Fault::SignIn(format!("the email or password was refused: {}", answer.status)));
        }
        return Err(Fault::Status(answer.status, "the password sign-in".into()));
    }
    let signed = answer.json().ok_or_else(|| Fault::Changed("the password sign-in does not read".into()))?;
    let id_a = non_empty(&signed, "idToken").ok_or_else(|| Fault::Changed("no id token in the password sign-in".into()))?;
    let uid = non_empty(&signed, "localId").ok_or_else(|| Fault::Changed("no uid in the password sign-in".into()))?;
    let refresh_a = non_empty(&signed, "refreshToken");
    hide(host, &[Some(&id_a), refresh_a.as_deref()]);

    // 2. Planity's Pro-token exchange → a Firebase custom token with the Pro claims.
    let body = json!({ "uid": uid, "token": id_a, "source": "web", "idToken": id_a, "isBusinessSharded": true, "isUserSharded": true });
    let answer = host.web(&Request::post_json(PRO_TOKEN, &body).header("Accept", "application/json"))?;
    if answer.status != 200 {
        return Err(Fault::Status(answer.status, "the Pro-token exchange".into()));
    }
    let pro = answer.json().ok_or_else(|| Fault::Changed("the Pro-token exchange does not read".into()))?;
    let custom = non_empty(&pro, "token").ok_or_else(|| Fault::Changed("no custom token in the Pro-token exchange".into()))?;
    hide(host, &[Some(&custom)]);

    // 3. Firebase custom-token sign-in → id token B (Pro claims) and refresh token B.
    let url_custom = format!("{IDENTITY}/accounts:signInWithCustomToken?key={API_KEY}");
    let body = json!({ "token": custom, "returnSecureToken": true });
    let answer = host.web(&Request::post_json(&url_custom, &body).header("Accept", "application/json"))?;
    if answer.status != 200 {
        return Err(Fault::Status(answer.status, "the custom-token sign-in".into()));
    }
    let signed = answer.json().ok_or_else(|| Fault::Changed("the custom-token sign-in does not read".into()))?;
    let id_b = non_empty(&signed, "idToken").ok_or_else(|| Fault::Changed("no id token in the custom-token sign-in".into()))?;
    let refresh_b = non_empty(&signed, "refreshToken").ok_or_else(|| Fault::Changed("no refresh token in the custom-token sign-in".into()))?;
    hide(host, &[Some(&id_b), Some(&refresh_b)]);

    // 4. The business id from token B's claims.
    let business = business_id(&id_b).ok_or_else(|| Fault::Changed("token B names no Pro business".into()))?;
    Ok((refresh_b, business))
}

/// Whether a sign-in answer names a bad-credentials error.
fn bad_credentials(body: &str) -> bool {
    ["EMAIL_NOT_FOUND", "INVALID_PASSWORD", "INVALID_LOGIN_CREDENTIALS"].iter().any(|code| body.contains(code))
}

/// The business id from an id token's claims: the one claim whose key is a
/// Firebase push id (`^-[A-Za-z0-9_-]{19}$`) with an integer value (a role),
/// and only when `plPro` is true (`api.md`). Read, never verified — the
/// server verifies the token; this only names the business to read.
pub fn business_id(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let claims: Value = serde_json::from_slice(&url::base64url(payload)?).ok()?;
    if claims.get("plPro") != Some(&Value::Bool(true)) {
        return None;
    }
    claims.as_object()?.iter().find(|(key, value)| is_push_id(key) && is_integer(value)).map(|(key, _)| key.clone())
}

/// A Firebase push id: `-` then nineteen of letters, digits, `-` or `_`.
fn is_push_id(key: &str) -> bool {
    let bytes = key.as_bytes();
    bytes.len() == 20 && bytes[0] == b'-' && bytes[1..].iter().all(|b| b.is_ascii_alphanumeric() || *b == b'-' || *b == b'_')
}

fn is_integer(value: &Value) -> bool {
    value.is_i64() || value.is_u64()
}

// ---------------------------------------------------------------- the refresh

/// A fresh id token from the refresh token, through `securetoken` (`api.md`):
/// the id token to sign Firestore with, and a rotated refresh token when one
/// comes. A refusal is the end of the sign-in.
pub fn mint(host: &mut dyn Host, refresh: &str) -> Result<(String, Option<String>), Fault> {
    let url_token = format!("{SECURETOKEN}?key={API_KEY}");
    let answer = host.web(&Request::post_form(&url_token, &[("grant_type", "refresh_token"), ("refresh_token", refresh)]).header("Accept", "application/json"))?;
    match answer.status {
        200 => {}
        400 | 401 | 403 => return Err(Fault::SignIn(format!("the token refresh was refused: {}", answer.status))),
        status => return Err(Fault::Status(status, "the token refresh".into())),
    }
    let body = answer.json().ok_or_else(|| Fault::Changed("the token refresh does not read".into()))?;
    // A still-rejected refresh may answer 200 with an error body; treat it as a sign-in end.
    if body.get("error").is_some() || ["TOKEN_EXPIRED", "INVALID_REFRESH_TOKEN", "USER_DISABLED"].iter().any(|c| answer.body.contains(c)) {
        return Err(Fault::SignIn("the refresh token is no longer good".into()));
    }
    let id_token = non_empty(&body, "access_token").ok_or_else(|| Fault::Changed("no access token in the token refresh".into()))?;
    let rotated = non_empty(&body, "refresh_token");
    Ok((id_token, rotated))
}

// ---------------------------------------------------------------- the appointments

/// One `vevents` document: its own id (the last path segment, a stable UID),
/// its full resource name (for the pagination cursor), and its fields
/// flattened to plain JSON ([`plain_fields`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    pub id: String,
    pub name: String,
    pub fields: Value,
}

/// The salon's appointments from `from` to `until` (local wall-clock strings
/// `"YYYY-MM-DD HH:MM"`), read over Firestore REST `:runQuery`, paginated.
/// Only `s >= from` is filtered server-side (one range field, so no composite
/// index is needed); ordered ascending by `s` then the document name, the
/// read stops at the first document past `until`.
pub fn read_vevents(host: &mut dyn Host, business: &str, id_token: &str, from: &str, until: &str) -> Result<Vec<Document>, Fault> {
    let url_query = format!("{FIRESTORE}/businesses/{business}:runQuery");
    let authorization = format!("Bearer {id_token}");
    let mut out = Vec::new();
    let mut cursor: Option<(String, String)> = None;
    for _ in 0..MAX_PAGES {
        let body = query_body(from, cursor.as_ref());
        let answer = host.web(&Request::post_json(&url_query, &body).header("Authorization", &authorization).header("Accept", "application/json"))?;
        match answer.status {
            200 => {}
            401 => return Err(Fault::SignIn("Firestore refused the token".into())),
            403 => return Err(Fault::Refused("Firestore refused the business".into())),
            status => return Err(Fault::Status(status, "the appointments query".into())),
        }
        let page = answer.json().ok_or_else(|| Fault::Changed("an appointments page does not read".into()))?;
        let elements = page.as_array().ok_or_else(|| Fault::Changed("an appointments page is not a list".into()))?;
        let mut documents = 0;
        let mut last: Option<(String, String)> = None;
        for element in elements {
            // A page element is either a {document} or a read-time marker; skip the marker.
            let Some(document) = element.get("document") else { continue };
            documents += 1;
            let name = document.get("name").and_then(Value::as_str).unwrap_or("").to_string();
            let fields = plain_fields(document.get("fields"));
            let start = fields.get("s").and_then(Value::as_str).unwrap_or("").to_string();
            last = Some((start.clone(), name.clone()));
            // Ascending by `s`: the first past `until` ends the read.
            if start.as_str() > until {
                return Ok(out);
            }
            let id = name.rsplit('/').next().unwrap_or("").to_string();
            out.push(Document { id, name, fields });
        }
        // A page shorter than the limit (or with no document) is the last.
        if documents < usize::try_from(LIMIT).unwrap_or(usize::MAX) {
            return Ok(out);
        }
        match last {
            Some(next) => cursor = Some(next),
            None => return Ok(out),
        }
    }
    Err(Fault::Changed("too many appointment pages".into()))
}

// ---------------------------------------------------------------- the official feed

/// The host and the path of Planity's official iCalendar export (`api.md`).
pub const FEED_HOST: &str = "product.api.euwest1.prod.planityapp.com";
pub const FEED_PATH: &str = "/getCalendarIcalEvents";

/// A calendar's export link, checked: `https`, Planity's own host (no port,
/// no user), its export path, and a query naming the calendar, the business
/// and the key. The key makes the link a capability: it is hidden from the
/// log and kept only in the keyring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// The whole address, as pasted (trimmed, any `#…` left out).
    pub address: String,
    pub calendar: String,
    pub business: String,
    pub key: String,
}

impl Link {
    /// The link checked; what is wrong with it, otherwise (for the log, never the link itself).
    pub fn read(text: &str) -> Result<Self, &'static str> {
        let address = text.trim().split('#').next().unwrap_or("").to_string();
        let rest = address.strip_prefix("https://").ok_or("the web address is not https")?;
        let authority = rest.split(['/', '?']).next().unwrap_or("");
        if !authority.eq_ignore_ascii_case(FEED_HOST) {
            return Err("the web address is not Planity's calendar export");
        }
        let path = rest[authority.len()..].split('?').next().unwrap_or("");
        if path != FEED_PATH {
            return Err("the web address is not a calendar export");
        }
        let value = |name: &str| url::query_value(&address, name).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let calendar = value("calendarId").ok_or("the web address names no calendar")?;
        let business = value("businessId").ok_or("the web address names no business")?;
        let key = value("key").ok_or("the web address has no key")?;
        Ok(Self { address, calendar, business, key })
    }
}

/// The calendar's feed, as text. A refusal of the key (401, 403) is the end
/// of the link (the person pastes it again); anything else, a feed not read.
pub fn read_feed(host: &mut dyn Host, link: &Link) -> Result<String, Fault> {
    let answer = host.web(&Request::get(&link.address).header("Accept", "text/calendar"))?;
    match answer.status {
        200 => Ok(answer.body),
        401 | 403 => Err(Fault::SignIn(format!("the calendar's web address was refused: {}", answer.status))),
        status => Err(Fault::Status(status, "the calendar feed".into())),
    }
}

/// The `structuredQuery` body: the `vevents` collection, `s >= from`, ordered
/// by `s` then `__name__` (both ascending), paged after a cursor.
fn query_body(from: &str, after: Option<&(String, String)>) -> Value {
    let mut query = json!({
        "structuredQuery": {
            "from": [{ "collectionId": "vevents" }],
            "where": { "fieldFilter": { "field": { "fieldPath": "s" }, "op": "GREATER_THAN_OR_EQUAL", "value": { "stringValue": from } } },
            "orderBy": [
                { "field": { "fieldPath": "s" }, "direction": "ASCENDING" },
                { "field": { "fieldPath": "__name__" }, "direction": "ASCENDING" }
            ],
            "limit": LIMIT
        }
    });
    if let Some((start, name)) = after {
        // Start strictly after the last document read (`before: false`).
        query["structuredQuery"]["startAt"] = json!({
            "values": [{ "stringValue": start }, { "referenceValue": name }],
            "before": false
        });
    }
    query
}

/// A Firestore document's `fields` map flattened to plain JSON.
fn plain_fields(fields: Option<&Value>) -> Value {
    let mut out = Map::new();
    if let Some(object) = fields.and_then(Value::as_object) {
        for (name, value) in object {
            out.insert(name.clone(), plain(value));
        }
    }
    Value::Object(out)
}

/// A Firestore typed value flattened to plain JSON (Firestore REST `Value`):
/// a string as a string, an integer (a decimal string) as a number, a double
/// as it is, a boolean, a null, a timestamp as its string, a map as an object
/// of its flattened fields, an array as a list; anything else null.
pub fn plain(value: &Value) -> Value {
    if let Some(string) = value.get("stringValue").and_then(Value::as_str) {
        return Value::String(string.to_string());
    }
    if let Some(integer) = value.get("integerValue") {
        if let Some(text) = integer.as_str() {
            return text.parse::<i64>().map(Value::from).unwrap_or(Value::String(text.to_string()));
        }
        return integer.clone();
    }
    if let Some(double) = value.get("doubleValue") {
        return double.clone();
    }
    if let Some(boolean) = value.get("booleanValue") {
        return boolean.clone();
    }
    if value.get("nullValue").is_some() {
        return Value::Null;
    }
    if let Some(timestamp) = value.get("timestampValue") {
        return timestamp.clone();
    }
    if let Some(map) = value.get("mapValue") {
        return plain_fields(map.get("fields"));
    }
    if let Some(array) = value.get("arrayValue") {
        let items = array.get("values").and_then(Value::as_array).map(|all| all.iter().map(plain).collect()).unwrap_or_default();
        return Value::Array(items);
    }
    Value::Null
}

/// A string field's value, non-empty.
fn non_empty(body: &Value, field: &str) -> Option<String> {
    body.get(field).and_then(Value::as_str).filter(|value| !value.is_empty()).map(str::to_string)
}

/// Hides each token that is present from the log.
fn hide(host: &mut dyn Host, values: &[Option<&str>]) {
    let present: Vec<&str> = values.iter().copied().flatten().collect();
    if !present.is_empty() {
        host.hide(&present);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firestore_values_flatten() {
        let fields = json!({
            "s": { "stringValue": "2026-11-20 14:00" },
            "d": { "integerValue": "30" },
            "p": { "integerValue": "4500" },
            "ad": { "booleanValue": true },
            "cu": { "mapValue": { "fields": { "id": { "stringValue": "C1" }, "name": { "stringValue": "Marie Exemple" } } } },
            "dat": { "timestampValue": "2026-11-18T10:00:00Z" }
        });
        let flat = plain_fields(Some(&fields));
        assert_eq!(flat["s"], "2026-11-20 14:00");
        assert_eq!(flat["d"], 30);
        assert_eq!(flat["p"], 4500);
        assert_eq!(flat["ad"], true);
        assert_eq!(flat["cu"]["name"], "Marie Exemple");
        assert_eq!(flat["dat"], "2026-11-18T10:00:00Z");
    }

    #[test]
    fn a_business_id_is_read_from_token_b() {
        // header.payload.signature, the payload base64url of the claims; no real token.
        let payload = url::base64url_encode(br#"{"plPro":true,"iss":"https://securetoken.google.com/planity-production","-Binvented0000000000":1,"iat":1763800000,"exp":1763803600}"#);
        let token = format!("e30.{payload}.sig");
        assert_eq!(business_id(&token).as_deref(), Some("-Binvented0000000000"));
        // No plPro claim: no business.
        let payload = url::base64url_encode(br#"{"-Binvented0000000000":1}"#);
        assert_eq!(business_id(&format!("e30.{payload}.sig")), None);
        // A key that is not a push id, or a non-integer value: no business.
        let payload = url::base64url_encode(br#"{"plPro":true,"iss":"x","aud":"planity-production"}"#);
        assert_eq!(business_id(&format!("e30.{payload}.sig")), None);
        assert!(is_push_id("-Binvented0000000000"));
        assert!(!is_push_id("-Bshort"));
        assert!(!is_push_id("Binvented00000000000"));
    }

    #[test]
    fn a_calendar_link_is_checked() {
        let good = "https://product.api.euwest1.prod.planityapp.com/getCalendarIcalEvents?calendarId=-Cinvented0000000000&businessId=-Binvented0000000000&key=KEYinvented0000";
        let link = Link::read(&format!("  {good}  ")).unwrap();
        assert_eq!((link.address.as_str(), link.calendar.as_str(), link.business.as_str(), link.key.as_str()), (good, "-Cinvented0000000000", "-Binvented0000000000", "KEYinvented0000"));
        assert_eq!(Link::read(&format!("{good}#here")).unwrap().address, good, "a fragment is left out");
        for (wrong, why) in [
            (good.replacen("https://", "http://", 1), "not https"),
            (good.replacen("product.api.euwest1.prod.planityapp.com", "product.api.euwest1.prod.planityapp.com.example.org", 1), "another host"),
            (good.replacen("product.api.euwest1.prod.planityapp.com", "product.api.euwest1.prod.planityapp.com:8443", 1), "a port"),
            (good.replacen("https://", "https://someone@", 1), "a user"),
            (good.replacen("/getCalendarIcalEvents", "/getCalendarIcalEvents/x", 1), "another path"),
            (good.replacen("/getCalendarIcalEvents", "/getProAuthToken", 1), "another path"),
            (good.replacen("&key=KEYinvented0000", "", 1), "no key"),
            (good.replacen("key=KEYinvented0000", "key=", 1), "an empty key"),
            (good.replacen("calendarId=-Cinvented0000000000&", "", 1), "no calendar"),
            (good.replacen("businessId=-Binvented0000000000&", "", 1), "no business"),
            (String::new(), "nothing"),
        ] {
            assert!(Link::read(&wrong).is_err(), "{why}: {wrong}");
        }
    }

    #[test]
    fn the_query_body_pages_after_a_cursor() {
        let first = query_body("2026-09-23 00:00", None);
        assert_eq!(first["structuredQuery"]["where"]["fieldFilter"]["op"], "GREATER_THAN_OR_EQUAL");
        assert_eq!(first["structuredQuery"]["where"]["fieldFilter"]["value"]["stringValue"], "2026-09-23 00:00");
        assert_eq!(first["structuredQuery"]["limit"], LIMIT);
        assert_eq!(first["structuredQuery"].get("startAt"), None);
        let next = query_body("2026-09-23 00:00", Some(&("2026-10-01 09:00".into(), "projects/planity-production/databases/(default)/documents/businesses/-Binvented0000000000/vevents/SQ1".into())));
        assert_eq!(next["structuredQuery"]["startAt"]["before"], false);
        assert_eq!(next["structuredQuery"]["startAt"]["values"][0]["stringValue"], "2026-10-01 09:00");
        assert!(next["structuredQuery"]["startAt"]["values"][1]["referenceValue"].as_str().unwrap().ends_with("/vevents/SQ1"));
    }
}
