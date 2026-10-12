//! What `list`, `show` and `account list` print, for people and for scripts. The JSON shapes are documented in
//! docs/json.md; change them only by adding fields.

use std::io::{self, Write};

use chrono::{DateTime, Local, SecondsFormat, Utc};
use nm_core::cache::{ListEntry, StoredMessage};
use nm_core::parse::{Attachment, BodyKind, EmailAddress, ParsedMessage};
use nm_core::{Flags, MessageId, Uid};
use nm_imap::Security;
use serde::Serialize;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::config::{Account, Login, PasswordSource, Server};
use crate::style::Paint;

/// The human list's width when the terminal's is unknown.
pub const WIDTH: usize = 80;
const FROM_WIDTH: usize = 20;

#[derive(Serialize)]
pub struct ListItem<'a> {
    pub id: MessageId,
    pub account: &'a str,
    /// `None` (JSON `null`) for a message read through Microsoft Graph, which has no UIDs.
    pub uid: Option<Uid>,
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
    /// As [`ListItem::uid`].
    pub uid: Option<Uid>,
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

/// How a command prints its results, chosen once in main.rs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Aligned columns for a terminal.
    Human,
    /// TSV: one record per line, fields separated by one tab, no header, padding or truncation.
    Plain,
    /// One JSON value.
    Json,
    /// JSON Lines: one object per line.
    Jsonl,
}

impl Mode {
    /// `--plain`, `--json` and `--jsonl` (clap lets at most one through) win; else human on a
    /// terminal and plain off one.
    pub fn choose(plain: bool, json: bool, jsonl: bool, terminal: bool) -> Self {
        match (plain, json, jsonl) {
            (_, true, _) => Self::Json,
            (_, _, true) => Self::Jsonl,
            (true, _, _) => Self::Plain,
            _ if terminal => Self::Human,
            _ => Self::Plain,
        }
    }
}

pub fn write_json<T: Serialize + ?Sized, W: Write + ?Sized>(
    out: &mut W,
    value: &T,
) -> io::Result<()> {
    serde_json::to_writer_pretty(&mut *out, value).map_err(io::Error::from)?;
    writeln!(out)
}

/// JSON Lines: each item as one compact object on its own line.
pub fn write_jsonl<T: Serialize, W: Write + ?Sized>(
    out: &mut W,
    items: impl IntoIterator<Item = T>,
) -> io::Result<()> {
    for item in items {
        serde_json::to_writer(&mut *out, &item).map_err(io::Error::from)?;
        writeln!(out)?;
    }
    Ok(())
}

/// A listing in any mode: `items` for JSON and JSON Lines, `human` and `plain` for the others.
pub fn write_listing<T: Serialize>(
    out: &mut impl Write,
    mode: Mode,
    items: &[T],
    human: impl FnOnce(&mut dyn Write) -> io::Result<()>,
    plain: impl FnOnce(&mut dyn Write) -> io::Result<()>,
) -> io::Result<()> {
    match mode {
        Mode::Json => write_json(out, items),
        Mode::Jsonl => write_jsonl(out, items),
        Mode::Human => human(out),
        Mode::Plain => plain(out),
    }
}

/// One TSV record: each field made one line (control characters, tabs included, and bidi
/// controls become one space), then joined by tabs. Colour, when on, goes inside each field.
fn write_record(out: &mut dyn Write, fields: &[(&str, Styler)], paint: Paint) -> io::Result<()> {
    let line: Vec<String> = fields
        .iter()
        .map(|(f, style)| style(paint, &one_line(f)))
        .collect();
    writeln!(out, "{}", line.join("\t"))
}

/// How one TSV field is coloured.
type Styler = fn(Paint, &str) -> String;

fn plain_text(_: Paint, s: &str) -> String {
    s.to_owned()
}

/// `list` as TSV: id, unread (`*` or empty), date (RFC 3339 UTC, empty when unknown), account,
/// from (`Name <address>`, the address, or empty), subject.
pub fn write_list_plain(
    out: &mut dyn Write,
    entries: &[ListEntry],
    paint: Paint,
) -> io::Result<()> {
    for e in entries {
        let id = e.id.to_string();
        let unread = if e.flags.is_seen() { "" } else { "*" };
        let date = e
            .date
            .map(|d| d.to_rfc3339_opts(SecondsFormat::AutoSi, true))
            .unwrap_or_default();
        let from = e
            .from
            .as_ref()
            .map(EmailAddress::display)
            .unwrap_or_default();
        let subject = e.subject.as_deref().unwrap_or("");
        let subject_style = if unread.is_empty() {
            plain_text
        } else {
            Paint::strong
        };
        write_record(
            out,
            &[
                (&id, Paint::id),
                (unread, Paint::strong),
                (&date, Paint::date),
                (&e.account, Paint::account),
                (&from, Paint::from),
                (subject, subject_style),
            ],
            paint,
        )?;
    }
    Ok(())
}

/// One line per message: `id * date sender subject`, `*` marking unread, at most `width`
/// columns (the terminal's).
pub fn write_list(
    out: &mut dyn Write,
    entries: &[ListEntry],
    width: usize,
    paint: Paint,
) -> io::Result<()> {
    let id_width = entries
        .iter()
        .map(|e| e.id.to_string().len())
        .max()
        .unwrap_or(1);
    // id, mark, date and the gaps; the rest is shared by sender and subject.
    let rest = width.saturating_sub(id_width + 1 + 1 + 1 + 10 + 2 + 2);
    let from_width = FROM_WIDTH.min(rest / 2);
    let subject_width = rest - from_width;
    for e in entries {
        let unread = !e.flags.is_seen();
        let mark = if unread { "*" } else { " " };
        let date = e.date.map_or_else(
            || "-".repeat(10),
            |d| d.with_timezone(&Local).format("%Y-%m-%d").to_string(),
        );
        let from = e.from.as_ref().map(EmailAddress::short).unwrap_or("");
        let subject = e.subject.as_deref().unwrap_or("(no subject)");
        let subject = fit(&one_line(subject), subject_width, false);
        let subject = subject.trim_end();
        writeln!(
            out,
            "{} {} {}  {}  {}",
            paint.id(&format!("{:>id_width$}", e.id)),
            paint.strong(mark),
            paint.date(&date),
            paint.from(&fit(&one_line(from), from_width, true)),
            if unread {
                paint.strong(subject)
            } else {
                subject.to_owned()
            },
        )?;
    }
    Ok(())
}

/// `account list` for `--json` and `--jsonl`.
#[derive(Serialize)]
pub struct AccountItem<'a> {
    pub name: &'a str,
    /// "password" (the OS keyring), "password_command" or "oauth".
    pub login: &'static str,
    pub username: &'a str,
    /// Where the mail is read: the IMAP server, or Microsoft Graph for a Graph account.
    pub host: &'a str,
    pub port: u16,
    /// "tls" or "starttls"; Graph is HTTPS, so "tls".
    pub security: &'static str,
    pub cert_pinned: bool,
    /// "imap" or "graph" (Microsoft Graph).
    pub server: &'static str,
}

impl<'a> From<&'a Account> for AccountItem<'a> {
    fn from(a: &'a Account) -> Self {
        let (host, port, security, cert_pinned, server) = match &a.server {
            Server::Imap(i) => (
                i.host.as_str(),
                i.port,
                match i.security {
                    Security::Tls => "tls",
                    Security::StartTls => "starttls",
                },
                i.cert_pin.is_some(),
                "imap",
            ),
            Server::Graph { .. } => (graph_host(), 443, "tls", false, "graph"),
        };
        Self {
            name: &a.name,
            login: match &a.login {
                Login::Password(PasswordSource::Keyring) => "password",
                Login::Password(PasswordSource::Command(_)) => "password_command",
                Login::OAuth(_) => "oauth",
            },
            username: a.username(),
            host,
            port,
            security,
            cert_pinned,
            server,
        }
    }
}

/// Microsoft Graph's host, "graph.microsoft.com", from the origin nm-graph sends requests to.
fn graph_host() -> &'static str {
    nm_graph::GRAPH_ORIGIN
        .trim_start_matches("https://")
        .trim_end_matches('/')
}

/// `account list` as TSV: name, login, username, host, port, security, `pinned` or empty.
pub fn write_accounts_plain(
    out: &mut dyn Write,
    accounts: &[AccountItem<'_>],
    paint: Paint,
) -> io::Result<()> {
    for a in accounts {
        let port = a.port.to_string();
        let pinned = if a.cert_pinned { "pinned" } else { "" };
        write_record(
            out,
            &[
                (a.name, Paint::account),
                (a.login, plain_text),
                (a.username, plain_text),
                (a.host, plain_text),
                (&port, plain_text),
                (a.security, plain_text),
                (pinned, plain_text),
            ],
            paint,
        )?;
    }
    Ok(())
}

/// One line per configured account: name, login kind, username, host, port, security and
/// whether the certificate is pinned, in aligned columns. Reads nothing but the config.
pub fn write_accounts(
    out: &mut dyn Write,
    accounts: &[AccountItem<'_>],
    paint: Paint,
) -> io::Result<()> {
    let rows: Vec<[String; 7]> = accounts
        .iter()
        .map(|a| {
            [
                one_line(a.name),
                a.login.to_owned(),
                one_line(a.username),
                one_line(a.host),
                a.port.to_string(),
                a.security.to_owned(),
                if a.cert_pinned { "pinned" } else { "-" }.to_owned(),
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
        let mut line: Vec<String> = row
            .iter()
            .zip(widths)
            .map(|(cell, w)| fit(cell, w, true))
            .collect();
        if let Some(last) = line.last_mut() {
            *last = last.trim_end().to_owned();
        }
        line[0] = paint.account(&line[0]);
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

/// The line between messages when `show` prints several: it names the next one.
pub fn write_separator(out: &mut dyn Write, id: MessageId, paint: Paint) -> io::Result<()> {
    writeln!(out)?;
    writeln!(out, "{}", paint.strong(&format!("── message {id} ──")))?;
    writeln!(out)
}

pub fn write_show(out: &mut dyn Write, item: &ShowItem<'_>, paint: Paint) -> io::Result<()> {
    let label = |name: &str| paint.strong(name);
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
    writeln!(out, "{}    {}", label("Date:"), paint.date(&date))?;
    writeln!(
        out,
        "{}    {}",
        label("From:"),
        paint.from(&addrs(item.from))
    )?;
    writeln!(out, "{}      {}", label("To:"), addrs(item.to))?;
    if !item.cc.is_empty() {
        writeln!(out, "{}      {}", label("Cc:"), addrs(item.cc))?;
    }
    writeln!(
        out,
        "{} {}",
        label("Subject:"),
        paint.strong(&one_line(item.subject.unwrap_or("")))
    )?;
    writeln!(out)?;
    match item.body_type {
        BodyKind::None => writeln!(out, "(no text body)")?,
        _ => writeln!(out, "{}", text(item.body).trim_end())?,
    }
    if !item.attachments.is_empty() {
        writeln!(out, "\n{}", label("Attachments:"))?;
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

/// Whether a character could change what the terminal or a notification shows: control
/// characters (escape sequences; NEL, U+0085, is one), the line and paragraph separators U+2028
/// and U+2029 (a line break where Unicode line breaking applies, as in a desktop notification),
/// and every bidi control: the marks ALM, LRM and RLM, the embeddings and overrides, and the
/// isolates.
fn unsafe_char(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{0085}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{061C}'
                | '\u{200E}'
                | '\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2066}'..='\u{2069}'
        )
}

/// A header value made safe for one line.
pub(crate) fn one_line(s: &str) -> String {
    s.chars()
        .map(|c| if unsafe_char(c) { ' ' } else { c })
        .collect()
}

/// A body made safe for the terminal, keeping newlines and tabs; the line and paragraph
/// separators U+2028 and U+2029 become newlines, so a body keeps its line breaks.
fn text(s: &str) -> String {
    s.chars()
        .filter(|c| *c != '\r')
        .map(|c| match c {
            '\n' | '\t' => c,
            '\u{2028}' | '\u{2029}' => '\n',
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

    fn entry(id: i64, from: Option<(&str, &str)>, subject: &str, seen: bool) -> ListEntry {
        ListEntry {
            id: MessageId::new(id),
            account: "wo\trk".to_owned(),
            uid: Uid::new(1),
            date: DateTime::from_timestamp(1_791_624_600, 0),
            from: from.map(|(n, a)| EmailAddress {
                name: (!n.is_empty()).then(|| n.to_owned()),
                address: Some(a.to_owned()),
            }),
            subject: Some(subject.to_owned()),
            flags: Flags::new(if seen { vec![Flags::SEEN] } else { vec![] }),
        }
    }

    fn render(f: impl FnOnce(&mut dyn Write) -> io::Result<()>) -> String {
        let mut out = Vec::new();
        f(&mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn choosing_the_mode() {
        assert_eq!(Mode::choose(false, false, false, true), Mode::Human);
        assert_eq!(Mode::choose(false, false, false, false), Mode::Plain);
        assert_eq!(Mode::choose(true, false, false, true), Mode::Plain);
        assert_eq!(Mode::choose(false, true, false, true), Mode::Json);
        assert_eq!(Mode::choose(false, false, true, false), Mode::Jsonl);
    }

    #[test]
    fn the_human_list_fits_the_width_it_is_given() {
        let long = "word ".repeat(60);
        let entries = [
            entry(7, Some(("Ada Lovelace", "ada@mail.example")), &long, false),
            entry(12, None, "Short", true),
        ];
        for width in [40, 80, 120, 200] {
            let out = render(|o| write_list(o, &entries, width, Paint::OFF));
            let lines: Vec<&str> = out.lines().collect();
            assert_eq!(lines.len(), 2, "{out}");
            assert_eq!(lines[0].width(), width, "{width}: {:?}", lines[0]);
            assert!(lines[0].ends_with('…'), "{:?}", lines[0]);
            assert!(lines[0].starts_with(" 7 * "), "{:?}", lines[0]);
            assert!(lines[1].starts_with("12   "), "{:?}", lines[1]);
        }
        let coloured = render(|o| write_list(o, &entries, 80, Paint::ON));
        assert!(
            coloured.contains("\x1b[33m 7\x1b[0m \x1b[1m*\x1b[0m"),
            "{coloured:?}"
        );
    }

    #[test]
    fn plain_is_one_record_per_line_with_safe_untruncated_fields() {
        let long = "x".repeat(300);
        let entries = [
            entry(
                1,
                Some(("Ada\tL\x1b[31m", "ada@mail.example")),
                "a\tb\r\nc\u{85}d\u{202E}e\u{061C}f\u{200E}g\u{200F}h\u{2066}i",
                false,
            ),
            entry(2, Some(("", "bob@mail.example")), &long, true),
            entry(3, None, "", true),
        ];
        let out = render(|o| write_list_plain(o, &entries, Paint::OFF));
        let lines: Vec<&str> = out.split_terminator('\n').collect();
        assert_eq!(lines.len(), 3, "{out:?}");
        for line in &lines {
            assert_eq!(line.split('\t').count(), 6, "{line:?}");
            assert!(!line.chars().any(unsafe_char_but_tab), "{line:?}");
        }
        assert_eq!(
            lines[0],
            "1\t*\t2026-10-10T09:30:00Z\two rk\tAda L [31m <ada@mail.example>\ta b  c d e f g h i"
        );
        assert_eq!(lines[1].split('\t').nth(5), Some(long.as_str()));
        assert_eq!(lines[1].split('\t').nth(4), Some("bob@mail.example"));
        assert_eq!(lines[2], "3\t\t2026-10-10T09:30:00Z\two rk\t\t");
        // Colour goes inside the fields, never across a tab.
        let coloured = render(|o| write_list_plain(o, &entries[1..2], Paint::ON));
        assert!(
            coloured.starts_with("\x1b[33m2\x1b[0m\t\t\x1b[32m"),
            "{coloured:?}"
        );
        assert_eq!(coloured.split('\t').count(), 6);
    }

    fn unsafe_char_but_tab(c: char) -> bool {
        c != '\t' && unsafe_char(c)
    }

    #[test]
    fn jsonl_lines_are_the_json_arrays_elements() {
        let entries = [entry(1, None, "a\nb", false), entry(2, None, "c", true)];
        let items: Vec<ListItem<'_>> = entries.iter().map(ListItem::from).collect();
        let array: serde_json::Value =
            serde_json::from_str(&render(|o| write_json(o, &items))).unwrap();
        let lines = render(|o| write_jsonl(o, &items));
        let lines: Vec<serde_json::Value> = lines
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(serde_json::Value::Array(lines), array);
    }

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
        assert_eq!(one_line("a\u{2028}b\u{2029}c\u{85}d"), "a b c d");
        assert_eq!(text("a\r\n\x1b]0;x\x07b\tc"), "a\n\u{FFFD}]0;x\u{FFFD}b\tc");
        assert_eq!(text("a\u{2028}b\u{2029}c\u{85}d"), "a\nb\nc\u{FFFD}d");
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
