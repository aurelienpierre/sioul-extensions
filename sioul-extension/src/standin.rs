// SPDX-License-Identifier: GPL-3.0-or-later

//! A stand-in for Sioul, on a computer, for tests on recorded answers: the
//! web answered by a function the test gives, the secret kept in memory, the
//! sign-in answered as the test says (or as a function of the page opened),
//! the time fixed, random bytes never the same twice (not random: a test's). Every ask crosses the
//! same JSON as on `wasm32` ([`crate::forms`]), and the web's limits that an
//! extension can meet are checked as Sioul checks them: `https`, the hosts
//! named, no redirect followed.

use crate::{Answer, Host, Refused, Request, SignIn, forms};
use serde_json::{Value, json};

/// The stand-in. Its fields are the test's to set and to read.
pub struct StandIn {
    /// The time, seconds since 1970.
    pub now: i64,
    /// The hosts the manifest would name; empty: any.
    pub hosts: Vec<String>,
    /// What the web answers.
    pub web: Box<dyn FnMut(&Request) -> Answer>,
    /// Every request made, in order.
    pub asked: Vec<Request>,
    pub secret: Option<String>,
    /// The keyring refuses with this word (`keyring`): its secret neither read nor kept.
    pub secret_refused: Option<String>,
    /// What the person does on the sign-in page; none: `page` says, or no sign-in may be asked.
    pub sign_in: Option<SignIn>,
    /// What the person does on the page opened, given its address (a return that carries its `state`).
    pub page: Option<Box<dyn FnMut(&str) -> SignIn>>,
    /// Random bytes drawn so far: each draw differs.
    pub drawn: u64,
    /// Each sign-in asked: the page opened and the address watched for.
    pub opened: Vec<(String, String)>,
    pub log: Vec<String>,
    pub hidden: Vec<String>,
}

impl StandIn {
    /// A stand-in at `now`, whose web answers nothing (status 0 would not read: every request is refused).
    pub fn new(now: i64) -> Self {
        Self { now, hosts: Vec::new(), web: Box::new(|_| Answer::new(599, "")), asked: Vec::new(), secret: None, secret_refused: None, sign_in: None, page: None, drawn: 0, opened: Vec::new(), log: Vec::new(), hidden: Vec::new() }
    }

    /// Whether a value the extension hid shows in the log.
    pub fn log_shows(&self, value: &str) -> bool {
        self.log.iter().any(|l| l.contains(value))
    }

    /// A credentials `sign-in` call, run as Sioul's engine runs one
    /// (login-core-design.md): every value is hidden from the log first, then
    /// the values are handed to the extension in the call JSON under their
    /// fixed kind (`email`, `password`…). The extension does its own auth and
    /// keeps only what it chooses through `secret`; Sioul keeps nothing of
    /// them after the call. Returns the reply as JSON.
    pub fn sign_in_with<E: crate::Extension>(&mut self, lang: &str, choice: Option<&str>, credentials: &[(&str, &str)]) -> Value {
        let values: Vec<&str> = credentials.iter().map(|(_, value)| *value).collect();
        self.hide(&values);
        let fields: serde_json::Map<String, Value> = credentials.iter().map(|(kind, value)| ((*kind).to_string(), Value::String((*value).to_string()))).collect();
        let call = json!({ "interface": crate::INTERFACE, "call": "sign-in", "lang": lang, "choice": choice, "credentials": fields });
        serde_json::from_str(&crate::run::<E>(self, call.to_string().as_bytes())).unwrap_or(Value::Null)
    }

    fn checked(&self, request: &Request) -> Result<(), Refused> {
        let host = crate::url::https_host(&request.url).ok_or_else(|| Refused("scheme".into()))?;
        if !self.hosts.is_empty() && !self.hosts.iter().any(|h| h.eq_ignore_ascii_case(&host)) {
            return Err(Refused("host".into()));
        }
        Ok(())
    }
}

impl Host for StandIn {
    fn web(&mut self, request: &Request) -> Result<Answer, Refused> {
        // Through the JSON the module would write, and back.
        let ask = forms::web(request);
        let request = Request {
            method: if ask["method"] == "POST" { "POST" } else { "GET" },
            url: ask["url"].as_str().unwrap_or("").to_string(),
            headers: ask["headers"].as_array().map(|all| all.iter().filter_map(|h| Some((h[0].as_str()?.to_string(), h[1].as_str()?.to_string()))).collect()).unwrap_or_default(),
            body: ask["body"].as_str().map(str::to_string),
        };
        if let Err(refused) = self.checked(&request) {
            return forms::web_answer(&json!({ "refused": refused.0 }));
        }
        let answer = (self.web)(&request);
        self.asked.push(request);
        let headers: Vec<[String; 2]> = answer.headers.iter().map(|(n, v)| [n.to_ascii_lowercase(), v.clone()]).collect();
        forms::web_answer(&json!({ "status": answer.status, "headers": headers, "body": answer.body }))
    }

    fn secret(&mut self) -> Result<Option<String>, Refused> {
        if let Some(why) = &self.secret_refused {
            return forms::secret_answer(&json!({ "refused": why }));
        }
        forms::secret_answer(&json!({ "secret": self.secret }))
    }

    fn keep_secret(&mut self, secret: &str) -> Result<(), Refused> {
        if let Some(why) = &self.secret_refused {
            return forms::done(&json!({ "refused": why }));
        }
        if secret.len() > 2400 {
            return forms::done(&json!({ "refused": "size" }));
        }
        self.secret = Some(secret.to_string());
        forms::done(&json!({ "kept": true }))
    }

    fn forget_secret(&mut self) -> Result<(), Refused> {
        self.secret = None;
        forms::done(&json!({ "forgotten": true }))
    }

    fn sign_in(&mut self, open: &str, watch: &str) -> Result<SignIn, Refused> {
        self.opened.push((open.to_string(), watch.to_string()));
        let act = self.sign_in.take().or_else(|| self.page.as_mut().map(|page| page(open)));
        let answer: Value = match act {
            None => json!({ "refused": "not-now" }),
            Some(SignIn::Cancelled) => json!({ "cancelled": true }),
            Some(SignIn::Returned(address)) if address.starts_with(watch) => json!({ "returned": address }),
            Some(SignIn::Returned(_)) => json!({ "cancelled": true }),
        };
        forms::sign_in_answer(&answer)
    }

    fn now(&mut self) -> i64 {
        forms::now_answer(&json!({ "unix": self.now }))
    }

    fn random(&mut self, bytes: usize) -> Result<String, Refused> {
        if !(1..=64).contains(&bytes) {
            return forms::random_answer(&json!({ "refused": "form" }));
        }
        // Splitmix64 from a counter: different each draw, the same from one run to the next.
        let mut out = Vec::with_capacity(bytes);
        while out.len() < bytes {
            self.drawn += 1;
            let mut z = self.drawn.wrapping_mul(0x9E37_79B9_7F4A_7C15);
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            out.extend_from_slice(&(z ^ (z >> 31)).to_le_bytes());
        }
        out.truncate(bytes);
        forms::random_answer(&json!({ "random": crate::url::base64url_encode(&out) }))
    }

    fn log(&mut self, line: &str) {
        let mut line = line.to_string();
        for value in &self.hidden {
            line = line.replace(value.as_str(), "[hidden]");
        }
        self.log.push(line);
    }

    fn hide(&mut self, values: &[&str]) {
        self.hidden.extend(values.iter().filter(|v| v.len() >= 4).map(|v| v.to_string()));
    }
}
