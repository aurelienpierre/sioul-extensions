// SPDX-License-Identifier: GPL-3.0-or-later

//! What every extension of Sioul shares, so that an extension's own code is
//! its logic alone.
//!
//! Sioul's interface is written down in Sioul's repository,
//! `docs/extensions.md`; this crate speaks its version 1 ([`INTERFACE`]):
//!
//! - **the calls** Sioul makes ([`Extension::sign_in`],
//!   [`Extension::refresh`]) and their replies ([`SignInReply`],
//!   [`RefreshReply`], an [`Agenda`], a [`Problem`]);
//! - **what an extension may ask** Sioul, through a [`Host`]: a web request
//!   ([`Request`], [`Answer`]), its own secret, a sign-in, the time, random
//!   bytes, a line in its log, values to hide from that log;
//! - **an iCalendar writer** ([`ical`]) for the agenda it hands back;
//! - **the plumbing**: JSON through the module's memory ([`export!`] makes the
//!   three exports Sioul runs), on `wasm32` only. On a computer the same code
//!   runs against a stand-in host ([`standin`]), for tests on recorded answers.

pub mod ical;
#[cfg(not(target_arch = "wasm32"))]
pub mod standin;
pub mod url;
#[cfg(target_arch = "wasm32")]
#[doc(hidden)]
pub mod wasm;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The version of Sioul's interface this crate speaks.
pub const INTERFACE: i32 = 1;

// ---------------------------------------------------------------- what an extension asks

/// Sioul's refusal of an ask, in one word (`host`, `scheme`, `size`, `time`…: docs/extensions.md).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused(pub String);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "refused by Sioul: {}", self.0)
    }
}

/// A web request, made by Sioul for the extension: `https`, to a host its
/// manifest names, no redirect followed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// `GET` or `POST`.
    pub method: &'static str,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

impl Request {
    pub fn get(url: impl Into<String>) -> Self {
        Self { method: "GET", url: url.into(), headers: Vec::new(), body: None }
    }

    /// A form posted (`application/x-www-form-urlencoded`).
    pub fn post_form(url: impl Into<String>, pairs: &[(&str, &str)]) -> Self {
        Self { method: "POST", url: url.into(), headers: vec![("Content-Type".into(), "application/x-www-form-urlencoded".into())], body: Some(url::form(pairs)) }
    }

    /// A JSON body posted (`application/json`).
    pub fn post_json(url: impl Into<String>, body: &Value) -> Self {
        Self { method: "POST", url: url.into(), headers: vec![("Content-Type".into(), "application/json".into())], body: Some(body.to_string()) }
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    fn to_json(&self) -> Value {
        let mut ask = json!({ "method": self.method, "url": self.url, "headers": self.headers.iter().map(|(n, v)| [n, v]).collect::<Vec<_>>() });
        if let Some(body) = &self.body {
            ask["body"] = Value::String(body.clone());
        }
        ask
    }
}

/// What a web request answered: its status whatever it is, its headers
/// (names in lower case), its body as text.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Answer {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Answer {
    /// An answer with a status and a body, for stand-ins.
    pub fn new(status: u16, body: impl Into<String>) -> Self {
        Self { status, headers: Vec::new(), body: body.into() }
    }

    /// A header's value, its name in any case.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    /// The body as JSON.
    pub fn json(&self) -> Option<Value> {
        serde_json::from_str(&self.body).ok()
    }

    fn from_json(answer: &Value) -> Result<Self, Refused> {
        refusal(answer)?;
        let status = answer["status"].as_u64().and_then(|s| u16::try_from(s).ok()).ok_or_else(|| Refused("form".into()))?;
        let headers = answer["headers"]
            .as_array()
            .map(|all| all.iter().filter_map(|h| Some((h.get(0)?.as_str()?.to_string(), h.get(1)?.as_str()?.to_string()))).collect())
            .unwrap_or_default();
        Ok(Self { status, headers, body: answer["body"].as_str().unwrap_or("").to_string() })
    }
}

/// What a sign-in page came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignIn {
    /// The whole address the service returned to, which began with what was watched for.
    Returned(String),
    /// The person closed the page.
    Cancelled,
}

/// What an extension may ask Sioul for. On `wasm32` it is Sioul itself
/// ([`wasm::Sioul`]); on a computer, a stand-in ([`standin::StandIn`]).
pub trait Host {
    /// A web request (`https`, to a host the manifest names).
    fn web(&mut self, request: &Request) -> Result<Answer, Refused>;
    /// The extension's own secret, as last kept.
    fn secret(&mut self) -> Result<Option<String>, Refused>;
    /// Keeps the secret (2400 bytes at most), in place of the last.
    fn keep_secret(&mut self, secret: &str) -> Result<(), Refused>;
    /// Forgets the secret.
    fn forget_secret(&mut self) -> Result<(), Refused>;
    /// Shows `open`, and gives back the address the service returned to
    /// once it begins with `watch`; only during a sign-in call.
    fn sign_in(&mut self, open: &str, watch: &str) -> Result<SignIn, Refused>;
    /// The time now, in seconds since 1970 (UTC).
    fn now(&mut self) -> i64;
    /// `bytes` random bytes (1 to 64) from Sioul's system, in URL-safe
    /// base64 without padding: a sign-in's `state`, a PKCE verifier. A
    /// module built for `wasm32-unknown-unknown` has no randomness of its own.
    fn random(&mut self, bytes: usize) -> Result<String, Refused>;
    /// A line in the extension's own log, kept on the device.
    fn log(&mut self, line: &str);
    /// Values that must never reach the log (a token, a code).
    fn hide(&mut self, values: &[&str]);
}

/// The answer's refusal, if Sioul refused.
fn refusal(answer: &Value) -> Result<(), Refused> {
    match answer.get("refused") {
        Some(why) => Err(Refused(why.as_str().unwrap_or("form").to_string())),
        None => Ok(()),
    }
}

// ---------------------------------------------------------------- the calls and their replies

/// A call of Sioul's.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Call {
    pub interface: i32,
    pub call: String,
    /// The language Sioul speaks now (`en`, `fr`), for the names given.
    #[serde(default)]
    pub lang: String,
    /// On a `sign-in` call: the id the person chose among those offered.
    #[serde(default)]
    pub choice: Option<String>,
    /// On a `sign-in` call for a credentials extension: the fields the person
    /// typed into Sioul's own form, by their fixed kind (login-core-design.md).
    /// Sioul hides every value from the log and keeps none once the call ends;
    /// the extension keeps only what it chooses through [`Host::keep_secret`]
    /// (a refresh token), never the password.
    #[serde(default)]
    pub credentials: Option<Credentials>,
}

impl Call {
    /// Whether Sioul speaks French now.
    pub fn french(&self) -> bool {
        self.lang == "fr"
    }
}

/// The credential values a `sign-in` call may carry, each under its fixed
/// kind (login-core-design.md's closed set: `email`, `username`, `password`,
/// `server`, `one-time-code`). Only the kinds the signed manifest declares
/// are ever filled.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
pub struct Credentials {
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub server: Option<String>,
    #[serde(default, rename = "one-time-code")]
    pub one_time_code: Option<String>,
}

/// One thing the person may choose before a sign-in (a platform, a school).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Choice {
    pub id: String,
    pub name: String,
}

/// What a problem is, for Sioul to say it in its own words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProblemKind {
    /// The sign-in has ended or was refused: the person signs in again.
    SignIn,
    /// The service did not answer.
    Unreachable,
    /// The service refused (an account blocked, a school not subscribed).
    Refused,
    /// The service's answers no longer read as expected.
    Changed,
}

/// A problem, its detail for the log only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Problem {
    pub kind: ProblemKind,
    pub detail: String,
}

impl Problem {
    pub fn new(kind: ProblemKind, detail: impl Into<String>) -> Self {
        Self { kind, detail: detail.into() }
    }
}

/// The reply to a `sign-in` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignInReply {
    SignedIn,
    /// The person chooses first; Sioul calls `sign-in` again with the choice.
    Choose(Vec<Choice>),
    Cancelled,
    Problem(Problem),
}

impl SignInReply {
    pub fn to_json(&self) -> Value {
        match self {
            Self::SignedIn => json!({ "signed_in": true }),
            Self::Choose(choices) => json!({ "choose": choices }),
            Self::Cancelled => json!({ "cancelled": true }),
            Self::Problem(problem) => json!({ "problem": problem }),
        }
    }
}

/// A period: the one an agenda read, or one it could not read (what Sioul had there stays).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Period {
    /// RFC 3339, UTC (`2026-11-20T00:00:00Z`).
    pub from: String,
    pub until: String,
}

impl Period {
    /// From and until, in seconds since 1970.
    pub fn between(from: i64, until: i64) -> Self {
        Self { from: ical::rfc3339(from), until: ical::rfc3339(until) }
    }
}

/// An agenda handed back. Sioul writes its events into a calendar of the
/// person's own, each marked as this extension's: one not yet there is
/// added, one changed written again, and one of `period` that the agenda no
/// longer holds removed, except on the `unread` days.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Agenda {
    /// Lower-case letters, digits and `-`, 48 at most; stable.
    pub id: String,
    pub name: String,
    /// iCalendar text, events only ([`ical::Calendar::text`]): each with a
    /// UID of its own, the same at every refresh; no category (Sioul adds
    /// the extension's name).
    pub calendar: String,
    /// The period this answer read, its unread days aside: what Sioul holds
    /// of this agenda there and no longer finds in it is removed. None:
    /// nothing is removed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period: Option<Period>,
    pub unread: Vec<Period>,
    /// The extension's suggestion that these are the current user's **own**
    /// busy time (so the door may write `TRANSP:OPAQUE`), not another
    /// person's (busy-core-design.md). It is a default for the person's
    /// per-agenda choice, never the final word: the door writes the
    /// transparency from that choice. Planity's appointments: true; a child's
    /// Skolengo lessons: false.
    pub busy: bool,
}

/// The reply to a `refresh` call: the agendas, and a problem if there was one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct RefreshReply {
    pub agendas: Vec<Agenda>,
    pub problem: Option<Problem>,
}

impl RefreshReply {
    pub fn problem(kind: ProblemKind, detail: impl Into<String>) -> Self {
        Self { agendas: Vec::new(), problem: Some(Problem::new(kind, detail)) }
    }
}

/// An extension: what it does for each call. Its file exports it with [`export!`].
pub trait Extension {
    /// The person pressed Sign in…
    fn sign_in(host: &mut dyn Host, call: &Call) -> SignInReply;
    /// Sioul wants the extension's data now.
    fn refresh(host: &mut dyn Host, call: &Call) -> RefreshReply;
}

/// One call, as JSON text in and out: what the module's export runs, and
/// what tests run on a computer. A call of another interface or of an
/// unknown name is answered with a problem.
pub fn run<E: Extension>(host: &mut dyn Host, call: &[u8]) -> String {
    let reply = match serde_json::from_slice::<Call>(call) {
        Err(e) => json!({ "problem": Problem::new(ProblemKind::Changed, format!("a call that does not read: {e}")) }),
        Ok(call) if call.interface != INTERFACE => json!({ "problem": Problem::new(ProblemKind::Changed, format!("interface {} asked, {INTERFACE} spoken", call.interface)) }),
        Ok(call) => match call.call.as_str() {
            "sign-in" => E::sign_in(host, &call).to_json(),
            "refresh" => serde_json::to_value(E::refresh(host, &call)).unwrap_or(Value::Null),
            other => json!({ "problem": Problem::new(ProblemKind::Changed, format!("an unknown call: {other}")) }),
        },
    };
    reply.to_string()
}

/// The three exports Sioul runs (`sioul_interface`, `sioul_alloc`,
/// `sioul_call`), for the extension `$extension`; on `wasm32` only.
#[macro_export]
macro_rules! export {
    ($extension:ty) => {
        #[cfg(target_arch = "wasm32")]
        #[unsafe(no_mangle)]
        pub extern "C" fn sioul_interface() -> i32 {
            $crate::INTERFACE
        }

        #[cfg(target_arch = "wasm32")]
        #[unsafe(no_mangle)]
        pub extern "C" fn sioul_alloc(len: i32) -> i32 {
            $crate::wasm::alloc(len)
        }

        #[cfg(target_arch = "wasm32")]
        #[unsafe(no_mangle)]
        pub extern "C" fn sioul_call(ptr: i32, len: i32) -> i32 {
            $crate::wasm::call::<$extension>(ptr, len)
        }
    };
}

// ---------------------------------------------------------------- asks as JSON, both sides

/// The JSON of each ask, and the reading of each answer: the same on
/// `wasm32` (Sioul's imports) and in the stand-in, so that the tests cross
/// the same forms the module does.
#[doc(hidden)]
pub mod forms {
    use super::*;

    pub fn web(request: &Request) -> Value {
        request.to_json()
    }

    pub fn web_answer(answer: &Value) -> Result<Answer, Refused> {
        Answer::from_json(answer)
    }

    pub fn secret_answer(answer: &Value) -> Result<Option<String>, Refused> {
        refusal(answer)?;
        Ok(answer["secret"].as_str().map(str::to_string))
    }

    pub fn done(answer: &Value) -> Result<(), Refused> {
        refusal(answer)
    }

    pub fn sign_in_answer(answer: &Value) -> Result<SignIn, Refused> {
        refusal(answer)?;
        match answer["returned"].as_str() {
            Some(returned) => Ok(SignIn::Returned(returned.to_string())),
            None => Ok(SignIn::Cancelled),
        }
    }

    pub fn now_answer(answer: &Value) -> i64 {
        answer["unix"].as_i64().unwrap_or(0)
    }

    pub fn random_answer(answer: &Value) -> Result<String, Refused> {
        refusal(answer)?;
        answer["random"].as_str().filter(|r| !r.is_empty()).map(str::to_string).ok_or_else(|| Refused("form".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;

    impl Extension for Echo {
        fn sign_in(_: &mut dyn Host, call: &Call) -> SignInReply {
            match call.choice.as_deref() {
                None => SignInReply::Choose(vec![Choice { id: "a".into(), name: "A".into() }]),
                Some(_) => SignInReply::SignedIn,
            }
        }

        fn refresh(host: &mut dyn Host, _: &Call) -> RefreshReply {
            let now = host.now();
            RefreshReply { agendas: vec![Agenda { id: "one".into(), name: "One".into(), calendar: String::new(), period: Some(Period::between(now - 86_400, now + 86_400)), unread: vec![Period::between(now, now + 86_400)], busy: false }], problem: None }
        }
    }

    #[test]
    fn calls_and_replies_cross_as_json() {
        let mut host = standin::StandIn::new(1_795_046_400);
        let reply: Value = serde_json::from_str(&run::<Echo>(&mut host, br#"{"interface":1,"call":"sign-in","lang":"fr","choice":null}"#)).unwrap();
        assert_eq!(reply, json!({ "choose": [{ "id": "a", "name": "A" }] }));
        let reply: Value = serde_json::from_str(&run::<Echo>(&mut host, br#"{"interface":1,"call":"sign-in","choice":"a"}"#)).unwrap();
        assert_eq!(reply, json!({ "signed_in": true }));
        let reply: Value = serde_json::from_str(&run::<Echo>(&mut host, br#"{"interface":1,"call":"refresh","lang":"en"}"#)).unwrap();
        assert_eq!(reply["agendas"][0]["unread"][0], json!({ "from": "2026-11-19T00:00:00Z", "until": "2026-11-20T00:00:00Z" }));
        assert_eq!(reply["agendas"][0]["period"], json!({ "from": "2026-11-18T00:00:00Z", "until": "2026-11-20T00:00:00Z" }));
        assert_eq!(reply["agendas"][0]["busy"], json!(false), "the agenda carries its busy suggestion");
        assert_eq!(reply["problem"], Value::Null);
        for wrong in [&br#"{"interface":2,"call":"refresh"}"#[..], br#"{"interface":1,"call":"dance"}"#, b"not json"] {
            let reply: Value = serde_json::from_str(&run::<Echo>(&mut host, wrong)).unwrap();
            assert_eq!(reply["problem"]["kind"], "changed", "{}", String::from_utf8_lossy(wrong));
        }
    }

    #[test]
    fn answers_and_refusals_read() {
        let answer = forms::web_answer(&json!({ "status": 500, "headers": [["content-type", "text/plain"]], "body": "no" })).unwrap();
        assert_eq!((answer.status, answer.header("Content-Type"), answer.body.as_str()), (500, Some("text/plain"), "no"));
        assert_eq!(forms::web_answer(&json!({ "refused": "host" })), Err(Refused("host".into())));
        assert_eq!(forms::sign_in_answer(&json!({ "cancelled": true })), Ok(SignIn::Cancelled));
        assert_eq!(forms::secret_answer(&json!({ "secret": null })), Ok(None));
        assert_eq!(forms::random_answer(&json!({ "random": "AbC_-9" })).as_deref(), Ok("AbC_-9"));
        assert_eq!(forms::random_answer(&json!({ "refused": "form" })), Err(Refused("form".into())));
        let form = Request::post_form("https://example.org/token", &[("grant_type", "refresh_token"), ("refresh_token", "RT-a b&c")]);
        assert_eq!(form.body.as_deref(), Some("grant_type=refresh_token&refresh_token=RT-a+b%26c"));
    }

    #[test]
    fn a_call_may_carry_credentials() {
        // A credentials sign-in call carries the fields by their fixed kind; a
        // call without them reads None (login-core-design.md).
        let call: Call = serde_json::from_str(r#"{"interface":1,"call":"sign-in","lang":"fr","choice":null,"credentials":{"email":"a@b.org","password":"pw"}}"#).unwrap();
        let credentials = call.credentials.expect("credentials");
        assert_eq!((credentials.email.as_deref(), credentials.password.as_deref()), (Some("a@b.org"), Some("pw")));
        assert_eq!((credentials.username, credentials.one_time_code), (None, None));
        let bare: Call = serde_json::from_str(r#"{"interface":1,"call":"sign-in"}"#).unwrap();
        assert_eq!(bare.credentials, None);
        let coded: Call = serde_json::from_str(r#"{"interface":1,"call":"sign-in","credentials":{"one-time-code":"123456"}}"#).unwrap();
        assert_eq!(coded.credentials.unwrap().one_time_code.as_deref(), Some("123456"));
    }
}
