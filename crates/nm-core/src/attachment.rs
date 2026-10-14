//! A cached message's attachments, offline: what each is (its declared type and its real type,
//! told from its content), the safe name it is saved under, whether an HTML body shows it
//! inline, and whether opening it could run code. The cache holds every message's whole MIME
//! source, so nothing is fetched.

pub mod name;
pub mod sniff;

use mail_parser::decoders::base64::base64_decode;
use mail_parser::decoders::quoted_printable::quoted_printable_decode;
use mail_parser::{Encoding, MessageParser, MessagePart, MimeHeaders};
use serde::Serialize;

use crate::parse::ParseError;

/// One attachment, as `neomail attachment list` describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Info {
    /// Its position among the message's attachments, from 1: what `save` and `open` take.
    pub index: usize,
    /// The name the sender gave it, as sent.
    pub name: Option<String>,
    /// The name `save` and `open` write it under ([`name::safe_name`]), unique in the message.
    pub safe_name: String,
    /// The type the sender declared, lower case.
    pub declared_type: String,
    /// The type its content shows ([`sniff::real_type`]); `message/rfc822` for a nested message.
    pub real_type: String,
    /// Whether the real type contradicts the declared type or the name's extension.
    pub type_mismatch: bool,
    /// Decoded size in bytes.
    pub size: usize,
    /// Whether an HTML body of the message shows it, through a `cid:` reference.
    pub inline: bool,
    /// Its Content-ID, without angle brackets.
    pub content_id: Option<String>,
    /// Whether it is an executable or a script, by real type or by its safe name's extension.
    pub executable: bool,
}

/// An attachment and its decoded content.
#[derive(Debug, Clone)]
pub struct Extracted {
    pub info: Info,
    pub data: Vec<u8>,
}

/// Every attachment of the message `raw`, in order. A nested message (`message/rfc822`) is one
/// attachment, its own attachments inside it.
pub fn extract(raw: &[u8]) -> Result<Vec<Extracted>, ParseError> {
    let msg = MessageParser::default()
        .parse(raw)
        .ok_or(ParseError::NotMail)?;
    let html: Vec<String> = msg
        .html_bodies()
        .filter_map(MessagePart::text_contents)
        .map(str::to_ascii_lowercase)
        .collect();
    let mut taken: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for (i, part) in msg.attachments().enumerate() {
        let index = i + 1;
        let data = content(msg.raw_message(), part);
        let name = part.attachment_name().map(str::to_owned);
        let declared_type = declared_type(part);
        let real_type = if part.is_message() {
            sniff::MESSAGE
        } else {
            sniff::real_type(&data)
        };
        let mut safe_name = name::safe_name(name.as_deref(), index, real_type);
        let is_taken = |n: &str, taken: &[String]| taken.iter().any(|t| t.eq_ignore_ascii_case(n));
        if is_taken(&safe_name, &taken) {
            let first = name::numbered(&safe_name, index);
            safe_name = first.clone();
            let mut n = 2;
            while is_taken(&safe_name, &taken) {
                safe_name = name::numbered(&first, n);
                n += 1;
            }
        }
        taken.push(safe_name.clone());
        let type_mismatch = !sniff::fits(&declared_type, real_type)
            || sniff::extension_type(&safe_name).is_some_and(|t| !sniff::fits(t, real_type));
        let content_id = part
            .content_id()
            .map(|id| {
                id.trim()
                    .trim_start_matches('<')
                    .trim_end_matches('>')
                    .to_owned()
            })
            .filter(|id| !id.is_empty());
        let inline = content_id.as_deref().is_some_and(|id| {
            let reference = format!("cid:{}", id.to_ascii_lowercase());
            html.iter().any(|h| h.contains(&reference))
        });
        let executable = sniff::is_risky(real_type, &safe_name);
        out.push(Extracted {
            info: Info {
                index,
                name,
                safe_name,
                declared_type,
                real_type: real_type.to_owned(),
                type_mismatch,
                size: data.len(),
                inline,
                content_id,
                executable,
            },
            data,
        });
    }
    Ok(out)
}

/// The attachment's bytes as they were sent. mail-parser turns a text part (any `text/*`, and a
/// part without a Content-Type) into a UTF-8 string, converting its charset and replacing bytes
/// that are not UTF-8, which would corrupt a binary file sent that way. For such a part only the
/// Content-Transfer-Encoding is undone, on its raw body; the charset is left as it is.
fn content(raw: &[u8], part: &MessagePart<'_>) -> Vec<u8> {
    if !part.is_text() {
        return part.contents().to_vec();
    }
    let body = raw
        .get(part.raw_body_offset() as usize..part.raw_end_offset() as usize)
        .unwrap_or_default();
    let decoded = match part.encoding {
        Encoding::Base64 => base64_decode(body),
        Encoding::QuotedPrintable => quoted_printable_decode(body),
        Encoding::None => Some(body.to_vec()),
    };
    decoded.unwrap_or_else(|| part.contents().to_vec())
}

/// The part's Content-Type, lower case, or `application/octet-stream` without one (RFC 2046
/// section 4.5.1 makes that the default for an attachment).
fn declared_type(part: &MessagePart<'_>) -> String {
    part.content_type()
        .map(|ct| match ct.subtype() {
            Some(sub) => format!("{}/{}", ct.ctype(), sub),
            None => ct.ctype().to_owned(),
        })
        .unwrap_or_else(|| sniff::UNKNOWN.to_owned())
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests;
