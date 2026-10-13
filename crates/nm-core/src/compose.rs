//! Outgoing mail, built without the network: a [`Draft`] becomes an [`Outgoing`] message, the
//! RFC 5322 bytes plus the SMTP envelope. [`reply`] and [`forward`] make a draft from a cached
//! original. The CLI builds drafts today; the TUI (slice 22) and agents (slice 17) reuse them.
//!
//! - The body is Markdown. It is sent as multipart/alternative: the Markdown source as the
//!   text/plain part, unchanged but for its line breaks, which MIME's canonical form makes CRLF
//!   (RFC 2046 section 4.1.1), and the HTML pulldown-cmark renders from it. A text-only draft
//!   has the text/plain part alone. Raw HTML in the Markdown is escaped as text, never passed
//!   through, so the HTML part holds only what the Markdown means.
//! - Bcc recipients are in the envelope only, never in a header.
//! - A reply threads as RFC 5322 section 3.6.4 says
//!   (<https://www.rfc-editor.org/rfc/rfc5322#section-3.6.4>), and quotes the original as a
//!   Markdown blockquote.
//! - Addresses are ASCII: an internationalised address needs SMTPUTF8 (RFC 6531), which the
//!   SMTP client does not speak yet. Names may be any text and are RFC 2047 encoded.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Utc};
use mail_builder::MessageBuilder;
use mail_builder::headers::address::Address;
use mail_builder::headers::date::Date;
use mail_builder::headers::message_id::MessageId;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use sha2::{Digest, Sha256};

use crate::parse::{EmailAddress, ParsedMessage};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ComposeError {
    #[error("{0:?} is not an email address: {1}")]
    Address(String, &'static str),
    #[error("the {0} holds a line break or a control character")]
    LineBreak(&'static str),
    #[error("the message has no recipients")]
    NoRecipients,
    #[error("it lists {0:?}, which cannot be sent to: {1}")]
    Unsendable(String, &'static str),
}

/// A sender or a recipient: an ASCII address and an optional display name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mailbox {
    name: Option<String>,
    address: String,
}

impl Mailbox {
    /// A mailbox from its parts. The name is trimmed, and an empty one is none.
    pub fn new(name: Option<&str>, address: &str) -> Result<Self, ComposeError> {
        let address = address.trim();
        check_address(address)?;
        let name = name.map(str::trim).filter(|n| !n.is_empty());
        if name.is_some_and(|n| n.chars().any(char::is_control)) {
            return Err(ComposeError::LineBreak("display name"));
        }
        Ok(Self {
            name: name.map(str::to_owned),
            address: address.to_owned(),
        })
    }

    /// `you@example.com` or `Your Name <you@example.com>`, as typed on a command line.
    pub fn parse(text: &str) -> Result<Self, ComposeError> {
        let text = text.trim();
        match text.strip_suffix('>').and_then(|t| t.rsplit_once('<')) {
            Some((name, address)) => {
                let name = name.trim().trim_matches('"');
                Self::new(Some(name), address)
            }
            None => Self::new(None, text),
        }
    }

    pub fn address(&self) -> &str {
        &self.address
    }

    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Whether `address` is this mailbox's: the domain ignores case (RFC 5321 section 2.4), and
    /// so does the local part here, as the providers neomail supports treat it.
    pub fn is(&self, address: &str) -> bool {
        self.address.eq_ignore_ascii_case(address.trim())
    }

    fn domain(&self) -> &str {
        self.address.rsplit_once('@').map_or("", |(_, d)| d)
    }

    /// The mailbox of a parsed address. A display name that cannot go in a header is dropped;
    /// the address is kept. An address that cannot be sent to is an error naming it.
    fn of(e: &EmailAddress) -> Result<Self, ComposeError> {
        let address = e.address.as_deref().unwrap_or_default();
        Self::new(e.name.as_deref(), address)
            .or_else(|_| Self::new(None, address))
            .map_err(|err| {
                let why = match err {
                    ComposeError::Address(_, why) => why,
                    _ => "it holds a control character",
                };
                ComposeError::Unsendable(address.to_owned(), why)
            })
    }
}

impl fmt::Display for Mailbox {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.name {
            Some(name) => write!(f, "{name} <{}>", self.address),
            None => f.write_str(&self.address),
        }
    }
}

/// An address neomail can put in a header and an SMTP command: printable ASCII with no space or
/// RFC 5322 special, one `@`, and a dotted domain.
fn check_address(address: &str) -> Result<(), ComposeError> {
    let bad = |why| Err(ComposeError::Address(address.to_owned(), why));
    if address.chars().any(char::is_control) {
        return Err(ComposeError::LineBreak("address"));
    }
    if !address.is_ascii() {
        return bad("only ASCII addresses can be sent for now");
    }
    if address
        .chars()
        .any(|c| c.is_ascii_whitespace() || "<>()[]\\,;:\"".contains(c))
    {
        return bad("it holds a space or one of <>()[]\\,;:\"");
    }
    let Some((local, domain)) = address.split_once('@') else {
        return bad("it has no @");
    };
    if local.is_empty() || domain.is_empty() || domain.contains('@') {
        return bad("it needs one @ with text on both sides");
    }
    if domain.starts_with('.') || domain.ends_with('.') || domain.contains("..") {
        return bad("its domain is not a host name");
    }
    Ok(())
}

/// A message to send. `in_reply_to` and `references` are message ids without angle brackets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub from: Mailbox,
    pub to: Vec<Mailbox>,
    pub cc: Vec<Mailbox>,
    pub bcc: Vec<Mailbox>,
    pub subject: String,
    pub markdown: String,
    pub text_only: bool,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
}

/// A built message: its bytes and the SMTP envelope. `envelope_to` holds every recipient, Bcc
/// included, once each. `Debug` leaves the message out, as it is mail.
#[derive(Clone, PartialEq, Eq)]
pub struct Outgoing {
    pub mime: Vec<u8>,
    pub envelope_from: String,
    pub envelope_to: Vec<String>,
    /// The Bcc addresses, also in `envelope_to`. A sender without an envelope (Microsoft Graph
    /// reads the recipients from the headers) cannot send to them.
    pub bcc: Vec<String>,
    /// The Message-ID, without angle brackets.
    pub message_id: String,
}

impl fmt::Debug for Outgoing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Outgoing({}, {} recipients, {} bytes)",
            self.message_id,
            self.envelope_to.len(),
            self.mime.len()
        )
    }
}

impl Draft {
    /// Builds the message, dated `now`, with a new Message-ID on the sender's domain.
    pub fn build(&self, now: DateTime<Utc>) -> Result<Outgoing, ComposeError> {
        if self.subject.chars().any(char::is_control) {
            return Err(ComposeError::LineBreak("subject"));
        }
        let mut envelope_to: Vec<String> = Vec::new();
        for m in self.to.iter().chain(&self.cc).chain(&self.bcc) {
            if !envelope_to.iter().any(|a| m.is(a)) {
                envelope_to.push(m.address.clone());
            }
        }
        if envelope_to.is_empty() {
            return Err(ComposeError::NoRecipients);
        }
        let message_id = new_message_id(now, &self.from);
        let mut builder = MessageBuilder::new()
            .from(address(&self.from))
            .subject(self.subject.as_str())
            .date(Date::new(now.timestamp()))
            .message_id(message_id.as_str());
        if !self.to.is_empty() {
            builder = builder.to(list(&self.to));
        }
        if !self.cc.is_empty() {
            builder = builder.cc(list(&self.cc));
        }
        if let Some(id) = self.in_reply_to.as_deref().filter(|id| valid_id(id)) {
            builder = builder.in_reply_to(id);
        }
        let references: Vec<&str> = self
            .references
            .iter()
            .map(String::as_str)
            .filter(|id| valid_id(id))
            .collect();
        if !references.is_empty() {
            builder = builder.references(MessageId::new_list(references.into_iter()));
        }
        builder = builder.text_body(self.markdown.as_str());
        if !self.text_only {
            builder = builder.html_body(render_html(&self.markdown));
        }
        let mime = builder
            .write_to_vec()
            .unwrap_or_else(|_| unreachable!("writing to a Vec cannot fail"));
        Ok(Outgoing {
            mime,
            envelope_from: self.from.address.clone(),
            envelope_to,
            bcc: self.bcc.iter().map(|m| m.address.clone()).collect(),
            message_id,
        })
    }
}

fn address(m: &Mailbox) -> Address<'_> {
    Address::new_address(m.name.as_deref(), m.address.as_str())
}

fn list(ms: &[Mailbox]) -> Address<'_> {
    Address::new_list(ms.iter().map(address).collect())
}

/// A message id another message can be threaded on: printable ASCII with no space or angle
/// bracket. One that is not (from a malformed original) is left out rather than sent broken.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_graphic() && b != b'<' && b != b'>')
}

/// A unique Message-ID on the sender's domain (RFC 5322 section 3.6.4): a hash of the time, the
/// process and a counter, so two messages from one run never share one, and the id says nothing
/// about the machine.
fn new_message_id(now: DateTime<Utc>, from: &Mailbox) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut hash = Sha256::new();
    hash.update(now.timestamp_nanos_opt().unwrap_or_default().to_le_bytes());
    hash.update(std::process::id().to_le_bytes());
    hash.update(COUNTER.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    hash.update(from.address.as_bytes());
    let digest = hash.finalize();
    let hex: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
    format!("{hex}.neomail@{}", from.domain())
}

/// Renders Markdown to the HTML part. Raw HTML, inline or as a block, comes out as escaped text.
/// Inside a blockquote a line break stays a line break, so quoted mail keeps its lines.
pub fn render_html(markdown: &str) -> String {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut quote_depth = 0usize;
    let events = Parser::new_ext(markdown, options).map(|event| match event {
        Event::Html(raw) | Event::InlineHtml(raw) => Event::Text(raw),
        Event::Start(Tag::HtmlBlock) => Event::Start(Tag::Paragraph),
        Event::End(TagEnd::HtmlBlock) => Event::End(TagEnd::Paragraph),
        Event::Start(Tag::BlockQuote(kind)) => {
            quote_depth += 1;
            Event::Start(Tag::BlockQuote(kind))
        }
        Event::End(TagEnd::BlockQuote(kind)) => {
            quote_depth = quote_depth.saturating_sub(1);
            Event::End(TagEnd::BlockQuote(kind))
        }
        Event::SoftBreak if quote_depth > 0 => Event::HardBreak,
        other => other,
    });
    let mut html = String::with_capacity(markdown.len() * 3 / 2);
    pulldown_cmark::html::push_html(&mut html, events);
    html
}

/// A reply to `original` from `me`, with `markdown` above the quoted original.
///
/// It goes to the original's Reply-To, else its From; to its To when those are only `me` (a reply
/// to one's own message). With `all` the original's To and Cc are copied too. An address taken
/// from the original that cannot be sent to is an error, never dropped or rerouted. No address
/// is listed twice, and `me` is never a recipient, nor any of `also_me`: the account's other
/// addresses, such as its login when `me` is an alias. The subject gets `Re: ` unless it has one
/// already, in any case. In-Reply-To and References follow RFC 5322 section 3.6.4.
pub fn reply(
    original: &ParsedMessage,
    all: bool,
    me: &Mailbox,
    also_me: &[&str],
    markdown: &str,
) -> Result<Draft, ComposeError> {
    fn with_address(list: &[EmailAddress]) -> Vec<&EmailAddress> {
        list.iter().filter(|e| e.address.is_some()).collect()
    }
    // Every address taken from the original must be one neomail can send to: one it cannot is an
    // error, so a reply never quietly reaches fewer people than it would have.
    let mailboxes = |list: &[EmailAddress]| {
        with_address(list)
            .into_iter()
            .map(Mailbox::of)
            .collect::<Result<Vec<_>, _>>()
    };
    // RFC 5322 section 3.6.3: a reply goes to Reply-To when it is present. The choice is made on
    // the raw addresses, so an address neomail cannot send to is an error, never a reroute.
    let mut answer_to = with_address(&original.reply_to);
    if answer_to.is_empty() {
        answer_to = with_address(&original.from);
    }
    let is_me = |a: &str| {
        me.is(a)
            || also_me
                .iter()
                .any(|o| o.trim().eq_ignore_ascii_case(a.trim()))
    };
    let own = |e: &EmailAddress| e.address.as_deref().is_some_and(is_me);
    let primary = if !answer_to.is_empty() && answer_to.iter().all(|e| own(e)) {
        mailboxes(&original.to)?
    } else {
        answer_to
            .into_iter()
            .map(Mailbox::of)
            .collect::<Result<Vec<_>, _>>()?
    };
    let mut to = Vec::new();
    add_unique(&mut to, &[], &is_me, primary);
    let mut cc = Vec::new();
    if all {
        let mut others = mailboxes(&original.to)?;
        others.extend(mailboxes(&original.cc)?);
        add_unique(&mut cc, &to, &is_me, others);
    }
    if to.is_empty() && cc.is_empty() {
        return Err(ComposeError::NoRecipients);
    }
    let t = &original.threading;
    // "If the parent message does not contain a "References:" field but does have an
    // "In-Reply-To:" field containing a single message identifier", that one comes first.
    let mut references = if !t.references.is_empty() {
        t.references.clone()
    } else if t.in_reply_to.len() == 1 {
        t.in_reply_to.clone()
    } else {
        Vec::new()
    };
    references.extend(t.message_id.clone());
    let attribution = match original.date {
        Some(date) => format!("On {}, {} wrote:", long_date(date), senders(original)),
        None => format!("{} wrote:", senders(original)),
    };
    Ok(Draft {
        from: me.clone(),
        to,
        cc,
        bcc: Vec::new(),
        subject: prefixed("Re: ", &["re:"], original.subject.as_deref()),
        markdown: format!(
            "{}\n\n{attribution}\n\n{}",
            markdown.trim_end(),
            quoted(&original.body)
        ),
        text_only: false,
        in_reply_to: t.message_id.clone(),
        references,
    })
}

/// A forward of `original` to `to`: `markdown`, then the original's headers and text, quoted. The
/// original's attachments are not carried; the caller warns when it had any.
pub fn forward(original: &ParsedMessage, me: &Mailbox, to: Vec<Mailbox>, markdown: &str) -> Draft {
    let joined = |list: &[EmailAddress]| {
        list.iter()
            .map(EmailAddress::display)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut header = vec![format!("From: {}", joined(&original.from))];
    if let Some(date) = original.date {
        header.push(format!("Date: {}", long_date(date)));
    }
    header.push(format!(
        "Subject: {}",
        original.subject.as_deref().unwrap_or_default()
    ));
    header.push(format!("To: {}", joined(&original.to)));
    if !original.cc.is_empty() {
        header.push(format!("Cc: {}", joined(&original.cc)));
    }
    let quoted_text = quoted(&format!("{}\n\n{}", header.join("\n"), original.body));
    let intro = markdown.trim_end();
    let separator = "---------- Forwarded message ----------";
    let markdown = if intro.is_empty() {
        format!("{separator}\n\n{quoted_text}")
    } else {
        format!("{intro}\n\n{separator}\n\n{quoted_text}")
    };
    Draft {
        from: me.clone(),
        to,
        cc: Vec::new(),
        bcc: Vec::new(),
        subject: prefixed("Fwd: ", &["fwd:", "fw:"], original.subject.as_deref()),
        markdown,
        text_only: false,
        in_reply_to: None,
        references: Vec::new(),
    }
}

/// Adds each of `new` to `into` unless it is the account's (`is_me`), already in `into` or in
/// `besides`.
fn add_unique(
    into: &mut Vec<Mailbox>,
    besides: &[Mailbox],
    is_me: &dyn Fn(&str) -> bool,
    new: Vec<Mailbox>,
) {
    for m in new {
        let taken = |list: &[Mailbox]| list.iter().any(|o| o.is(&m.address));
        if !is_me(&m.address) && !taken(into) && !taken(besides) {
            into.push(m);
        }
    }
}

/// `prefix` and the subject, unless the subject starts with one of `already` in any case.
///
/// The original's subject is not the user's typing, so a control character in it (a tab, or one
/// an encoded word decoded to) becomes a space, with runs of spaces collapsed, rather than making
/// the reply impossible to build.
fn prefixed(prefix: &str, already: &[&str], subject: Option<&str>) -> String {
    let cleaned = subject
        .unwrap_or_default()
        .split(|c: char| c == ' ' || c.is_control())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let subject = cleaned.as_str();
    let has = already.iter().any(|p| {
        subject
            .get(..p.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(p))
    });
    if has {
        subject.to_owned()
    } else {
        format!("{prefix}{subject}")
    }
}

fn senders(original: &ParsedMessage) -> String {
    let from: Vec<String> = original.from.iter().map(EmailAddress::display).collect();
    if from.is_empty() {
        "someone".to_owned()
    } else {
        from.join(", ")
    }
}

fn long_date(date: DateTime<Utc>) -> String {
    date.format("%a, %-d %b %Y at %H:%M UTC").to_string()
}

/// `text` as a Markdown blockquote: each line after `> `, an empty one as `>`.
fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 8);
    for line in text.trim_end().lines() {
        out.push('>');
        if !line.is_empty() {
            out.push(' ');
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests;
