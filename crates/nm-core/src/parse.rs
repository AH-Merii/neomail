//! Turning RFC 822 source into what `list` and `show` print. Never panics on bad mail.

use chrono::{DateTime, Utc};
use mail_parser::{Address, Message, MessageParser, MessagePart, MimeHeaders};
use serde::Serialize;
use sha2::{Digest, Sha256};

/// Width that HTML bodies are wrapped to when converted to text.
const HTML_WRAP: usize = 80;

/// Above this size or nesting depth, HTML is not laid out (html2text's work grows faster than
/// linearly with nesting, so hostile mail could stall `neomail show`); its tags are stripped instead.
const HTML_MAX_BYTES: usize = 1 << 20;
const HTML_MAX_DEPTH: usize = 100;

/// The first line of a body whose HTML was only stripped of tags.
pub const HTML_STRIPPED_NOTE: &str =
    "[neomail: this HTML is too large or too deeply nested to lay out; showing its text only]";

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
    /// The Message-ID header, without its angle brackets.
    pub message_id: Option<String>,
}

/// The key a classification result is stored under (with the account): the Message-ID when the
/// message has one, else `sha256:` and the hex digest of the whole source. Unlike the cache's
/// own id, it stays the same when a mailbox is rebuilt and the message fetched again.
pub fn msg_key(message_id: Option<&str>, raw: &[u8]) -> String {
    match message_id.map(str::trim).filter(|id| !id.is_empty()) {
        Some(id) => id.to_owned(),
        None => {
            let digest = Sha256::digest(raw);
            let mut key = String::with_capacity(7 + digest.len() * 2);
            key.push_str("sha256:");
            for byte in digest {
                key.push_str(&format!("{byte:02x}"));
            }
            key
        }
    }
}

/// The headers that tell bulk, automated and reply mail apart, as classification reads them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Headers {
    /// Message-ID, without its angle brackets.
    pub message_id: Option<String>,
    pub list_unsubscribe: bool,
    pub list_id: bool,
    /// Auto-Submitted's value (RFC 3834), trimmed.
    pub auto_submitted: Option<String>,
    /// Precedence's value, trimmed.
    pub precedence: Option<String>,
    pub in_reply_to: bool,
}

impl Headers {
    /// List-Unsubscribe or List-Id is present (RFC 2369, RFC 2919).
    pub fn mailing_list(&self) -> bool {
        self.list_unsubscribe || self.list_id
    }

    /// Auto-Submitted is present and not `no` (RFC 3834 section 5), or Precedence is `bulk`,
    /// `list` or `junk`. Compared without case; a parameter after `;` is ignored.
    pub fn automated(&self) -> bool {
        let keyword = |v: &str| {
            v.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        };
        let auto = self
            .auto_submitted
            .as_deref()
            .is_some_and(|v| keyword(v) != "no");
        let bulk = self
            .precedence
            .as_deref()
            .is_some_and(|v| matches!(keyword(v).as_str(), "bulk" | "list" | "junk"));
        auto || bulk
    }

    /// In-Reply-To is present.
    pub fn reply(&self) -> bool {
        self.in_reply_to
    }

    fn of(msg: &Message<'_>) -> Self {
        let present = |name: &str| msg.header_raw(name).is_some();
        let value = |name: &str| {
            msg.header_raw(name)
                .map(|v| v.split_whitespace().collect::<Vec<_>>().join(" "))
        };
        Headers {
            message_id: msg.message_id().map(str::to_owned),
            list_unsubscribe: present("List-Unsubscribe"),
            list_id: present("List-Id"),
            auto_submitted: value("Auto-Submitted"),
            precedence: value("Precedence"),
            in_reply_to: present("In-Reply-To"),
        }
    }
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

/// A message as `neomail show` presents it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedMessage {
    pub date: Option<DateTime<Utc>>,
    pub from: Vec<EmailAddress>,
    pub to: Vec<EmailAddress>,
    pub cc: Vec<EmailAddress>,
    /// Reply-To: not shown by `neomail show`, but its display names are known names for
    /// redaction.
    pub reply_to: Vec<EmailAddress>,
    pub subject: Option<String>,
    pub body_kind: BodyKind,
    pub body: String,
    pub attachments: Vec<Attachment>,
    pub headers: Headers,
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
        message_id: msg.message_id().map(str::to_owned),
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
        (BodyKind::Html, html_to_text(html))
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
        reply_to: addresses(msg.reply_to()),
        subject: msg.subject().map(str::to_owned),
        body_kind,
        body,
        attachments,
        headers: Headers::of(&msg),
    })
}

/// Converts an HTML body to text, with a bounded amount of work.
fn html_to_text(html: &str) -> String {
    let text = html_layout(html)
        .unwrap_or_else(|| format!("{HTML_STRIPPED_NOTE}\n\n{}", strip_tags(html)));
    let trimmed: Vec<&str> = text.lines().map(str::trim_end).collect();
    trimmed.join("\n")
}

/// Lays `html` out as text, or `None` when it is too large or its parsed DOM too deep to lay out
/// in bounded time. The depth is measured on html5ever's tree, not on the tags in the source,
/// because the parser ignores stray end tags and reopens formatting elements on its own.
fn html_layout(html: &str) -> Option<String> {
    if html.len() > HTML_MAX_BYTES {
        return None;
    }
    let dom = parse_bounded(html)?;
    // Mail uses tables for layout, so their borders are noise. A conversion failure falls back
    // to the HTML source rather than hiding the body.
    let config = html2text::config::plain().no_table_borders();
    let laid_out = config
        .dom_to_render_tree(&dom)
        .and_then(|tree| config.render_to_string(tree, HTML_WRAP));
    Some(laid_out.unwrap_or_else(|_| html.into()))
}

/// Parses `html` as html2text would, or gives up with `None` once the tree nests deeper than
/// `HTML_MAX_DEPTH`. html5ever's own work also grows with nesting (its scope checks walk the
/// stack of open elements), so the input is fed in chunks and the depth checked after each one:
/// a hostile body costs at most one chunk's worth of deep parsing.
fn parse_bounded(html: &str) -> Option<html2text::RcDom> {
    use html5ever::tendril::{StrTendril, TendrilSink};
    const CHUNK: usize = 16 << 10;

    let opts = html5ever::ParseOpts {
        tree_builder: html5ever::tree_builder::TreeBuilderOpts {
            scripting_enabled: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut parser = html5ever::parse_document(html2text::RcDom::default(), opts);
    let mut rest = html;
    while !rest.is_empty() {
        let mut end = rest.len().min(CHUNK);
        while !rest.is_char_boundary(end) {
            end += 1;
        }
        let (chunk, tail) = rest.split_at(end);
        parser.process(StrTendril::from_slice(chunk));
        if dom_depth(&parser.tokenizer.sink.sink.document) > HTML_MAX_DEPTH {
            return None;
        }
        rest = tail;
    }
    let dom = parser.finish();
    (dom_depth(&dom.document) <= HTML_MAX_DEPTH).then_some(dom)
}

/// How deeply the elements under `root` nest, counted without recursion so a hostile tree cannot
/// overflow the stack. Stops counting once past `HTML_MAX_DEPTH`.
fn dom_depth(root: &html2text::Handle) -> usize {
    let mut max = 0;
    let mut stack = vec![(root.clone(), 0usize)];
    while let Some((node, depth)) = stack.pop() {
        let depth = match node.data {
            html2text::Element { .. } => depth + 1,
            _ => depth,
        };
        max = max.max(depth);
        if max > HTML_MAX_DEPTH {
            break;
        }
        stack.extend(node.children.borrow().iter().map(|c| (c.clone(), depth)));
    }
    max
}

/// The text between tags, with the commonest entities decoded and blank runs collapsed: the
/// fallback for HTML too large or deep to lay out.
fn strip_tags(html: &str) -> String {
    let mut text = String::with_capacity(html.len() / 2);
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => {
                in_tag = true;
                text.push(' ');
            }
            '>' if in_tag => in_tag = false,
            c if !in_tag => text.push(c),
            _ => {}
        }
    }
    let text = text
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        if !words.is_empty() {
            out.push_str(&words.join(" "));
            out.push('\n');
        }
    }
    out
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
        assert!(m.reply_to.is_empty());
    }

    #[test]
    fn reply_to_is_parsed_with_its_names() {
        let raw = b"From: a@mail.example\r\nReply-To: Desk <desk@mail.example>, b@mail.example\r\n\
Subject: Hi\r\n\r\nBody.\r\n";
        let m = parse(raw).unwrap();
        assert_eq!(
            m.reply_to
                .iter()
                .map(EmailAddress::display)
                .collect::<Vec<_>>(),
            ["Desk <desk@mail.example>", "b@mail.example"]
        );
    }

    #[test]
    fn html_only_is_converted() {
        let m = parse(HTML_ONLY).unwrap();
        assert_eq!(m.body_kind, BodyKind::Html);
        assert!(m.body.contains("Hello"), "{}", m.body);
        assert!(!m.body.contains("<p>"), "{}", m.body);
    }

    fn html_mail(html: &str) -> String {
        format!("From: a@mail.example\r\nContent-Type: text/html; charset=utf-8\r\n\r\n{html}\r\n")
    }

    /// Parses `html` as a mail body within two seconds and checks it was only stripped.
    fn assert_stripped_quickly(html: &str) {
        let start = std::time::Instant::now();
        let m = parse(html_mail(html).as_bytes()).unwrap();
        let elapsed = start.elapsed();
        assert!(elapsed < std::time::Duration::from_secs(2), "{elapsed:?}");
        assert_eq!(m.body_kind, BodyKind::Html);
        assert!(m.body.starts_with(HTML_STRIPPED_NOTE), "{}", m.body);
        assert!(m.body.contains("deep text"), "{}", m.body);
        assert!(!m.body.contains('<'), "{}", m.body);
    }

    #[test]
    fn deeply_nested_html_is_stripped_quickly() {
        let depth = 50_000;
        let html = format!(
            "{}deep text{}",
            "<div>".repeat(depth),
            "</div>".repeat(depth)
        );
        assert_stripped_quickly(&html);
    }

    #[test]
    fn stray_end_tags_do_not_hide_nesting() {
        // The parser ignores an end tag that matches no open element, so these nest 50,000 deep.
        assert_stripped_quickly(&("<div></b>".repeat(50_000) + "deep text"));
        assert_stripped_quickly(&("<span></x>".repeat(50_000) + "deep text"));
    }

    #[test]
    fn long_flat_html_is_still_laid_out() {
        let html = "<p>line<br>".repeat(500) + "<img src=x><table><tr><td>cell</td></tr></table>";
        let m = parse(html_mail(&html).as_bytes()).unwrap();
        assert!(!m.body.starts_with(HTML_STRIPPED_NOTE), "{}", m.body);
        assert!(m.body.contains("cell"), "{}", m.body);
        let dom = html2text::config::plain()
            .parse_html("<div><div><span>x</span></div></div>".as_bytes())
            .unwrap();
        // html, body, then the three written elements.
        assert_eq!(dom_depth(&dom.document), 5);
        assert_eq!(strip_tags("<b>a &amp; b</b>\n\n<i>c</i>"), "a & b\nc\n");
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

    fn headers(extra: &str) -> Headers {
        let raw = format!("From: a@mail.example\r\n{extra}Subject: s\r\n\r\nbody\r\n");
        parse(raw.as_bytes()).unwrap().headers
    }

    #[test]
    fn a_list_header_marks_a_mailing_list() {
        assert!(!headers("").mailing_list());
        assert!(headers("List-Unsubscribe: <https://list.example/u>\r\n").mailing_list());
        assert!(headers("list-id: News <news.list.example>\r\n").mailing_list());
    }

    #[test]
    fn auto_submitted_other_than_no_or_a_bulk_precedence_marks_automated_mail() {
        for extra in [
            "Auto-Submitted: auto-generated\r\n",
            "auto-submitted: Auto-Replied; owner-email=x\r\n",
            "Precedence: bulk\r\n",
            "Precedence: LIST\r\n",
            "Precedence: junk\r\n",
        ] {
            assert!(headers(extra).automated(), "{extra}");
        }
        for extra in [
            "",
            "Auto-Submitted: no\r\n",
            "Auto-Submitted: No ; comment\r\n",
            "Precedence: first-class\r\n",
        ] {
            assert!(!headers(extra).automated(), "{extra}");
        }
    }

    #[test]
    fn in_reply_to_marks_a_reply() {
        assert!(!headers("References: <a@mail.example>\r\n").reply());
        assert!(headers("In-Reply-To: <a@mail.example>\r\n").reply());
    }

    #[test]
    fn msg_key_is_the_message_id_else_a_hash_of_the_source() {
        let h = headers("Message-ID: <abc.123@mail.example>\r\n");
        assert_eq!(h.message_id.as_deref(), Some("abc.123@mail.example"));
        assert_eq!(
            summarize(b"Message-ID: <abc.123@mail.example>\r\n\r\nx").message_id,
            h.message_id
        );
        assert_eq!(
            msg_key(h.message_id.as_deref(), b"any"),
            "abc.123@mail.example"
        );
        // sha256("abc"), FIPS 180-2 appendix B.1.
        assert_eq!(
            msg_key(None, b"abc"),
            "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(msg_key(Some("  "), b"abc"), msg_key(None, b"abc"));
    }
}
