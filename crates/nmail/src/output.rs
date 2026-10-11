//! What `list` and `show` print, for people and for scripts. The JSON shapes are documented in
//! docs/json.md; change them only by adding fields.

use std::io::{self, Write};

use chrono::{DateTime, Local, Utc};
use nm_core::cache::{ListEntry, StoredMessage};
use nm_core::parse::{Attachment, BodyKind, EmailAddress, ParsedMessage};
use nm_core::{Flags, MessageId, Uid};
use serde::Serialize;
use unicode_width::UnicodeWidthChar;

/// The human list fits this many columns.
pub const WIDTH: usize = 80;
const FROM_WIDTH: usize = 20;

#[derive(Serialize)]
pub struct ListItem<'a> {
    pub id: MessageId,
    pub account: &'a str,
    pub uid: Uid,
    pub date: Option<DateTime<Utc>>,
    pub from: Option<&'a EmailAddress>,
    pub subject: Option<&'a str>,
    pub unread: bool,
    pub flags: &'a Flags,
}

impl<'a> From<&'a ListEntry> for ListItem<'a> {
    fn from(e: &'a ListEntry) -> Self {
        Self {
            id: e.id,
            account: &e.account,
            uid: e.uid,
            date: e.date,
            from: e.from.as_ref(),
            subject: e.subject.as_deref(),
            unread: !e.flags.is_seen(),
            flags: &e.flags,
        }
    }
}

#[derive(Serialize)]
pub struct ShowItem<'a> {
    pub id: MessageId,
    pub account: &'a str,
    pub uid: Uid,
    pub date: Option<DateTime<Utc>>,
    pub received: Option<DateTime<Utc>>,
    pub from: &'a [EmailAddress],
    pub to: &'a [EmailAddress],
    pub cc: &'a [EmailAddress],
    pub subject: Option<&'a str>,
    pub unread: bool,
    pub flags: &'a Flags,
    pub body_type: BodyKind,
    pub body: &'a str,
    pub attachments: &'a [Attachment],
}

impl<'a> ShowItem<'a> {
    pub fn new(stored: &'a StoredMessage, parsed: &'a ParsedMessage) -> Self {
        Self {
            id: stored.id,
            account: &stored.account,
            uid: stored.uid,
            date: parsed.date,
            received: stored.received,
            from: &parsed.from,
            to: &parsed.to,
            cc: &parsed.cc,
            subject: parsed.subject.as_deref(),
            unread: !stored.flags.is_seen(),
            flags: &stored.flags,
            body_type: parsed.body_kind,
            body: &parsed.body,
            attachments: &parsed.attachments,
        }
    }
}

pub fn write_json<T: Serialize>(out: &mut impl Write, value: &T) -> io::Result<()> {
    serde_json::to_writer_pretty(&mut *out, value).map_err(io::Error::other)?;
    writeln!(out)
}

/// One line per message: `id * date sender subject`, `*` marking unread, at most 80 columns.
pub fn write_list(out: &mut impl Write, entries: &[ListEntry]) -> io::Result<()> {
    let id_width = entries
        .iter()
        .map(|e| e.id.to_string().len())
        .max()
        .unwrap_or(1);
    for e in entries {
        let mark = if e.flags.is_seen() { ' ' } else { '*' };
        let date = e.date.map_or_else(
            || "-".repeat(10),
            |d| d.with_timezone(&Local).format("%Y-%m-%d").to_string(),
        );
        let from = e.from.as_ref().map(EmailAddress::short).unwrap_or("");
        let fixed = id_width + 1 + 1 + 1 + 10 + 2 + FROM_WIDTH + 2;
        let subject_width = WIDTH.saturating_sub(fixed);
        let subject = e.subject.as_deref().unwrap_or("(no subject)");
        let line = format!(
            "{:>id_width$} {mark} {date}  {}  {}",
            e.id,
            fit(&one_line(from), FROM_WIDTH, true),
            fit(&one_line(subject), subject_width, false),
        );
        writeln!(out, "{}", line.trim_end())?;
    }
    Ok(())
}

pub fn write_show(out: &mut impl Write, item: &ShowItem<'_>) -> io::Result<()> {
    let addrs = |list: &[EmailAddress]| {
        one_line(
            &list
                .iter()
                .map(EmailAddress::display)
                .collect::<Vec<_>>()
                .join(", "),
        )
    };
    let date = item.date.or(item.received).map_or_else(String::new, |d| {
        d.with_timezone(&Local)
            .format("%a, %d %b %Y %H:%M %:z")
            .to_string()
    });
    writeln!(out, "Date:    {date}")?;
    writeln!(out, "From:    {}", addrs(item.from))?;
    writeln!(out, "To:      {}", addrs(item.to))?;
    if !item.cc.is_empty() {
        writeln!(out, "Cc:      {}", addrs(item.cc))?;
    }
    writeln!(out, "Subject: {}", one_line(item.subject.unwrap_or("")))?;
    writeln!(out)?;
    match item.body_type {
        BodyKind::None => writeln!(out, "(no text body)")?,
        _ => writeln!(out, "{}", text(item.body).trim_end())?,
    }
    if !item.attachments.is_empty() {
        writeln!(out, "\nAttachments:")?;
        for a in item.attachments {
            let name = one_line(a.name.as_deref().unwrap_or("(unnamed)"));
            writeln!(
                out,
                "  {name}  {}  {}",
                one_line(&a.content_type),
                size(a.size)
            )?;
        }
    }
    Ok(())
}

fn size(bytes: usize) -> String {
    match bytes {
        b if b < 1024 => format!("{b} B"),
        b if b < 1024 * 1024 => format!("{:.1} KiB", b as f64 / 1024.0),
        b => format!("{:.1} MiB", b as f64 / (1024.0 * 1024.0)),
    }
}

/// Whether a character could change what the terminal shows: control characters (escape
/// sequences) and bidirectional overrides.
fn unsafe_char(c: char) -> bool {
    c.is_control() || matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// A header value made safe for one line.
fn one_line(s: &str) -> String {
    s.chars()
        .map(|c| if unsafe_char(c) { ' ' } else { c })
        .collect()
}

/// A body made safe for the terminal, keeping newlines and tabs.
fn text(s: &str) -> String {
    s.chars()
        .filter(|c| *c != '\r')
        .map(|c| match c {
            '\n' | '\t' => c,
            c if unsafe_char(c) => '\u{FFFD}',
            c => c,
        })
        .collect()
}

/// Cuts `s` to `width` display columns with an ellipsis; pads it when `pad` is set.
fn fit(s: &str, width: usize, pad: bool) -> String {
    let full: usize = s.chars().map(|c| c.width().unwrap_or(0)).sum();
    let mut out = String::new();
    let mut used = 0;
    if full <= width {
        out.push_str(s);
        used = full;
    } else if width > 0 {
        for c in s.chars() {
            let w = c.width().unwrap_or(0);
            if used + w > width - 1 {
                break;
            }
            out.push(c);
            used += w;
        }
        out.push('…');
        used += 1;
    }
    if pad {
        out.extend(std::iter::repeat_n(' ', width.saturating_sub(used)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_truncates_by_display_width() {
        assert_eq!(fit("hello", 10, true), "hello     ");
        assert_eq!(fit("hello world", 6, false), "hello…");
        assert_eq!(fit("日本語のテキスト", 7, true), "日本語…");
        assert_eq!(fit("日本語のテキスト", 8, true), "日本語… ");
    }

    #[test]
    fn escapes_are_neutralised() {
        assert_eq!(one_line("a\x1b[31mb\nc\u{202E}d"), "a [31mb c d");
        assert_eq!(text("a\r\n\x1b]0;x\x07b\tc"), "a\n\u{FFFD}]0;x\u{FFFD}b\tc");
    }
}
