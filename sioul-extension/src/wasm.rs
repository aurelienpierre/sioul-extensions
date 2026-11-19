// SPDX-License-Identifier: GPL-3.0-or-later

//! The module's side of Sioul's interface, on `wasm32` only: the imports of
//! the module `sioul`, each ask written as JSON in the module's memory, its
//! answer copied back, and the call's reply given once.

use crate::{Extension, Host, Refused, Request, SignIn, forms};
use serde_json::{Value, json};

#[link(wasm_import_module = "sioul")]
unsafe extern "C" {
    fn answer(ptr: i32);
    fn reply(ptr: i32, len: i32);
    fn now(ptr: i32, len: i32) -> i32;
    fn log(ptr: i32, len: i32) -> i32;
    fn hide(ptr: i32, len: i32) -> i32;
    fn web(ptr: i32, len: i32) -> i32;
    fn secret(ptr: i32, len: i32) -> i32;
    fn sign_in(ptr: i32, len: i32) -> i32;
}

/// One ask through `import`: the ask written, its answer's length returned,
/// the answer copied here.
fn ask(import: unsafe extern "C" fn(i32, i32) -> i32, value: &Value) -> Value {
    let bytes = value.to_string().into_bytes();
    // SAFETY: the pointer and length name `bytes`, alive for the call; Sioul only reads them.
    let len = unsafe { import(bytes.as_ptr() as i32, bytes.len() as i32) };
    if len <= 0 {
        return json!({ "refused": "form" });
    }
    let mut buffer = vec![0u8; len as usize];
    // SAFETY: `buffer` holds exactly the answer's length, which Sioul writes and no more.
    unsafe { answer(buffer.as_mut_ptr() as i32) };
    serde_json::from_slice(&buffer).unwrap_or_else(|_| json!({ "refused": "form" }))
}

/// Sioul, as the extension's host.
pub struct Sioul;

impl Host for Sioul {
    fn web(&mut self, request: &Request) -> Result<crate::Answer, Refused> {
        forms::web_answer(&ask(web, &forms::web(request)))
    }

    fn secret(&mut self) -> Result<Option<String>, Refused> {
        forms::secret_answer(&ask(secret, &json!({ "get": true })))
    }

    fn keep_secret(&mut self, kept: &str) -> Result<(), Refused> {
        forms::done(&ask(secret, &json!({ "keep": kept })))
    }

    fn forget_secret(&mut self) -> Result<(), Refused> {
        forms::done(&ask(secret, &json!({ "forget": true })))
    }

    fn sign_in(&mut self, open: &str, watch: &str) -> Result<SignIn, Refused> {
        forms::sign_in_answer(&ask(sign_in, &json!({ "open": open, "watch": watch })))
    }

    fn now(&mut self) -> i64 {
        forms::now_answer(&ask(now, &json!({})))
    }

    fn log(&mut self, line: &str) {
        let _ = ask(log, &json!({ "line": line }));
    }

    fn hide(&mut self, values: &[&str]) {
        let _ = ask(hide, &json!({ "values": values }));
    }
}

/// `sioul_alloc`: `len` bytes for Sioul to write a call into (one at least),
/// kept until the call takes them back; a boxed slice, so that its length is
/// its allocation's.
pub fn alloc(len: i32) -> i32 {
    let len = usize::try_from(len).unwrap_or(0).max(1);
    Box::into_raw(vec![0u8; len].into_boxed_slice()) as *mut u8 as i32
}

/// `sioul_call`: the call at `ptr` run, its reply given, its buffer freed.
pub fn call<E: Extension>(ptr: i32, len: i32) -> i32 {
    let len = usize::try_from(len).unwrap_or(0);
    // SAFETY: `ptr` came from `alloc` for this length (one byte when it was 0),
    // and Sioul wrote `len` bytes there; it is taken back once.
    let buffer = unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr as *mut u8, len.max(1))) };
    let said = crate::run::<E>(&mut Sioul, &buffer[..len]);
    drop(buffer);
    // SAFETY: the pointer and length name `said`, alive for the call.
    unsafe { reply(said.as_ptr() as i32, said.len() as i32) };
    0
}
