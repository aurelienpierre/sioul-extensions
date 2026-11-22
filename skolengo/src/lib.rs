// SPDX-License-Identifier: GPL-3.0-or-later

//! A pupil's timetable from a school platform built on Skolengo, for Sioul.
//! **Unofficial**: not the vendor's, not endorsed by it.
//!
//! A port to Rust of what the community clients do, from their source:
//! scolengo-api (Maël Gangloff, GPL-3.0-or-later, commit 4241a20, cited as
//! "TS") and ha-skolengo (infernalK, MIT, commit 20dd40f, cited as "HA").
//!
//! - **The sign-in** is the mobile application's own: OpenID Connect, by
//!   authorization code, on the platform's sign-in service, with the
//!   application's client and its return address `skoapp-prod://…`, which
//!   Sioul watches for in a browser of its own (TS `index.ts:24-26`, `:44-51`;
//!   HA `const.py:11-13`, `api.py:337-357`, `api.py:997-1026`).
//! - **The renewal**: the refresh token posted to the token address with the
//!   client's identifier and secret, the new refresh token kept when one
//!   comes (HA `api.py:1031-1057`); on a 401 the access token is renewed
//!   once and the request asked again (TS `index.ts:1094-1108`).
//! - **The pupils of an account** (`users-info`, HA `api.py:1119-1132`): a
//!   parent's children, or the pupil whose account it is
//!   (HA `config_flow.py:288-300`).
//! - **A pupil's agenda**, fifteen days a request (wider ranges are cut
//!   without a word), a range answering 500 halved until the bad day is
//!   alone, then that day skipped (HA `api.py:1147-1207`); one week back to
//!   four ahead.
//!
//! Each lesson becomes an event: its subject, its teachers, its room; a
//! cancelled lesson (`canceled`, the only state field: HA
//! `coordinator.py:676-698`) a cancelled event. One agenda per pupil.

mod jsonapi;
pub mod platforms;
#[cfg(test)]
#[path = "../tests/recorded.rs"]
mod recorded;

use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sioul_extension::ical::{Calendar, Event};
use sioul_extension::{Agenda, Call, Choice, Extension, Host, Period, Problem, ProblemKind, RefreshReply, Refused, Request, SignIn, SignInReply, url};
use std::collections::BTreeMap;

/// The extension.
pub struct Skolengo;

sioul_extension::export!(Skolengo);

/// The data gateway of every platform (TS `index.ts:23`; HA `const.py:15`).
pub const API: &str = "https://api.skolengo.com/api/v1/bff-sko-app";
/// The mobile application's client, as every community client signs in
/// (TS `index.ts:24`, in base64 there; JV `SkolengoConstants.java:41`).
pub const CLIENT_ID: &str = "SkoApp.Prod.0d349217-9a4e-41ec-9af9-df9e69e09494";
/// Its secret (TS `index.ts:25`, in base64; JV `SkolengoConstants.java:42`).
pub const CLIENT_SECRET: &str = "7cb4d9a8-2580-4041-9ae8-d5803869183f";
/// Its return address after the sign-in (TS `index.ts:26`; HA `const.py:13`).
pub const RETURN: &str = "skoapp-prod://sign-in-callback";

const PRODUCT: &str = concat!("skolengo ", env!("CARGO_PKG_VERSION"));
const AGENT: &str = concat!("Sioul-extension-skolengo/", env!("CARGO_PKG_VERSION"));
/// Days a request of `/agendas` asks (HA `api.py:1147-1157`).
const CHUNK: i64 = 15;
/// The window: a week back, four ahead.
const BEFORE: i64 = 7;
const AHEAD: i64 = 28;
/// Ranges halved at most per pupil when they answer 500: two bad days found in
/// the window, a whole outage given up after some twenty requests (HA spends
/// forty on a whole school year, `api.py:1176`).
const HALVINGS: u32 = 8;
/// The zone of a school that says none.
const ZONE: &str = "Europe/Paris";

impl Extension for Skolengo {
    fn sign_in(host: &mut dyn Host, call: &Call) -> SignInReply {
        let Some(platform) = call.choice.as_deref().and_then(platforms::by_code) else {
            return SignInReply::Choose(platforms::PLATFORMS.iter().map(|p| Choice { id: p.code.to_string(), name: p.name.to_string() }).collect());
        };
        match sign_in_at(host, platform) {
            Ok(reply) => reply,
            Err(fault) => {
                host.log(&format!("sign-in on {}: {}", platform.code, fault.detail()));
                SignInReply::Problem(fault.problem())
            }
        }
    }

    fn refresh(host: &mut dyn Host, call: &Call) -> RefreshReply {
        let kept = match host.secret() {
            Ok(Some(text)) => match serde_json::from_str::<Kept>(&text) {
                Ok(kept) => kept,
                Err(_) => return RefreshReply::problem(ProblemKind::SignIn, "the sign-in kept does not read"),
            },
            Ok(None) => return RefreshReply::problem(ProblemKind::SignIn, "no sign-in kept"),
            Err(refused) => return RefreshReply { agendas: Vec::new(), problem: Some(Fault::from(refused).problem()) },
        };
        let mut session = Session { host, kept, renewed: false };
        match session.refresh(call.french()) {
            Ok(reply) => reply,
            Err(fault) => {
                session.host.log(&format!("refresh: {}", fault.detail()));
                RefreshReply { agendas: Vec::new(), problem: Some(fault.problem()) }
            }
        }
    }
}

// ---------------------------------------------------------------- what went wrong

/// What went wrong, before it is a problem said to Sioul.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Fault {
    SignIn(String),
    Unreachable(String),
    Refused(String),
    Changed(String),
    /// A status the caller may look at (500 on a range of days).
    Status(u16, String),
}

impl Fault {
    fn detail(&self) -> String {
        match self {
            Self::SignIn(d) | Self::Unreachable(d) | Self::Refused(d) | Self::Changed(d) => d.clone(),
            Self::Status(status, d) => format!("{d}: {status}"),
        }
    }

    fn problem(&self) -> Problem {
        let kind = match self {
            Self::SignIn(_) => ProblemKind::SignIn,
            Self::Unreachable(_) => ProblemKind::Unreachable,
            Self::Refused(_) => ProblemKind::Refused,
            Self::Changed(_) => ProblemKind::Changed,
            Self::Status(401, _) => ProblemKind::SignIn,
            Self::Status(403, _) => ProblemKind::Refused,
            Self::Status(s, _) if *s >= 500 => ProblemKind::Unreachable,
            Self::Status(_, _) => ProblemKind::Changed,
        };
        Problem::new(kind, self.detail())
    }
}

/// Sioul's refusal of an ask: a host or an address it would not reach means
/// the service moved (an update is needed); the network or a limit, that it
/// could not be reached now.
impl From<Refused> for Fault {
    fn from(refused: Refused) -> Self {
        let detail = format!("Sioul refused: {}", refused.0);
        match refused.0.as_str() {
            "network" | "time" | "keyring" | "count" => Self::Unreachable(detail),
            _ => Self::Changed(detail),
        }
    }
}

/// The service's refusal, from the error codes it sends (TS `models/Errors/SkolengoError.ts:1`).
fn refusal_in(body: &str) -> Option<&'static str> {
    ["BLOCKED_ACCOUNT", "SKO_APP_NOT_SUBSCRIBED", "SUSPENDED_ACCOUNT"].into_iter().find(|code| body.contains(code))
}

// ---------------------------------------------------------------- the sign-in

/// What is kept in the keyring: the platform, its token address, the
/// refresh token, the user, and the access token while it fits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Kept {
    platform: String,
    token_endpoint: String,
    refresh: String,
    sub: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    access: Option<String>,
    #[serde(default)]
    expires: i64,
}

/// The most the keyring keeps for an extension (docs/extensions.md).
const KEPT_BYTES: usize = 2400;

/// The sign-in kept, the access token left out when it would not fit.
fn keep(host: &mut dyn Host, kept: &Kept) -> Result<(), Fault> {
    let mut text = serde_json::to_string(kept).unwrap_or_default();
    if text.len() > KEPT_BYTES {
        let lean = Kept { access: None, expires: 0, ..kept.clone() };
        text = serde_json::to_string(&lean).unwrap_or_default();
    }
    if text.len() > KEPT_BYTES {
        return Err(Fault::Changed("the renewal token is longer than the keyring keeps".into()));
    }
    host.keep_secret(&text).map_err(Fault::from)
}

/// The user's identifier, the `sub` claim of an id token (read, not verified: TS `index.ts:557`, HA `api.py:129-137`).
fn subject(id_token: &str) -> Option<String> {
    let payload = id_token.split('.').nth(1)?;
    let claims: Value = serde_json::from_slice(&url::base64url(payload)?).ok()?;
    claims["sub"].as_str().filter(|s| !s.is_empty()).map(str::to_string)
}

/// The tokens of an answer of the token address, each hidden from the log.
fn tokens(host: &mut dyn Host, body: &Value) -> Result<(String, Option<String>, Option<String>, i64), Fault> {
    let access = body["access_token"].as_str().filter(|t| !t.is_empty()).ok_or_else(|| Fault::Changed("no access token in the answer".into()))?.to_string();
    let refresh = body["refresh_token"].as_str().filter(|t| !t.is_empty()).map(str::to_string);
    let id = body["id_token"].as_str().filter(|t| !t.is_empty()).map(str::to_string);
    let hidden: Vec<&str> = [Some(access.as_str()), refresh.as_deref(), id.as_deref()].into_iter().flatten().collect();
    host.hide(&hidden);
    // Bounded (E4): a huge value would overflow `now + lasts`, a negative one renew at every refresh.
    let lasts = body["expires_in"].as_i64().or_else(|| body["expires_in"].as_str().and_then(|s| s.parse().ok())).unwrap_or(3600).clamp(60, 86_400);
    Ok((access, refresh, id, lasts))
}

fn sign_in_at(host: &mut dyn Host, platform: &platforms::Platform) -> Result<SignInReply, Fault> {
    // The platform's discovery document (TS `index.ts:218-229`; HA `api.py:337-349`).
    let answer = host.web(&Request::get(platform.discovery).header("Accept", "application/json").header("User-Agent", AGENT))?;
    if answer.status != 200 {
        return Err(Fault::Status(answer.status, "the discovery document".into()));
    }
    let discovery = answer.json().ok_or_else(|| Fault::Changed("the discovery document does not read".into()))?;
    let authorize = discovery["authorization_endpoint"].as_str().filter(|a| url::https_host(a).is_some());
    let token = discovery["token_endpoint"].as_str().filter(|a| url::https_host(a).is_some());
    let (Some(authorize), Some(token)) = (authorize, token) else {
        return Err(Fault::Changed("the discovery document names no https sign-in or token address".into()));
    };
    // The scopes HA asks, those the platform lists, else openid (HA `api.py:347-357`).
    let listed: Vec<&str> = discovery["scopes_supported"].as_array().map(|all| all.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let mut scopes: Vec<&str> = ["openid", "profile", "email", "offline_access"].into_iter().filter(|s| listed.contains(s)).collect();
    if !scopes.contains(&"openid") {
        scopes = vec!["openid"];
    }
    // What ties the address returned to this sign-in: 256 random bits, never the time (E1).
    // Neither community client this is ported from sends PKCE: none is sent here either.
    let state = host.random(32)?;
    let scope = scopes.join(" ");
    let open = url::with_query(authorize, &[("response_type", "code"), ("client_id", CLIENT_ID), ("redirect_uri", RETURN), ("scope", &scope), ("state", &state)]);
    let returned = match host.sign_in(&open, RETURN)? {
        SignIn::Cancelled => return Ok(SignInReply::Cancelled),
        SignIn::Returned(returned) => returned,
    };
    if let Some(error) = url::query_value(&returned, "error") {
        return Err(Fault::SignIn(format!("the platform returned an error: {error}")));
    }
    if url::query_value(&returned, "state").as_deref() != Some(state.as_str()) {
        return Err(Fault::SignIn("the sign-in returned another state than the one sent".into()));
    }
    let code = url::query_value(&returned, "code").filter(|c| !c.is_empty()).ok_or_else(|| Fault::SignIn("the sign-in returned no code".into()))?;
    host.hide(&[&code]);
    // The code for tokens (HA `api.py:997-1026`).
    let form = [("grant_type", "authorization_code"), ("code", code.as_str()), ("redirect_uri", RETURN), ("client_id", CLIENT_ID), ("client_secret", CLIENT_SECRET)];
    let answer = host.web(&Request::post_form(token, &form).header("Accept", "application/json").header("User-Agent", AGENT))?;
    match answer.status {
        200 => {}
        400 | 401 => return Err(Fault::SignIn(format!("the code was refused: {}", answer.status))),
        status => return Err(Fault::Status(status, "the code's exchange".into())),
    }
    let body = answer.json().ok_or_else(|| Fault::Changed("the tokens do not read".into()))?;
    let (access, refresh, id, lasts) = tokens(host, &body)?;
    let refresh = refresh.ok_or_else(|| Fault::Changed("the platform gave no renewal token".into()))?;
    let sub = id.as_deref().and_then(subject).ok_or_else(|| Fault::Changed("the id token names no user".into()))?;
    let now = host.now();
    keep(host, &Kept { platform: platform.code.to_string(), token_endpoint: token.to_string(), refresh, sub, access: Some(access), expires: now + lasts })?;
    host.log(&format!("signed in on {}", platform.code));
    Ok(SignInReply::SignedIn)
}

// ---------------------------------------------------------------- a session

/// A signed-in session: the kept sign-in, renewed at most once a call.
struct Session<'h> {
    host: &'h mut dyn Host,
    kept: Kept,
    renewed: bool,
}

/// A pupil: its identifier, first name, school and zone.
struct Pupil {
    id: String,
    first_name: String,
    school: Option<String>,
    zone: TimeZone,
}

impl Session<'_> {
    /// The access token, renewed when it is missing or ends within a minute.
    fn access(&mut self) -> Result<String, Fault> {
        let now = self.host.now();
        match &self.kept.access {
            Some(access) if self.kept.expires - 60 > now => {
                let access = access.clone();
                self.host.hide(&[&access, &self.kept.refresh]);
                Ok(access)
            }
            _ => {
                self.renew()?;
                self.kept.access.clone().ok_or_else(|| Fault::SignIn("no access token".into()))
            }
        }
    }

    /// A new access token from the refresh token (HA `api.py:1031-1057`): a
    /// refusal is the end of the sign-in; a new refresh token is kept.
    fn renew(&mut self) -> Result<(), Fault> {
        self.renewed = true;
        self.host.hide(&[&self.kept.refresh]);
        let form = [("grant_type", "refresh_token"), ("refresh_token", self.kept.refresh.as_str()), ("client_id", CLIENT_ID), ("client_secret", CLIENT_SECRET)];
        let answer = self.host.web(&Request::post_form(&self.kept.token_endpoint, &form).header("Accept", "application/json").header("User-Agent", AGENT))?;
        match answer.status {
            200 => {}
            400 | 401 => return Err(Fault::SignIn(format!("the renewal was refused: {}", answer.status))),
            status => return Err(Fault::Status(status, "the renewal".into())),
        }
        let body = answer.json().ok_or_else(|| Fault::Changed("the renewed tokens do not read".into()))?;
        let (access, refresh, _, lasts) = tokens(self.host, &body)?;
        self.kept.access = Some(access);
        self.kept.expires = self.host.now() + lasts;
        if let Some(refresh) = refresh {
            self.kept.refresh = refresh;
        }
        keep(self.host, &self.kept)?;
        self.host.log("the sign-in renewed");
        Ok(())
    }

    /// A signed request to the gateway (`path` and its query), with the
    /// platform's headers (TS `index.ts:1094-1098`; HA `api.py:1071-1080`);
    /// on a 401 the token renewed once and the request asked again. Its JSON
    /// when it answers 200; its status otherwise.
    fn get(&mut self, path: &str, school: Option<&str>) -> Result<Value, Fault> {
        let mut renewed_here = false;
        loop {
            let access = self.access()?;
            let mut request = Request::get(format!("{API}{path}"))
                .header("Authorization", &format!("Bearer {access}"))
                .header("X-Skolengo-Date-Format", "utc")
                .header("X-Skolengo-Ems-Code", &self.kept.platform)
                .header("Accept", "application/json")
                .header("User-Agent", AGENT);
            if let Some(school) = school {
                request = request.header("X-Skolengo-School-Id", school);
            }
            let answer = self.host.web(&request)?;
            match answer.status {
                200 => return answer.json().ok_or_else(|| Fault::Changed(format!("{} does not read", short(path)))),
                401 if !renewed_here && !self.renewed => {
                    renewed_here = true;
                    self.kept.access = None;
                }
                401 => return Err(Fault::SignIn(format!("{} refused the renewed sign-in", short(path)))),
                403 if refusal_in(&answer.body).is_some() => return Err(Fault::Refused(format!("{}: {}", short(path), refusal_in(&answer.body).unwrap_or("")))),
                status => return Err(Fault::Status(status, short(path).to_string())),
            }
        }
    }

    fn refresh(&mut self, french: bool) -> Result<RefreshReply, Fault> {
        let pupils = self.pupils()?;
        self.host.log(&format!("refresh: {} pupil(s)", pupils.len()));
        let today = Timestamp::from_second(self.host.now()).map_err(|_| Fault::Changed("the time does not read".into()))?;
        let mut reply = RefreshReply::default();
        for pupil in pupils {
            match self.agenda(&pupil, today, french) {
                // Two pupils whose ids differ by case or punctuation alone (E4): the second's agenda takes a hash of its own.
                Ok(mut agenda) => {
                    if reply.agendas.iter().any(|a| a.id == agenda.id) {
                        agenda.id = hashed_id(&agenda.id, &pupil.id);
                    }
                    reply.agendas.push(agenda);
                }
                Err(fault) => {
                    self.host.log(&format!("a pupil's agenda: {}", fault.detail()));
                    // The end of the sign-in ends everything; another fault leaves the other pupils' agendas.
                    if matches!(fault.problem().kind, ProblemKind::SignIn) {
                        return Err(fault);
                    }
                    reply.problem.get_or_insert(fault.problem());
                }
            }
        }
        Ok(reply)
    }

    /// The account's pupils: a parent's children, else the account's own pupil.
    fn pupils(&mut self) -> Result<Vec<Pupil>, Fault> {
        // Only the fields the agenda needs are asked of each pupil (HA `api.py:1119-1132` asks more).
        let path = format!(
            "/users-info/{}?{}",
            url::encode(&self.kept.sub),
            url::form(&[("include", "school,students,students.school,schools,prioritySchool"), ("fields[student]", "firstName,lastName,school")])
        );
        let user = jsonapi::flatten(&self.get(&path, None)?);
        if !user.is_object() {
            return Err(Fault::Changed("users-info names nobody".into()));
        }
        let students: Vec<&Value> = user["students"].as_array().map(|s| s.iter().collect()).unwrap_or_default();
        let pupils: Vec<&Value> = if students.is_empty() { vec![&user] } else { students };
        Ok(pupils
            .into_iter()
            .filter_map(|p| {
                let id = p["id"].as_str().filter(|i| !i.is_empty())?.to_string();
                let school = if p["school"].is_object() { &p["school"] } else { &user["school"] };
                let zone = school["timeZone"].as_str().and_then(|z| jiff::tz::db().get(z).ok()).or_else(|| jiff::tz::db().get(ZONE).ok()).unwrap_or(TimeZone::UTC);
                let first_name = p["firstName"].as_str().or_else(|| p["lastName"].as_str()).unwrap_or("").trim().to_string();
                Some(Pupil { id, first_name, school: school["id"].as_str().map(str::to_string), zone })
            })
            .collect())
    }

    /// A pupil's agenda, the window asked fifteen days at a time; the days
    /// the server failed on said unread. None read at all: a fault.
    fn agenda(&mut self, pupil: &Pupil, now: Timestamp, french: bool) -> Result<Agenda, Fault> {
        let today = now.to_zoned(pupil.zone.clone()).date();
        let (first, last) = (add_days(today, -BEFORE)?, add_days(today, AHEAD)?);
        let mut lessons: BTreeMap<String, Value> = BTreeMap::new();
        let (mut unread, mut read, mut halvings) = (Vec::new(), 0usize, HALVINGS);
        let mut start = first;
        while start <= last {
            let end = std::cmp::min(add_days(start, CHUNK - 1)?, last);
            self.range(pupil, start, end, &mut halvings, &mut unread, &mut read, &mut lessons)?;
            start = add_days(end, 1)?;
        }
        if read == 0 {
            return Err(Fault::Unreachable(format!("no day from {first} to {last} could be read")));
        }
        let mut events: Vec<Event> = lessons.values().filter_map(event).collect();
        events.sort_by(|a, b| (a.start, &a.uid).cmp(&(b.start, &b.uid)));
        let name = match (french, pupil.first_name.is_empty()) {
            (true, false) => format!("Emploi du temps\u{202f}: {}", pupil.first_name),
            (true, true) => "Emploi du temps".to_string(),
            (false, false) => format!("Timetable: {}", pupil.first_name),
            (false, true) => "Timetable".to_string(),
        };
        let calendar = Calendar { product: PRODUCT.into(), name: name.clone(), events }.text();
        let unread = unread.into_iter().filter_map(|day| day_period(day, &pupil.zone)).collect();
        // The days asked, first to last, in the pupil's school's zone: Sioul removes a lesson of
        // them it no longer finds, the unread days aside.
        let period = day_period(first, &pupil.zone).zip(day_period(last, &pupil.zone)).map(|(first, last)| Period { from: first.from, until: last.until });
        // A pupil's lessons are the child's own time, not the reader's: not busy by default.
        Ok(Agenda { id: agenda_id(&pupil.id), name, calendar, period, unread, busy: false })
    }

    /// One request of `/agendas` from `start` to `end` (TS `index.ts:557-587`;
    /// HA `api.py:1134-1142`); a range answering 500 halved until the bad
    /// day is alone, then that day said unread (HA `api.py:1176-1207`).
    #[allow(clippy::too_many_arguments)]
    fn range(&mut self, pupil: &Pupil, start: Date, end: Date, halvings: &mut u32, unread: &mut Vec<Date>, read: &mut usize, lessons: &mut BTreeMap<String, Value>) -> Result<(), Fault> {
        let query = url::form(&[
            ("filter[student.id]", pupil.id.as_str()),
            ("filter[date][GE]", &start.to_string()),
            ("filter[date][LE]", &end.to_string()),
            ("include", "lessons,lessons.subject,lessons.teachers,homeworkAssignments,homeworkAssignments.subject"),
        ]);
        match self.get(&format!("/agendas?{query}"), pupil.school.as_deref()) {
            Ok(document) => {
                for day in jsonapi::flatten(&document).as_array().into_iter().flatten() {
                    for lesson in day["lessons"].as_array().into_iter().flatten() {
                        if let Some(id) = lesson["id"].as_str() {
                            lessons.insert(id.to_string(), lesson.clone());
                        }
                    }
                }
                *read += days_between(start, end);
                Ok(())
            }
            Err(Fault::Status(500, _)) if start == end => {
                self.host.log(&format!("agendas: 500 for {start} alone; that day keeps what it had"));
                unread.push(start);
                Ok(())
            }
            Err(Fault::Status(500, _)) if *halvings == 0 => {
                self.host.log(&format!("agendas: 500 for {start}..{end}, no halving left; those days keep what they had"));
                let mut day = start;
                while day <= end {
                    unread.push(day);
                    day = add_days(day, 1)?;
                }
                Ok(())
            }
            Err(Fault::Status(500, _)) => {
                *halvings -= 1;
                let middle = add_days(start, (end - start).get_days() as i64 / 2)?;
                self.host.log(&format!("agendas: 500 for {start}..{end}; halved"));
                self.range(pupil, start, middle, halvings, unread, read, lessons)?;
                self.range(pupil, add_days(middle, 1)?, end, halvings, unread, read, lessons)
            }
            Err(fault) => Err(fault),
        }
    }
}

/// A short name of a path for the log: no query, no identifier.
fn short(path: &str) -> &str {
    let path = path.split('?').next().unwrap_or(path);
    let first = path.trim_start_matches('/').split('/').next().unwrap_or("");
    match first {
        "users-info" => "users-info",
        "agendas" => "agendas",
        _ => "a request",
    }
}

fn add_days(day: Date, n: i64) -> Result<Date, Fault> {
    day.checked_add(jiff::Span::new().days(n)).map_err(|_| Fault::Changed("a date out of range".into()))
}

fn days_between(start: Date, end: Date) -> usize {
    usize::try_from((end - start).get_days()).unwrap_or(0) + 1
}

/// A day of the school's zone as a period in UTC, from its start to the next day's.
fn day_period(day: Date, zone: &TimeZone) -> Option<Period> {
    let from = day.to_zoned(zone.clone()).ok()?.start_of_day().ok()?.timestamp().as_second();
    let until = day.tomorrow().ok()?.to_zoned(zone.clone()).ok()?.start_of_day().ok()?.timestamp().as_second();
    Some(Period::between(from, until))
}

/// An agenda's id from a pupil's: lower case, `-` for anything else, 48 at
/// most; a hash ends a longer one, and makes one of a pupil's id without a
/// letter or a digit (never an id beginning with `-`, which Sioul refuses: E4).
fn agenda_id(pupil: &str) -> String {
    let mut id: String = pupil.to_ascii_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    while id.contains("--") {
        id = id.replace("--", "-");
    }
    let id = id.trim_matches('-').to_string();
    if !id.is_empty() && id.len() <= 48 {
        return id;
    }
    hashed_id(&id, pupil)
}

/// `id` cut to 39 characters, then a hash of the pupil's own id: `pupil-<hash>` when nothing is left.
fn hashed_id(id: &str, pupil: &str) -> String {
    let hash = pupil.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193));
    let head = id[..id.len().min(39)].trim_matches('-');
    format!("{}-{hash:08x}", if head.is_empty() { "pupil" } else { head })
}

/// A time of the gateway: RFC 3339, or a civil time without zone, read as UTC (HA `timetable.py:21-28`).
fn instant(text: &str) -> Option<i64> {
    if let Ok(at) = text.parse::<Timestamp>() {
        return Some(at.as_second());
    }
    let civil: jiff::civil::DateTime = text.parse().ok()?;
    Some(civil.to_zoned(TimeZone::UTC).ok()?.timestamp().as_second())
}

/// A lesson as an event: its subject, its teachers, its room; cancelled when it says so.
fn event(lesson: &Value) -> Option<Event> {
    let id = lesson["id"].as_str()?;
    let start = instant(lesson["startDateTime"].as_str()?)?;
    let end = lesson["endDateTime"].as_str().and_then(instant).unwrap_or(start);
    let text = |v: &Value| v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let summary = text(&lesson["subject"]["label"]).or_else(|| text(&lesson["title"])).unwrap_or_else(|| "?".to_string());
    let location = match (text(&lesson["location"]), text(&lesson["locationComplement"])) {
        (Some(room), Some(more)) => Some(format!("{room} {more}")),
        (room, more) => room.or(more),
    };
    let teachers: Vec<String> = lesson["teachers"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|t| [text(&t["title"]), text(&t["firstName"]), text(&t["lastName"])].into_iter().flatten().collect::<Vec<_>>().join(" "))
        .filter(|t| !t.is_empty())
        .collect();
    Some(Event {
        uid: format!("{id}@skolengo"),
        start,
        end,
        summary,
        location,
        description: (!teachers.is_empty()).then(|| teachers.join(", ")),
        cancelled: lesson["canceled"] == true,
        // A pupil's lesson is not the time of the person who reads it (a parent).
        transparent: true,
        all_day: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_names_and_times_read() {
        assert_eq!(agenda_id("ESKO-P-0a1b2c3d-0000-4000-8000-000000000001"), "esko-p-0a1b2c3d-0000-4000-8000-000000000001");
        let long = agenda_id(&"X".repeat(80));
        assert!(long.len() <= 48 && long.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'), "{long}");
        assert_ne!(agenda_id(&format!("{}A", "X".repeat(80))), long);
        // E4: no letter, no digit; two ids alike but for case or punctuation.
        let bare = agenda_id("--__--");
        assert!(bare.starts_with("pupil-") && !bare.ends_with('-'), "{bare}");
        assert_eq!(agenda_id("P-1"), agenda_id("p_1"));
        assert_ne!(hashed_id(&agenda_id("p_1"), "p_1"), agenda_id("P-1"));
        assert_eq!(instant("2026-11-19T08:00:00Z"), Some(1_795_075_200));
        assert_eq!(instant("2026-11-19T09:00:00+01:00"), Some(1_795_075_200));
        assert_eq!(instant("2026-11-19T08:00:00"), Some(1_795_075_200), "no zone: UTC");
        assert_eq!(subject("e30.eyJzdWIiOiJVU0VSLTEifQ.c2ln").as_deref(), Some("USER-1"));
        assert_eq!(subject("not a token"), None);
        let paris = jiff::tz::db().get("Europe/Paris").unwrap();
        let period = day_period(Date::new(2026, 11, 20).unwrap(), &paris).unwrap();
        assert_eq!((period.from.as_str(), period.until.as_str()), ("2026-11-19T23:00:00Z", "2026-11-20T23:00:00Z"));
        assert_eq!(short("/users-info/ESKO-U-1?include=x"), "users-info");
    }
}
