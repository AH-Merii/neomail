//! IMAP's modified UTF-7 for mailbox names (RFC 3501 section 5.1.3,
//! <https://www.rfc-editor.org/rfc/rfc3501#section-5.1.3>): printable US-ASCII stands for
//! itself, except `&`, written `&-`; any other run of characters is `&`, its UTF-16 in base64
//! with `,` for `/` and no padding, then `-`.

/// Modified BASE64 (RFC 2152's, with `,` instead of `/`).
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+,";

fn printable(c: char) -> bool {
    (' '..='~').contains(&c)
}

/// The name a server sent, decoded; `None` when it is not valid modified UTF-7: a byte outside
/// printable US-ASCII, a `&` without its `-`, bad base64 or UTF-16, printable US-ASCII encoded
/// ("MUST NOT"), or a null shift, an encoded run straight after another ("not permitted", both
/// RFC 3501 section 5.1.3), so each name has one encoding.
pub(crate) fn decode(name: &str) -> Option<String> {
    let mut out = String::with_capacity(name.len());
    let mut rest = name;
    let mut after_run = false;
    while let Some(c) = rest.chars().next() {
        if !printable(c) {
            return None;
        }
        if c != '&' {
            out.push(c);
            rest = &rest[1..];
            after_run = false;
            continue;
        }
        let end = rest.find('-')?;
        let encoded = &rest[1..end];
        rest = &rest[end + 1..];
        if encoded.is_empty() {
            out.push('&');
            after_run = false;
            continue;
        }
        if after_run {
            return None;
        }
        for c in char::decode_utf16(utf16(encoded)?) {
            let c = c.ok()?;
            if printable(c) {
                return None;
            }
            out.push(c);
        }
        after_run = true;
    }
    Some(out)
}

/// A name for display: decoded, or as the server sent it when it is not valid modified UTF-7.
pub(crate) fn display(name: &str) -> String {
    decode(name).unwrap_or_else(|| name.to_owned())
}

/// The UTF-16 code units of one base64 run, or `None` when it is not whole units with zero
/// padding bits.
fn utf16(encoded: &str) -> Option<Vec<u16>> {
    let (mut bits, mut count) = (0u32, 0u32);
    let mut bytes = Vec::new();
    for b in encoded.bytes() {
        let value = ALPHABET.iter().position(|&a| a == b)?;
        bits = (bits << 6) | u32::try_from(value).ok()?;
        count += 6;
        if count >= 8 {
            count -= 8;
            bytes.push(u8::try_from(bits >> count).ok()?);
            bits &= (1 << count) - 1;
        }
    }
    if count >= 6 || bits != 0 || bytes.len() % 2 != 0 {
        return None;
    }
    Some(
        bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&pair| u16::from_be_bytes(pair))
            .collect(),
    )
}

/// `name` in modified UTF-7. Only the tests encode: a move sends the name LIST sent.
#[cfg(test)]
fn encode(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut run: Vec<u16> = Vec::new();
    for c in name.chars() {
        if printable(c) {
            flush(&mut out, &mut run);
            out.push(c);
            if c == '&' {
                out.push('-');
            }
        } else {
            run.extend_from_slice(c.encode_utf16(&mut [0; 2]));
        }
    }
    flush(&mut out, &mut run);
    out
}

#[cfg(test)]
fn flush(out: &mut String, run: &mut Vec<u16>) {
    if run.is_empty() {
        return;
    }
    out.push('&');
    let (mut bits, mut count) = (0u32, 0u32);
    for byte in run.drain(..).flat_map(u16::to_be_bytes) {
        bits = (bits << 8) | u32::from(byte);
        count += 8;
        while count >= 6 {
            count -= 6;
            out.push(char::from(ALPHABET[((bits >> count) & 63) as usize]));
        }
        bits &= (1 << count) - 1;
    }
    if count > 0 {
        out.push(char::from(ALPHABET[((bits << (6 - count)) & 63) as usize]));
    }
    out.push('-');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc_3501_s_example_decodes_and_encodes_back() {
        let wire = "~peter/mail/&U,BTFw-/&ZeVnLIqe-";
        let name = "~peter/mail/\u{53F0}\u{5317}/\u{65E5}\u{672C}\u{8A9E}";
        assert_eq!(decode(wire).as_deref(), Some(name));
        assert_eq!(encode(name), wire);
    }

    #[test]
    fn ampersand_is_written_ampersand_dash() {
        assert_eq!(encode("Q&A"), "Q&-A");
        assert_eq!(decode("Q&-A").as_deref(), Some("Q&A"));
    }

    #[test]
    fn names_round_trip() {
        for name in [
            "INBOX",
            "Folders/Receipts",
            "[Gmail]/All Mail",
            "Re\u{E7}us",
            "Entw\u{FC}rfe & Notizen",
            "\u{1F4E7} mail",
            "a\u{E9}\u{E9}\u{E9}b",
            "\u{E9}&\u{E9}",
            "",
        ] {
            let wire = encode(name);
            assert!(wire.chars().all(printable), "{wire}");
            assert_eq!(decode(&wire).as_deref(), Some(name), "{wire}");
        }
    }

    #[test]
    fn a_malformed_name_does_not_decode_and_is_shown_raw() {
        for wire in [
            "Re&AOc",     // no closing dash
            "&AGE-",      // "a", printable ASCII encoded
            "&2D0-",      // a lone surrogate
            "&AO-",       // not a whole UTF-16 unit
            "&AOd-",      // nonzero padding bits
            "&AO*-",      // not base64
            "caf\u{E9}",  // not ASCII
            "tab\there",  // a control character
            "&-&&-",      // an empty run, then a bad one
            "&AOk-&AOk-", // a null shift: "\u{E9}\u{E9}" is one run, "&AOkA6Q-"
        ] {
            assert_eq!(decode(wire), None, "{wire}");
            assert_eq!(display(wire), wire);
        }
    }
}
