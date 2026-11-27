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
use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_json::{Value, json};
use sioul_extension::{Answer, Host, Request, url};

const IDENTITY: &str = "https://identitytoolkit.googleapis.com/v1";
const PRO_TOKEN: &str = "https://product.api.euwest1.prod.planityapp.com/getProAuthToken";
const SECURETOKEN: &str = "https://securetoken.googleapis.com/v1/token";
/// The Firestore REST documents root of the salon's project.
const FIRESTORE: &str = "https://firestore.googleapis.com/v1/projects/planity-production/databases/(default)/documents";

/// Firestore's page size (300 documents weigh about 400 KB, measured on a
/// salon's window on 27 November 2026, far under the 4 MiB Sioul lets an
/// answer weigh).
const LIMIT: i64 = 300;
/// The most pages a read may take. Sioul gives a call 100 web requests
/// (docs/extensions.md, "Limits of a call"): a refresh asks one renewal,
/// a piece of the past, the window and the feed, 1 + 8 + 50 + 1.
pub const PIECE_PAGES: usize = 8;
pub const WINDOW_PAGES: usize = 50;

// ---------------------------------------------------------------- each step, named

/// The steps, as each problem and each line of the log names them: what was
/// asked of which service.
pub const PASSWORD_STEP: &str = "the password sign-in (identitytoolkit)";
pub const PRO_STEP: &str = "the Pro-token exchange (getProAuthToken)";
pub const CUSTOM_STEP: &str = "the custom-token sign-in (identitytoolkit)";
pub const RENEWAL_STEP: &str = "the token's renewal (securetoken)";
pub const FIRESTORE_STEP: &str = "the appointments' read (Firestore)";
pub const FEED_STEP: &str = "the calendar feed (getCalendarIcalEvents)";

/// The service's own error code in an answer, when it gives one: Firebase's
/// `error.message` when it is a code (`INVALID_LOGIN_CREDENTIALS`, the part
/// before " : " of `TOO_MANY_ATTEMPTS_TRY_LATER : …`), Google's reason for a
/// key refused (`API_KEY_INVALID`, from `error.details[].reason` or
/// `error.errors[].reason` `keyInvalid`), an OAuth-style `error`
/// (`invalid_grant`), or Google's `error.status` (`PERMISSION_DENIED`). Only
/// a code is ever taken (capitals, digits and `_`, 64 at most), never the
/// service's prose, which could repeat what was sent.
pub fn service_code(body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    let error = value.get("error")?;
    let code = |text: &str| {
        let head = text.split(" : ").next().unwrap_or("").trim().to_ascii_uppercase();
        (!head.is_empty() && head.len() <= 64 && head.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')).then_some(head)
    };
    if let Some(text) = error.as_str() {
        return code(text);
    }
    let message = error.get("message").and_then(Value::as_str).unwrap_or("");
    if let Some(found) = code(message) {
        return Some(found);
    }
    let reasons = ["details", "errors"].iter().flat_map(|list| error.get(*list).and_then(Value::as_array).cloned().unwrap_or_default()).filter_map(|item| item.get("reason").and_then(Value::as_str).map(str::to_string)).collect::<Vec<_>>();
    if let Some(found) = reasons.iter().find_map(|reason| if reason == "keyInvalid" { Some("API_KEY_INVALID".to_string()) } else { code(reason) }) {
        return Some(found);
    }
    if message.starts_with("API key not valid") {
        return Some("API_KEY_INVALID".into());
    }
    error.get("status").and_then(Value::as_str).and_then(code)
}

/// What an answer that is not 200 means, for `step`: the step, the HTTP
/// status and the service's own code, then the cause in words, as a fault of
/// the kind the person can act on. `credentials`: the password step, where a
/// 400 without a known code is still a refused sign-in.
pub fn answered(step: &str, answer: &Answer, credentials: bool) -> Fault {
    let code = service_code(&answer.body);
    let said = match &code {
        Some(code) => format!("{step} answered HTTP {} with {code}", answer.status),
        None => format!("{step} answered HTTP {}", answer.status),
    };
    match code.as_deref() {
        Some("INVALID_LOGIN_CREDENTIALS" | "INVALID_PASSWORD" | "EMAIL_NOT_FOUND") => Fault::SignIn(format!("{said}: the email or the password was refused")),
        Some("INVALID_EMAIL") => Fault::SignIn(format!("{said}: the email is not an address it reads")),
        Some("MISSING_EMAIL" | "MISSING_PASSWORD") => Fault::SignIn(format!("{said}: a field reached it empty")),
        Some("TOO_MANY_ATTEMPTS_TRY_LATER") => Fault::Refused(format!("{said}: too many tries; the service refuses them for a while")),
        Some("USER_DISABLED") => Fault::Refused(format!("{said}: the account is disabled")),
        Some("TOKEN_EXPIRED" | "INVALID_REFRESH_TOKEN" | "INVALID_GRANT" | "USER_NOT_FOUND" | "MISSING_REFRESH_TOKEN" | "INVALID_ID_TOKEN" | "CREDENTIAL_TOO_OLD_LOGIN_AGAIN" | "UNAUTHENTICATED") => Fault::SignIn(format!("{said}: the sign-in kept here has ended")),
        Some(key) if key.starts_with("API_KEY") => Fault::Changed(format!("{said}: the service refused the key of Planity's web app; an update of the extension is needed")),
        Some("PERMISSION_DENIED") => Fault::Refused(format!("{said}: this sign-in may not read it")),
        _ => match answer.status {
            400 if credentials => Fault::SignIn(format!("{said}: the sign-in was refused")),
            401 => Fault::SignIn(format!("{said}: the sign-in was refused")),
            403 => Fault::Refused(format!("{said}: the service refused")),
            429 => Fault::Unreachable(format!("{said}: too many requests; it tries again later")),
            500.. => Fault::Unreachable(format!("{said}: the service did not answer as it should; it tries again later")),
            _ => Fault::Changed(format!("{said}: an answer the extension does not expect")),
        },
    }
}

/// A request of `step`, made by Sioul; its refusal named with the step
/// (the network, the time, a host Sioul would not reach).
fn ask(host: &mut dyn Host, step: &str, request: &Request) -> Result<Answer, Fault> {
    host.web(request).map_err(|refused| Fault::refused_at(step, &refused))
}

/// A step's line in the log: the step, then what came of it.
fn said(host: &mut dyn Host, step: &str, what: &str) {
    host.log(&format!("{step}: {what}"));
}

// ---------------------------------------------------------------- the sign-in dance

/// The email/password sign-in, the Pro-token exchange and the custom-token
/// sign-in (`api.md`): on success, the refresh token B to keep and the
/// business id read from token B's claims. Bad credentials are a sign-in
/// fault (the person signs in again). Every token is hidden from the log.
pub fn sign_in(host: &mut dyn Host, email: &str, password: &str) -> Result<(String, String), Fault> {
    // 1. Firebase email/password → id token A, uid, refresh token A.
    let url_pw = format!("{IDENTITY}/accounts:signInWithPassword?key={API_KEY}");
    let body = json!({ "email": email, "password": password, "returnSecureToken": true, "clientType": "CLIENT_TYPE_WEB" });
    let answer = ask(host, PASSWORD_STEP, &Request::post_json(&url_pw, &body).header("Accept", "application/json"))?;
    if answer.status != 200 {
        // INVALID_LOGIN_CREDENTIALS, TOO_MANY_ATTEMPTS_TRY_LATER, USER_DISABLED, a key refused…: named.
        return Err(answered(PASSWORD_STEP, &answer, true));
    }
    let signed = answer.json().ok_or_else(|| Fault::Changed(format!("{PASSWORD_STEP} answered HTTP 200 with what is not JSON")))?;
    let id_a = non_empty(&signed, "idToken").ok_or_else(|| Fault::Changed(format!("{PASSWORD_STEP} answered HTTP 200 without an id token")))?;
    let uid = non_empty(&signed, "localId").ok_or_else(|| Fault::Changed(format!("{PASSWORD_STEP} answered HTTP 200 without an account id")))?;
    let refresh_a = non_empty(&signed, "refreshToken");
    hide(host, &[Some(&id_a), refresh_a.as_deref(), Some(&uid)]);
    said(host, PASSWORD_STEP, "signed in (step 1 of 3)");

    // 2. Planity's Pro-token exchange → a Firebase custom token with the Pro claims.
    let body = json!({ "uid": uid, "token": id_a, "source": "web", "idToken": id_a, "isBusinessSharded": true, "isUserSharded": true });
    let answer = ask(host, PRO_STEP, &Request::post_json(PRO_TOKEN, &body).header("Accept", "application/json"))?;
    if answer.status != 200 {
        return Err(answered(PRO_STEP, &answer, false));
    }
    let pro = answer.json().ok_or_else(|| Fault::Changed(format!("{PRO_STEP} answered HTTP 200 with what is not JSON")))?;
    let custom = non_empty(&pro, "token").ok_or_else(|| Fault::Changed(format!("{PRO_STEP} answered HTTP 200 without a custom token: the account may not be a Planity Pro one")))?;
    hide(host, &[Some(&custom)]);
    said(host, PRO_STEP, "a Pro token received (step 2 of 3)");

    // 3. Firebase custom-token sign-in → id token B (Pro claims) and refresh token B.
    let url_custom = format!("{IDENTITY}/accounts:signInWithCustomToken?key={API_KEY}");
    let body = json!({ "token": custom, "returnSecureToken": true });
    let answer = ask(host, CUSTOM_STEP, &Request::post_json(&url_custom, &body).header("Accept", "application/json"))?;
    if answer.status != 200 {
        return Err(answered(CUSTOM_STEP, &answer, false));
    }
    let signed = answer.json().ok_or_else(|| Fault::Changed(format!("{CUSTOM_STEP} answered HTTP 200 with what is not JSON")))?;
    let id_b = non_empty(&signed, "idToken").ok_or_else(|| Fault::Changed(format!("{CUSTOM_STEP} answered HTTP 200 without an id token")))?;
    let refresh_b = non_empty(&signed, "refreshToken").ok_or_else(|| Fault::Changed(format!("{CUSTOM_STEP} answered HTTP 200 without a refresh token")))?;
    hide(host, &[Some(&id_b), Some(&refresh_b)]);

    // 4. The business id from token B's claims.
    let business = business_id(&id_b).ok_or_else(|| Fault::Changed(format!("{CUSTOM_STEP}: its token names no Planity Pro business (no plPro claim, or no business id in it)")))?;
    hide(host, &[Some(&business)]);
    said(host, CUSTOM_STEP, "signed in to the salon's business (step 3 of 3)");
    Ok((refresh_b, business))
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
    let answer = ask(host, RENEWAL_STEP, &Request::post_form(&url_token, &[("grant_type", "refresh_token"), ("refresh_token", refresh)]).header("Accept", "application/json"))?;
    match answer.status {
        200 => {}
        // TOKEN_EXPIRED, INVALID_REFRESH_TOKEN, USER_DISABLED, a key refused…: named; a 400 or a 403 without a code, the sign-in's end.
        400 | 403 if service_code(&answer.body).is_none() => return Err(Fault::SignIn(format!("{RENEWAL_STEP} answered HTTP {}: the sign-in kept here has ended", answer.status))),
        _ => return Err(answered(RENEWAL_STEP, &answer, false)),
    }
    let body = answer.json().ok_or_else(|| Fault::Changed(format!("{RENEWAL_STEP} answered HTTP 200 with what is not JSON")))?;
    // A still-rejected refresh may answer 200 with an error body: the sign-in's end, its code named.
    if body.get("error").is_some() {
        let code = service_code(&answer.body).map(|c| format!(" with {c}")).unwrap_or_default();
        return Err(Fault::SignIn(format!("{RENEWAL_STEP} answered HTTP 200{code}: the sign-in kept here has ended")));
    }
    let id_token = non_empty(&body, "access_token").ok_or_else(|| Fault::Changed(format!("{RENEWAL_STEP} answered HTTP 200 without an access token")))?;
    let rotated = non_empty(&body, "refresh_token");
    said(host, RENEWAL_STEP, if rotated.is_some() { "a fresh token received, and a new refresh token" } else { "a fresh token received" });
    Ok((id_token, rotated))
}

// ---------------------------------------------------------------- the appointments

/// One `vevents` document: its own id (the last path segment, a stable UID),
/// its full resource name (for the pagination cursor), and the fields the
/// extension reads ([`Fields`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    pub id: String,
    pub name: String,
    pub fields: Fields,
}

/// A Firestore typed value (Firestore REST `Value`), read straight into its
/// kind: a string, an integer (a decimal string), a double, a boolean, a
/// timestamp, a map of a client's fields; an array is only seen to be there;
/// a null, or a kind the extension does not read (a reference, bytes, a
/// point), is no value. Every other key of an answer is skipped as it is
/// read, never kept: a document holds many fields the extension does not
/// use, and a page of a whole past holds three hundred documents (read in
/// Sioul's interpreter, a tree of every value cost a million steps a
/// document, 27 November 2026).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Typed {
    pub string_value: Option<String>,
    pub integer_value: Option<Value>,
    pub double_value: Option<Value>,
    pub boolean_value: Option<bool>,
    pub timestamp_value: Option<String>,
    pub map_value: Option<Box<TypedMap>>,
    /// An array, seen to be there, its items skipped.
    #[serde(deserialize_with = "seen")]
    pub array_value: Option<()>,
}

/// A value seen to be there, whatever it holds, skipped as it is read.
fn seen<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<()>, D::Error> {
    <IgnoredAny as serde::Deserialize>::deserialize(deserializer).map(|_| Some(()))
}

/// A map's fields, as a client's are read (`cu`: `id`, `name`, `phone`, `email`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct TypedMap {
    pub fields: ClientFields,
}

/// The fields of a client (`cu`); a block's `{d: true}` holds none of them.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ClientFields {
    pub id: Option<Typed>,
    pub name: Option<Typed>,
    pub phone: Option<Typed>,
    pub email: Option<Typed>,
}

/// The fields of a `vevents` document the extension reads (`api.md`): its
/// start, minutes, all-day mark, client, booking key, calendars, cancelled
/// mark, note, service, price, update time.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Fields {
    pub s: Option<Typed>,
    pub d: Option<Typed>,
    pub ad: Option<Typed>,
    pub cu: Option<Typed>,
    pub sq: Option<Typed>,
    pub ca: Option<Typed>,
    pub sca: Option<Typed>,
    pub dat: Option<Typed>,
    pub t: Option<Typed>,
    pub se: Option<Typed>,
    pub p: Option<Typed>,
    pub uat: Option<Typed>,
}

impl Typed {
    /// Its text: a string's, or a timestamp's as written.
    pub fn text(&self) -> Option<&str> {
        self.string_value.as_deref().or(self.timestamp_value.as_deref())
    }

    /// Its integer: an integer's (a decimal string, or a number), or a double that is a whole number written as one.
    pub fn integer(&self) -> Option<i64> {
        match &self.integer_value {
            Some(Value::String(text)) => text.parse().ok(),
            Some(number) => number.as_i64(),
            None => self.double_value.as_ref().and_then(Value::as_i64),
        }
    }

    pub fn boolean(&self) -> Option<bool> {
        self.boolean_value
    }

    /// Whether it holds a value of any kind it reads (a null, none).
    pub fn present(&self) -> bool {
        self.string_value.is_some() || self.integer_value.is_some() || self.double_value.is_some() || self.boolean_value.is_some() || self.timestamp_value.is_some() || self.map_value.is_some() || self.array_value.is_some()
    }

    /// Its scalar as plain JSON: an integer as a number (its decimal string
    /// when it does not parse), a double, a boolean, a string or a
    /// timestamp's text; anything else null.
    pub fn plain(&self) -> Value {
        if let Some(text) = &self.string_value {
            return Value::String(text.clone());
        }
        if let Some(integer) = &self.integer_value {
            return match integer {
                Value::String(text) => text.parse::<i64>().map(Value::from).unwrap_or_else(|_| Value::String(text.clone())),
                other => other.clone(),
            };
        }
        if let Some(double) = &self.double_value {
            return double.clone();
        }
        if let Some(boolean) = self.boolean_value {
            return Value::Bool(boolean);
        }
        self.timestamp_value.clone().map(Value::String).unwrap_or(Value::Null)
    }

    /// A map's client fields, when it is a map.
    pub fn client(&self) -> Option<&ClientFields> {
        self.map_value.as_deref().map(|map| &map.fields)
    }
}

/// An element of a `:runQuery` page: a document, or a read-time marker (none).
#[derive(Deserialize)]
struct Element {
    #[serde(default)]
    document: Option<RawDocument>,
}

#[derive(Deserialize)]
struct RawDocument {
    #[serde(default)]
    name: String,
    #[serde(default)]
    fields: Fields,
}

/// What a read of the appointment documents gave: each document as the
/// reader's `take` made it (none: left out), how many documents were read,
/// whether the read reached `until` (`complete`), and when it stopped short,
/// the local day the next read begins at (`resume`, `"YYYY-MM-DD 00:00"`):
/// the day of the first document not read.
#[derive(Debug)]
pub struct Read<T> {
    pub items: Vec<T>,
    pub documents: usize,
    pub complete: bool,
    pub resume: Option<String>,
}

/// The salon's appointments from `from` to `until` (local wall-clock strings
/// `"YYYY-MM-DD HH:MM"`), read over Firestore REST `:runQuery`, paginated,
/// `most_pages` pages at most. Only `s >= from` is filtered server-side (one
/// range field, so no composite index is needed); ordered ascending by `s`
/// then the document name, the read stops at the first document past
/// `until`. A `piece` of the past stops sooner: once it holds that many
/// documents, at the first document of another day, so that it holds whole
/// days and the next piece begins with that day (`Read::resume`); past its
/// pages too, at the day of the last document read. Each document goes
/// through `take` as soon as its page is read, so that only what the reader
/// keeps of it stays in memory.
#[allow(clippy::too_many_arguments)]
pub fn read_vevents<T>(host: &mut dyn Host, business: &str, id_token: &str, from: &str, until: &str, most_pages: usize, piece: Option<usize>, mut take: impl FnMut(Document) -> Option<T>) -> Result<Read<T>, Fault> {
    let url_query = format!("{FIRESTORE}/businesses/{business}:runQuery");
    let authorization = format!("Bearer {id_token}");
    let mut out = Read { items: Vec::new(), documents: 0, complete: true, resume: None };
    let mut cursor: Option<(String, String)> = None;
    // The local day of the last document read, `"YYYY-MM-DD"`.
    let mut day: Option<String> = None;
    for _ in 0..most_pages {
        let body = query_body(from, cursor.as_ref());
        let answer = ask(host, FIRESTORE_STEP, &Request::post_json(&url_query, &body).header("Authorization", &authorization).header("Accept", "application/json"))?;
        match answer.status {
            200 => {}
            // UNAUTHENTICATED (the token refused), PERMISSION_DENIED (the business not readable)…: named.
            _ => return Err(answered(FIRESTORE_STEP, &answer, false)),
        }
        // Read straight into the fields the extension uses, every other key skipped (`Typed`).
        let elements: Vec<Element> = serde_json::from_str(&answer.body).map_err(|_| match serde_json::from_str::<IgnoredAny>(&answer.body) {
            Ok(_) => Fault::Changed(format!("{FIRESTORE_STEP} answered HTTP 200 with what is not a list of documents")),
            Err(_) => Fault::Changed(format!("{FIRESTORE_STEP} answered HTTP 200 with what is not JSON")),
        })?;
        drop(answer);
        let mut documents = 0;
        let mut last: Option<(String, String)> = None;
        for element in elements {
            // A page element is either a {document} or a read-time marker; skip the marker.
            let Some(RawDocument { name, fields }) = element.document else { continue };
            documents += 1;
            let start = fields.s.as_ref().and_then(Typed::text).unwrap_or("").to_string();
            last = Some((start.clone(), name.clone()));
            // Ascending by `s`: the first past `until` ends the read.
            if start.as_str() > until {
                return Ok(out);
            }
            // A piece full: it ends with the day before this document's.
            let this_day = start.get(..10).filter(|d| d.len() == 10 && d.as_bytes()[4] == b'-');
            if piece.is_some_and(|most| out.documents >= most) && this_day.is_some() && day.as_deref().is_some_and(|before| Some(before) != this_day) {
                out.complete = false;
                out.resume = this_day.map(|d| format!("{d} 00:00"));
                return Ok(out);
            }
            if let Some(this_day) = this_day {
                day = Some(this_day.to_string());
            }
            let id = name.rsplit('/').next().unwrap_or("").to_string();
            out.documents += 1;
            if let Some(item) = take(Document { id, name, fields }) {
                out.items.push(item);
            }
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
    out.complete = false;
    out.resume = day.map(|d| format!("{d} 00:00"));
    Ok(out)
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
        let rest = address.strip_prefix("https://").ok_or("the iCal private link is not https")?;
        let authority = rest.split(['/', '?']).next().unwrap_or("");
        if !authority.eq_ignore_ascii_case(FEED_HOST) {
            return Err("the iCal private link is not Planity's calendar export");
        }
        let path = rest[authority.len()..].split('?').next().unwrap_or("");
        if path != FEED_PATH {
            return Err("the iCal private link is not a calendar export");
        }
        let value = |name: &str| url::query_value(&address, name).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let calendar = value("calendarId").ok_or("the iCal private link names no calendar")?;
        let business = value("businessId").ok_or("the iCal private link names no business")?;
        let key = value("key").ok_or("the iCal private link has no key")?;
        Ok(Self { address, calendar, business, key })
    }
}

/// The calendar's feed, as text. A refusal of the key (401, 403) is the end
/// of the link (the person pastes it again); anything else, a feed not read.
pub fn read_feed(host: &mut dyn Host, link: &Link) -> Result<String, Fault> {
    let answer = ask(host, FEED_STEP, &Request::get(&link.address).header("Accept", "text/calendar"))?;
    match answer.status {
        200 => Ok(answer.body),
        401 | 403 => Err(Fault::SignIn(format!("{FEED_STEP} answered HTTP {}: the iCal private link (its key) was refused; paste the iCal private link again", answer.status))),
        404 => Err(Fault::SignIn(format!("{FEED_STEP} answered HTTP 404: no calendar at that iCal private link; paste the iCal private link again"))),
        _ => Err(answered(FEED_STEP, &answer, false)),
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

    /// The service's own code, from each shape its answers take (invented
    /// bodies in Firebase's and Google's shapes); its prose never.
    #[test]
    fn a_service_s_own_code_is_read_and_its_prose_left() {
        for (body, code) in [
            (r#"{"error":{"code":400,"message":"INVALID_LOGIN_CREDENTIALS","errors":[{"message":"INVALID_LOGIN_CREDENTIALS","domain":"global","reason":"invalid"}]}}"#, Some("INVALID_LOGIN_CREDENTIALS")),
            (r#"{"error":{"code":400,"message":"TOO_MANY_ATTEMPTS_TRY_LATER : Access to this account has been temporarily disabled."}}"#, Some("TOO_MANY_ATTEMPTS_TRY_LATER")),
            (r#"{"error":{"code":400,"message":"TOKEN_EXPIRED","status":"INVALID_ARGUMENT"}}"#, Some("TOKEN_EXPIRED")),
            (r#"{"error":{"code":400,"message":"API key not valid. Please pass a valid API key.","status":"INVALID_ARGUMENT","details":[{"reason":"API_KEY_INVALID"}]}}"#, Some("API_KEY_INVALID")),
            (r#"{"error":{"code":400,"message":"API key not valid. Please pass a valid API key.","errors":[{"reason":"keyInvalid"}]}}"#, Some("API_KEY_INVALID")),
            (r#"{"error":{"code":403,"message":"Missing or insufficient permissions.","status":"PERMISSION_DENIED"}}"#, Some("PERMISSION_DENIED")),
            (r#"{"error":"invalid_grant","error_description":"Bad Request"}"#, Some("INVALID_GRANT")),
            (r#"{"error":{"message":"the token tk-invented-1234 was refused"}}"#, None),
            ("<html>Service Unavailable</html>", None),
            ("", None),
        ] {
            assert_eq!(service_code(body).as_deref(), code, "{body}");
        }
    }

    #[test]
    fn firestore_values_read_into_their_kinds() {
        let page = json!([
            { "readTime": "2026-11-22T10:00:00Z" },
            { "document": { "name": "projects/p/databases/(default)/documents/businesses/-B/vevents/-V1", "fields": {
                "s": { "stringValue": "2026-11-20 14:00" },
                "d": { "integerValue": "30" },
                "p": { "integerValue": 4500 },
                "ad": { "booleanValue": true },
                "cu": { "mapValue": { "fields": { "id": { "stringValue": "C1" }, "name": { "stringValue": "Marie Exemple" }, "tags": { "arrayValue": { "values": [{ "stringValue": "x" }] } } } } },
                "dat": { "timestampValue": "2026-11-18T10:00:00Z" },
                "uat": { "integerValue": "1795000000000" },
                "t": { "nullValue": null },
                "sq": { "arrayValue": {} },
                "unread": { "mapValue": { "fields": { "deep": { "mapValue": {} } } } },
                "se": { "referenceValue": "projects/p/databases/(default)/documents/x" }
            }, "createTime": "2026-11-20T10:00:00Z" }, "readTime": "2026-11-22T10:00:00Z" }
        ]);
        let elements: Vec<Element> = serde_json::from_str(&page.to_string()).unwrap();
        assert!(elements[0].document.is_none(), "a read-time marker");
        let document = elements[1].document.as_ref().unwrap();
        assert!(document.name.ends_with("/vevents/-V1"));
        let f = &document.fields;
        let get = |t: &Option<Typed>| t.clone().unwrap_or_default();
        assert_eq!(get(&f.s).text(), Some("2026-11-20 14:00"));
        assert_eq!((get(&f.d).integer(), get(&f.p).integer(), get(&f.ad).boolean()), (Some(30), Some(4500), Some(true)));
        let client = get(&f.cu);
        let client = client.client().unwrap();
        assert_eq!((get(&client.id).text(), get(&client.name).text(), client.phone.is_none()), (Some("C1"), Some("Marie Exemple"), true));
        assert_eq!(get(&f.dat).text(), Some("2026-11-18T10:00:00Z"));
        assert!(get(&f.dat).present());
        assert_eq!(get(&f.uat).plain(), json!(1_795_000_000_000_i64));
        assert!(!get(&f.t).present(), "a null is no value");
        assert!(get(&f.sq).present() && get(&f.sq).text().is_none(), "an array is there, and no text");
        assert!(!get(&f.se).present(), "a reference is a kind not read");
        assert_eq!(Typed { integer_value: Some(json!("x9")), ..Typed::default() }.plain(), json!("x9"));
        assert_eq!(Typed { double_value: Some(json!(45)), ..Typed::default() }.integer(), Some(45));
        assert_eq!(Typed { double_value: Some(json!(45.5)), ..Typed::default() }.integer(), None);
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
