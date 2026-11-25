//! Base64 (RFC 4648 §4: standard alphabet, padded), for the WebSocket handshake's key and accept
//! value, and the check of Antidote's correction memory.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// `data` in base64, padded with `=`, on one line.
pub(crate) fn encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3).saturating_mul(4));
    for chunk in data.chunks(3) {
        let byte = |i: usize| u32::from(chunk.get(i).copied().unwrap_or(0));
        let n = (byte(0) << 16) | (byte(1) << 8) | byte(2);
        // A chunk of k bytes gives k + 1 characters; `=` fills the group of four.
        for (i, shift) in [18u32, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                let index = ((n >> shift) & 63) as usize;
                out.push(char::from(ALPHABET.get(index).copied().unwrap_or(b'A')));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Whether `text` is made of base64 characters only (the alphabet and `=`): what Antidote's
/// correction memory must be before MailALM keeps it or gives it back.
pub(crate) fn is_base64_text(text: &str) -> bool {
    text.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc_4648_vectors() {
        let cases = [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")];
        for (plain, encoded) in cases {
            assert_eq!(encode(plain.as_bytes()), encoded);
        }
        assert_eq!(encode(&[0xFB, 0xFF]), "+/8=");
    }

    #[test]
    fn base64_text() {
        assert!(is_base64_text("QUJD+/=="));
        assert!(is_base64_text(""));
        assert!(!is_base64_text("QU JD"));
        assert!(!is_base64_text("QUJD\n"));
        assert!(!is_base64_text("QUJD-_"));
    }
}
