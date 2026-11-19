// SPDX-License-Identifier: GPL-3.0-or-later

//! Addresses, as an extension needs them: a query or a form encoded, a
//! value read back from a query, a host told, base64url decoded (a JWT's
//! claims). Small on purpose: an extension's file stays small.

/// A value encoded for a query or a form (`application/x-www-form-urlencoded`):
/// letters, digits and `-._~` as they are, a space as `+`, the rest as `%XX`.
pub fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(byte as char),
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Pairs as a form or a query: `a=1&b=2`.
pub fn form(pairs: &[(&str, &str)]) -> String {
    pairs.iter().map(|(name, value)| format!("{}={}", encode(name), encode(value))).collect::<Vec<_>>().join("&")
}

/// `base` with the pairs as its query.
pub fn with_query(base: &str, pairs: &[(&str, &str)]) -> String {
    if pairs.is_empty() {
        return base.to_string();
    }
    let joint = if base.contains('?') { '&' } else { '?' };
    format!("{base}{joint}{}", form(pairs))
}

/// A `%XX` and `+` encoded value decoded; bytes that are not UTF-8 replaced.
pub fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                match u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("zz"), 16) {
                    Ok(byte) => {
                        out.push(byte);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            other => out.push(other),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The first value of `name` in the query of `address` (after `?`, before `#`), decoded.
pub fn query_value(address: &str, name: &str) -> Option<String> {
    let query = address.split_once('?')?.1;
    let query = query.split('#').next().unwrap_or("");
    query.split('&').filter_map(|pair| pair.split_once('=').or(Some((pair, "")))).find(|(n, _)| decode(n) == name).map(|(_, v)| decode(v))
}

/// The host of an `https://` address, in lower case; none for another scheme.
pub fn https_host(address: &str) -> Option<String> {
    let rest = address.strip_prefix("https://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    Some(authority.to_ascii_lowercase())
}

/// Bytes in base64url (RFC 4648 §5), without padding.
pub fn base64url_encode(bytes: &[u8]) -> String {
    const LETTERS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..=chunk.len() {
            out.push(char::from(LETTERS[((n >> (18 - 6 * i)) & 63) as usize]));
        }
    }
    out
}

/// Base64url (RFC 4648 §5), with or without padding, decoded; none when it is not.
pub fn base64url(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for c in text.trim_end_matches('=').bytes() {
        let value = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_cross_both_ways() {
        assert_eq!(encode("a b/c?d=é"), "a+b%2Fc%3Fd%3D%C3%A9");
        assert_eq!(decode("a+b%2Fc%3Fd%3D%C3%A9"), "a b/c?d=é");
        assert_eq!(decode("100%"), "100%");
        assert_eq!(decode("%zz"), "%zz");
        assert_eq!(with_query("https://example.org/a", &[("x", "1"), ("y", "a b")]), "https://example.org/a?x=1&y=a+b");
        assert_eq!(query_value("app://cb?code=OC-1%2B2&state=s#frag", "code").as_deref(), Some("OC-1+2"));
        assert_eq!(query_value("app://cb?state=s", "code"), None);
        assert_eq!(https_host("https://SSO.Example.org:443/x"), Some("sso.example.org:443".into()));
        assert_eq!(https_host("https://user@example.org/"), None);
        assert_eq!(https_host("http://example.org/"), None);
        assert_eq!(base64url("eyJzdWIiOiJhYmMifQ").as_deref(), Some(&br#"{"sub":"abc"}"#[..]));
        assert_eq!(base64url("e$"), None);
        for bytes in [&b""[..], b"f", b"fo", b"foo", b"foob", &[0xfb, 0xff, 0xfe]] {
            assert_eq!(base64url(&base64url_encode(bytes)).as_deref(), Some(bytes));
        }
        assert_eq!(base64url_encode(&[0xfb, 0xff, 0xfe]), "-__-");
        assert_eq!(base64url_encode(b"foob"), "Zm9vYg");
    }
}
