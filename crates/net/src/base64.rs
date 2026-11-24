// Adapted from MailCraft (https://github.com/ZIDAN44/mailcraft), commit c3510d13, crates/model/src/encoding/base64.rs (`encode`; `b64url` from crates/net/src/oauth.rs). MIT OR Apache-2.0.
// Changed: moved here from imap/sign_in.rs so that IMAP, OAuth and Graph share it; base64url written directly, not by replacing characters; a strict decoder of our own for an ID token's claims.

//! Base64 (RFC 4648): the standard alphabet with padding (SASL PLAIN, Graph's MIME bodies) and
//! the URL-safe one without (PKCE, an ID token's parts).

const STANDARD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const URL_SAFE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn encode_with(data: &[u8], alphabet: &[u8; 64], pad: bool) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3).saturating_mul(4));
    for chunk in data.chunks(3) {
        let b = [chunk.first().copied().unwrap_or(0), chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        // A chunk of k bytes gives k + 1 characters; `=` fills the group of four.
        for (i, shift) in [18u32, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(char::from(alphabet.get(((n >> shift) & 63) as usize).copied().unwrap_or(b'A')));
            } else if pad {
                out.push('=');
            }
        }
    }
    out
}

/// `data` in base64 (RFC 4648 §4): the standard alphabet, padded with `=`, on one line.
pub(crate) fn encode(data: &[u8]) -> String {
    encode_with(data, STANDARD, true)
}

/// `data` in base64url without padding (RFC 4648 §5), as PKCE (RFC 7636) and JWTs use it.
pub(crate) fn encode_url(data: &[u8]) -> String {
    encode_with(data, URL_SAFE, false)
}

/// The bytes of base64 or base64url text, padded or not; `None` for anything else (a character
/// of neither alphabet, a lone trailing character, padding in the middle, bits left over).
pub(crate) fn decode(text: &str) -> Option<Vec<u8>> {
    let text = text.trim_end_matches('=');
    let value = |c: u8| -> Option<u32> {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        };
        Some(u32::from(v))
    };
    let mut out = Vec::with_capacity(text.len() / 4 * 3 + 3);
    for group in text.as_bytes().chunks(4) {
        if group.len() == 1 {
            return None;
        }
        let mut n = 0u32;
        for (i, c) in group.iter().enumerate() {
            n |= value(*c)? << (18 - 6 * i as u32);
        }
        let [_, a, b, c] = n.to_be_bytes();
        let bytes = [a, b, c];
        let keep = group.len() - 1;
        // The bits past the last whole byte must be zero (a canonical encoding).
        if bytes.get(keep..).is_some_and(|rest| rest.iter().any(|x| *x != 0)) {
            return None;
        }
        out.extend_from_slice(bytes.get(..keep)?);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 4648 §10's test vectors, and every byte value.
    #[test]
    fn base64_rfc_vectors() {
        for (plain, coded) in
            [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")]
        {
            assert_eq!(encode(plain.as_bytes()), coded, "{plain}");
            assert_eq!(decode(coded).unwrap(), plain.as_bytes(), "{coded}");
            assert_eq!(decode(&encode_url(plain.as_bytes())).unwrap(), plain.as_bytes(), "{plain}");
        }
        assert_eq!(encode(b"\0user\0pass"), "AHVzZXIAcGFzcw==");
        assert_eq!(encode(&[0xFB, 0xFF, 0xBF]), "+/+/");
        assert_eq!(encode_url(&[0xFB, 0xFF, 0xBF]), "-_-_");
        assert_eq!(encode_url(b"f"), "Zg");
        let all: Vec<u8> = (0..=255).collect();
        assert_eq!(encode(&all).len(), 344);
        assert_eq!(decode(&encode(&all)).unwrap(), all);
        assert_eq!(decode(&encode_url(&all)).unwrap(), all);
    }

    #[test]
    fn junk_is_not_decoded() {
        for junk in ["Z", "Zh==", "Zm9v!", "Zm 9v", "Zm=9v", "é"] {
            assert_eq!(decode(junk), None, "{junk}");
        }
    }
}
