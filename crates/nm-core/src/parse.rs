//! Turning RFC 822 source into what `list` and `show` print. Never panics on bad mail.

use chrono::{DateTime, Utc};
use mail_parser::{Address, MessageParser, MessagePart, MimeHeaders};
use serde::Serialize;

/// Width that HTML bodies are wrapped to when converted to text.
const HTML_WRAP: usize = 80;

/// A sender or recipient.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EmailAddress {
    pub name: Option<String>,
    pub address: Option<String>,
}

impl EmailAddress {
    /// `Name <address>`, or whichever part exists.
    pub fn display(&self) -> String {
        match (&self.name, &self.address) {
            (Some(n), Some(a)) => format!("{n} <{a}>"),
            (Some(n), None) => n.clone(),
            (None, Some(a)) => a.clone(),
            (None, None) => String::new(),
        }
    }

    /// The name if there is one, else the address: what a one-line list shows.
    pub fn short(&self) -> &str {
        self.name
            .as_deref()
            .or(self.address.as_deref())
            .unwrap_or("")
    }
}

/// The few fields the cache stores per message so `list` never parses mail.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    pub date: Option<DateTime<Utc>>,
    pub from: Option<EmailAddress>,
    pub subject: Option<String>,
}

/// Which part the body text came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum BodyKind {
    #[serde(rename = "text/plain")]
    Plain,
    /// HTML converted to text.
    #[serde(rename = "text/html")]
    Html,
    /// The message has no text body.
    #[serde(rename = "none")]
    None,
}

/// An attachment, listed but not saved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Attachment {
    pub name: Option<String>,
    pub content_type: String,
    /// Decoded size in bytes.
    pub size: usize,
}

/// A message as `nmail show` presents it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedMessage {
    pub date: Option<DateTime<Utc>>,
    pub from: Vec<EmailAddress>,
    pub to: Vec<EmailAddress>,
    pub cc: Vec<EmailAddress>,
    pub subject: Option<String>,
    pub body_kind: BodyKind,
    pub body: String,
    pub attachments: Vec<Attachment>,
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("the message is empty or is not RFC 822 mail")]
    NotMail,
}

/// Extracts the list fields. Bad or empty mail gives an empty summary, not an error.
pub fn summarize(raw: &[u8]) -> Summary {
    let Some(msg) = MessageParser::default().parse(raw) else {
        return Summary::default();
    };
    Summary {
        date: msg.date().and_then(|d| to_utc(d.to_timestamp())),
        from: msg
            .from()
            .and_then(|a| addresses(Some(a)).into_iter().next()),
        subject: msg.subject().map(str::to_owned),
    }
}

/// Parses a whole message for display.
pub fn parse(raw: &[u8]) -> Result<ParsedMessage, ParseError> {
    let msg = MessageParser::default()
        .parse(raw)
        .ok_or(ParseError::NotMail)?;

    let plain: Vec<&str> = msg
        .text_bodies()
        .filter(|p| p.is_text() && !p.is_text_html())
        .filter_map(MessagePart::text_contents)
        .collect();
    let (body_kind, body) = if !plain.is_empty() {
        (BodyKind::Plain, plain.join("\n"))
    } else if let Some(html) = msg
        .html_bodies()
        .find(|p| p.is_text_html())
        .and_then(MessagePart::text_contents)
    {
        // Mail uses tables for layout, so their borders are noise. A conversion failure falls
        // back to the HTML source rather than hiding the body.
        let text = html2text::config::plain()
            .no_table_borders()
            .string_from_read(html.as_bytes(), HTML_WRAP)
            .unwrap_or_else(|_| html.into());
        let trimmed: Vec<&str> = text.lines().map(str::trim_end).collect();
        (BodyKind::Html, trimmed.join("\n"))
    } else {
        (BodyKind::None, String::new())
    };

    let attachments = msg
        .attachments()
        .map(|part| Attachment {
            name: part.attachment_name().map(str::to_owned),
            content_type: part
                .content_type()
                .map(|ct| match ct.subtype() {
                    Some(sub) => format!("{}/{}", ct.ctype(), sub),
                    None => ct.ctype().to_owned(),
                })
                .unwrap_or_else(|| "application/octet-stream".to_owned())
                .to_ascii_lowercase(),
            size: part.len(),
        })
        .collect();

    Ok(ParsedMessage {
        date: msg.date().and_then(|d| to_utc(d.to_timestamp())),
        from: addresses(msg.from()),
        to: addresses(msg.to()),
        cc: addresses(msg.cc()),
        subject: msg.subject().map(str::to_owned),
        body_kind,
        body,
        attachments,
    })
}

fn addresses(header: Option<&Address<'_>>) -> Vec<EmailAddress> {
    header
        .map(|a| {
            a.iter()
                .map(|addr| EmailAddress {
                    name: addr.name().map(str::to_owned).filter(|s| !s.is_empty()),
                    address: addr.address().map(str::to_owned).filter(|s| !s.is_empty()),
                })
                .filter(|e| e.name.is_some() || e.address.is_some())
                .collect()
        })
        .unwrap_or_default()
}

fn to_utc(timestamp: i64) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp(timestamp, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN: &[u8] = b"From: Ada Lovelace <ada@mail.example>\r\nTo: bob@mail.example\r\n\
Cc: Carol <carol@mail.example>, dan@mail.example\r\nSubject: Hello there\r\n\
Date: Sat, 10 Oct 2026 09:30:00 +0100\r\n\r\nPlain body.\r\n";

    const HTML_ONLY: &[u8] = b"From: news@mail.example\r\nSubject: News\r\n\
Content-Type: text/html; charset=utf-8\r\n\r\n<html><body><p>Hello <b>world</b></p></body></html>\r\n";

    const WITH_ATTACHMENT: &[u8] = b"From: a@mail.example\r\nSubject: Report\r\n\
MIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"b1\"\r\n\r\n\
--b1\r\nContent-Type: text/plain\r\n\r\nSee attached.\r\n\
--b1\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=\"report.pdf\"\r\n\
Content-Transfer-Encoding: base64\r\n\r\naGVsbG8=\r\n--b1--\r\n";

    #[test]
    fn plain_message_fields() {
        let m = parse(PLAIN).unwrap();
        assert_eq!(m.subject.as_deref(), Some("Hello there"));
        assert_eq!(m.from[0].display(), "Ada Lovelace <ada@mail.example>");
        assert_eq!(m.to[0].display(), "bob@mail.example");
        assert_eq!(m.cc.len(), 2);
        assert_eq!(m.body_kind, BodyKind::Plain);
        assert_eq!(m.body.trim(), "Plain body.");
        assert_eq!(
            m.date.map(|d| d.to_rfc3339()),
            Some("2026-10-10T08:30:00+00:00".into())
        );
        assert!(m.attachments.is_empty());
    }

    #[test]
    fn html_only_is_converted() {
        let m = parse(HTML_ONLY).unwrap();
        assert_eq!(m.body_kind, BodyKind::Html);
        assert!(m.body.contains("Hello"), "{}", m.body);
        assert!(!m.body.contains("<p>"), "{}", m.body);
    }

    #[test]
    fn attachments_are_listed() {
        let m = parse(WITH_ATTACHMENT).unwrap();
        assert_eq!(m.body.trim(), "See attached.");
        assert_eq!(
            m.attachments,
            vec![Attachment {
                name: Some("report.pdf".into()),
                content_type: "application/pdf".into(),
                size: 5,
            }]
        );
    }

    #[test]
    fn summary_of_plain_message() {
        let s = summarize(PLAIN);
        assert_eq!(s.from.unwrap().short(), "Ada Lovelace");
        assert_eq!(s.subject.as_deref(), Some("Hello there"));
        assert!(s.date.is_some());
    }

    #[test]
    fn bad_mail_does_not_panic() {
        assert!(parse(b"").is_err());
        assert_eq!(summarize(b""), Summary::default());
        for junk in [
            &b"\xff\xfe\x00garbage"[..],
            b"Subject: =?utf-8?B?!!!?=\r\nDate: not a date\r\n\r\n",
            b"Content-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\n--x",
        ] {
            let _ = parse(junk);
            let _ = summarize(junk);
        }
    }
}
