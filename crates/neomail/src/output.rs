//! What `list`, `show` and `account list` print, for people and for scripts. The JSON shapes are documented in
//! docs/json.md; change them only by adding fields.

use std::io::{self, Write};

use chrono::{DateTime, Local, Utc};
use nm_core::cache::{ListEntry, StoredMessage};
use nm_core::parse::{Attachment, BodyKind, EmailAddress, ParsedMessage};
use nm_core::{Flags, MessageId, Uid};
use nm_imap::Security;
use serde::Serialize;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::config::{Account, Login, PasswordSource};

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

/// One line per configured account: name, login kind, username, host, port, security and
/// whether the certificate is pinned, in aligned columns. Reads nothing but the config.
pub fn write_accounts(out: &mut impl Write, accounts: &[Account]) -> io::Result<()> {
    let rows: Vec<[String; 7]> = accounts
        .iter()
        .map(|a| {
            let login = match &a.login {
                Login::Password(PasswordSource::Keyring) => "password",
                Login::Password(PasswordSource::Command(_)) => "password_command",
                Login::OAuth(_) => "oauth",
            };
            let security = match a.imap.security {
                Security::Tls => "tls",
                Security::StartTls => "starttls",
            };
            let pinned = if a.imap.cert_pin.is_some() {
                "pinned"
            } else {
                "-"
            };
            [
                one_line(&a.name),
                login.to_owned(),
                one_line(&a.imap.username),
                one_line(&a.imap.host),
                a.imap.port.to_string(),
                security.to_owned(),
                pinned.to_owned(),
            ]
        })
        .collect();
    let mut widths = [0; 7];
    for row in &rows {
        for (w, cell) in widths.iter_mut().zip(row) {
            *w = (*w).max(cell.width());
        }
    }
    for row in &rows {
        let line: Vec<String> = row
            .iter()
            .zip(widths)
            .map(|(cell, w)| fit(cell, w, true))
            .collect();
        writeln!(out, "{}", line.join("  ").trim_end())?;
    }
    Ok(())
}

/// The per-account counter `neomail sync` shows on standard error ("work: fetching 50/120"),
/// updated in place and cleared at the end. Disabled (off a terminal) it writes nothing.
pub struct ProgressLine {
    enabled: bool,
    shown: bool,
}

impl ProgressLine {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            shown: false,
        }
    }

    /// Best effort: a failed write to standard error does not stop the sync.
    pub fn update(&mut self, out: &mut impl Write, account: &str, p: nm_core::Progress) {
        if !self.enabled {
            return;
        }
        let _ = write!(
            out,
            "\r\x1b[2K{}: fetching {}/{}",
            one_line(account),
            p.done,
            p.total
        );
        let _ = out.flush();
        self.shown = true;
    }

    pub fn clear(&mut self, out: &mut impl Write) {
        if self.shown {
            let _ = write!(out, "\r\x1b[2K");
            let _ = out.flush();
            self.shown = false;
        }
    }
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

/// Cuts `s` to `width` display columns with an ellipsis; pads it when `pad` is set. Widths are
/// measured per grapheme cluster, so an emoji sequence such as U+2764 U+FE0F counts as the two
/// columns a terminal gives it, and is never cut in half.
fn fit(s: &str, width: usize, pad: bool) -> String {
    let full = s.width();
    let mut out = String::new();
    let mut used = 0;
    if full <= width {
        out.push_str(s);
        used = full;
    } else if width > 0 {
        for g in s.graphemes(true) {
            let w = g.width();
            if used + w > width - 1 {
                break;
            }
            out.push_str(g);
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
        // U+2764 alone is one column; with U+FE0F it is an emoji, two columns.
        assert_eq!(fit("\u{2764}\u{FE0F}", 2, true), "\u{2764}\u{FE0F}");
        assert_eq!(fit("a\u{2764}\u{FE0F}b", 3, false), "a…");
        assert_eq!(
            fit("\u{2764}\u{FE0F}\u{2764}\u{FE0F}", 3, true),
            "\u{2764}\u{FE0F}…"
        );
    }

    #[test]
    fn escapes_are_neutralised() {
        assert_eq!(one_line("a\x1b[31mb\nc\u{202E}d"), "a [31mb c d");
        assert_eq!(text("a\r\n\x1b]0;x\x07b\tc"), "a\n\u{FFFD}]0;x\u{FFFD}b\tc");
    }

    #[test]
    fn progress_off_a_terminal_writes_nothing_and_on_one_clears_itself() {
        let p = nm_core::Progress {
            done: 50,
            total: 120,
        };
        let mut off = ProgressLine::new(false);
        let mut out = Vec::new();
        off.update(&mut out, "work", p);
        off.clear(&mut out);
        assert!(out.is_empty());

        let mut on = ProgressLine::new(true);
        on.update(&mut out, "pro\x1b[31mton", p);
        on.clear(&mut out);
        on.clear(&mut out);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "\r\x1b[2Kpro [31mton: fetching 50/120\r\x1b[2K"
        );
    }
}
