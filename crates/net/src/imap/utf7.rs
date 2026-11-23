// Adapted from MailCraft (https://github.com/ZIDAN44/mailcraft), commit c3510d13, crates/model/src/encoding/utf7.rs. MIT OR Apache-2.0.
// Changed: decoding only (MailALM sends the names the server listed, never encodes one); the tests keep to decoding.

//! IMAP's modified UTF-7 for mailbox names (RFC 3501 §5.1.3).
//!
//! Printable ASCII stands for itself except `&`, written `&-`. Other characters are UTF-16BE,
//! base64-encoded with `,` instead of `/`, between `&` and `-`.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+,";

fn b64_value(c: u8) -> Option<u32> {
    ALPHABET.iter().position(|&a| a == c).and_then(|p| u32::try_from(p).ok())
}

/// Decode a mailbox name. Malformed sections are kept as written rather than dropped, so a
/// broken name still shows something recognisable.
pub(super) fn decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(rest.get(..amp).unwrap_or_default());
        let after = rest.get(amp + 1..).unwrap_or_default();
        let Some(dash) = after.find('-') else {
            out.push_str(rest.get(amp..).unwrap_or_default());
            return out;
        };
        let chunk = after.get(..dash).unwrap_or_default();
        if chunk.is_empty() {
            out.push('&');
        } else if let Some(text) = decode_chunk(chunk) {
            out.push_str(&text);
        } else {
            out.push('&');
            out.push_str(chunk);
            out.push('-');
        }
        rest = after.get(dash + 1..).unwrap_or_default();
    }
    out.push_str(rest);
    out
}

fn decode_chunk(chunk: &str) -> Option<String> {
    let mut bits: u32 = 0;
    let mut nbits = 0u32;
    let mut units = Vec::new();
    for c in chunk.bytes() {
        bits = (bits << 6) | b64_value(c)?;
        nbits += 6;
        if nbits >= 16 {
            nbits -= 16;
            units.push(((bits >> nbits) & 0xFFFF) as u16);
        }
        bits &= (1 << nbits) - 1;
    }
    // Too short for one character (`R&D-Team`, a name sent unencoded): not an encoded run.
    if units.is_empty() && !chunk.is_empty() {
        return None;
    }
    String::from_utf16(&units).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc_examples() {
        // RFC 3501 §5.1.3: "~peter/mail/台北/日本語"
        assert_eq!(decode("~peter/mail/&U,BTFw-/&ZeVnLIqe-"), "~peter/mail/台北/日本語");
        assert_eq!(decode("Tom &- Jerry"), "Tom & Jerry");
        assert_eq!(decode("&AMk-l&AOk-ments envoy&AOk-s"), "Éléments envoyés");
        assert_eq!(decode("plain"), "plain");
        assert_eq!(decode(""), "");
    }

    #[test]
    fn malformed_input_is_kept() {
        assert_eq!(decode("bad &!!- name"), "bad &!!- name");
        assert_eq!(decode("open &AOk"), "open &AOk");
        // A lone surrogate is no character.
        assert_eq!(decode("&2D0-"), "&2D0-");
    }

    /// A name sent unencoded, its `&` taken for the start of an encoded run too short to hold a
    /// character: `R&D-Team` lost its `&D-`.
    #[test]
    fn a_run_too_short_for_a_character_is_kept() {
        assert_eq!(decode("R&D-Team"), "R&D-Team");
        assert_eq!(decode("&AOk-"), "é");
    }
}
