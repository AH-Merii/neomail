//! How help prints: clap's styles, in mise's colours from the 16 ANSI ones, and neomail's logo
//! above the top-level help on a terminal. Whether help is coloured is the one decision in
//! [`crate::style`], made for the stream the help goes to.

use std::io::{self, IsTerminal};

use clap::Command;
use clap::builder::styling::{AnsiColor, Style, Styles};
use unicode_width::UnicodeWidthStr;

/// `assets/logo/logo.txt`: the mark in half blocks with the wordmark and tagline beside it.
const LOCKUP: &str = include_str!("../../../assets/logo/logo.txt");

/// The mark is the first 21 columns of each lockup row.
const MARK_COLUMNS: usize = 21;

/// The wordmark beside the mark, bold in colour; the other text beside it is the tagline.
const WORDMARK: &str = "neomail";

/// Headings bold yellow, command and flag names bold green, value placeholders magenta.
/// A usage error keeps clap's red `error:` and its valid and invalid colours; the usage line and
/// the tips in it use these help colours.
pub fn styles() -> Styles {
    Styles::styled()
        .header(HEADING)
        .usage(HEADING)
        .literal(Style::new().bold().fg_color(Some(AnsiColor::Green.into())))
        .placeholder(Style::new().fg_color(Some(AnsiColor::Magenta.into())))
}

/// The style of a heading: `Usage:`, `Commands:`, and `Examples:` in the text after a help.
const HEADING: Style = Style::new()
    .bold()
    .fg_color(Some(clap::builder::styling::Color::Ansi(AnsiColor::Yellow)));

/// `cmd` and every command under it, with the text after each help (its examples) headed like
/// clap's own headings. clap's styles reach only the help it lays out, so the heading style is
/// written into that text; clap drops it again when the help prints without colour.
pub fn headed(cmd: Command) -> Command {
    let cmd = match cmd.get_after_help().map(|t| head(&t.to_string())) {
        Some(text) => cmd.after_help(text),
        None => cmd,
    };
    let cmd = match cmd.get_after_long_help().map(|t| head(&t.to_string())) {
        Some(text) => cmd.after_long_help(text),
        None => cmd,
    };
    cmd.mut_subcommands(headed)
}

/// `text` with the label of each line that opens with one capitalised word and a colon
/// (`Examples:`, `Guide: URL`) in the heading style.
fn head(text: &str) -> String {
    text.split_inclusive('\n')
        .map(|line| match label_end(line) {
            Some(end) => format!("{HEADING}{}{HEADING:#}{}", &line[..end], &line[end..]),
            None => line.to_owned(),
        })
        .collect()
}

/// Where the label a line opens with ends, after its colon: one capitalised word, then the
/// colon, then a space or the end of the line. `https://` and `It needs the server:` are no label.
fn label_end(line: &str) -> Option<usize> {
    let (word, after) = line.split_once(':')?;
    let capitalised = word.chars().next().is_some_and(char::is_uppercase);
    let one_word = word.chars().all(char::is_alphabetic);
    let ends = after.is_empty() || after.starts_with([' ', '\n', '\r']);
    (capitalised && one_word && ends).then_some(word.len() + 1)
}

/// The logo for a terminal `width` columns wide, each row ending in a newline: the lockup when it
/// fits, else the mark alone, else nothing, so no row wraps. `color` paints the chip black on
/// yellow, the wordmark bold and the tagline dim; the window keeps the terminal's foreground.
pub fn logo(width: usize, color: bool) -> Option<String> {
    let lockup = LOCKUP.lines().map(UnicodeWidthStr::width).max()?;
    let mark_only = if width >= lockup {
        false
    } else if width >= MARK_COLUMNS {
        true
    } else {
        return None;
    };
    let mut out = String::new();
    for row in LOCKUP.lines() {
        let split = row
            .char_indices()
            .nth(MARK_COLUMNS)
            .map_or(row.len(), |(i, _)| i);
        let (mark, beside) = row.split_at(split);
        out.push_str(&paint_mark(mark, color));
        if !mark_only {
            out.push_str(&paint_beside(beside, color));
        }
        out.push('\n');
    }
    Some(out)
}

/// A mark row: only the chip's `NORMAL` is coloured.
fn paint_mark(mark: &str, color: bool) -> String {
    match mark.strip_prefix("NORMAL") {
        Some(rest) if color => {
            let chip = Style::new()
                .fg_color(Some(AnsiColor::Black.into()))
                .bg_color(Some(AnsiColor::Yellow.into()));
            format!("{chip}NORMAL{chip:#}{rest}")
        }
        _ => mark.to_owned(),
    }
}

/// The text beside a mark row, after its gap.
fn paint_beside(beside: &str, color: bool) -> String {
    let text = beside.trim_start();
    if !color || text.is_empty() {
        return beside.to_owned();
    }
    let gap = &beside[..beside.len() - text.len()];
    let style = if text == WORDMARK {
        Style::new().bold()
    } else {
        Style::new().dimmed()
    };
    format!("{gap}{style}{text}{style:#}")
}

/// The stream a help goes to.
#[derive(Clone, Copy, Debug)]
pub enum Stream {
    Stdout,
    Stderr,
}

impl Stream {
    pub fn is_terminal(self) -> bool {
        match self {
            Self::Stdout => io::stdout().is_terminal(),
            Self::Stderr => io::stderr().is_terminal(),
        }
    }

    /// The logo above the top-level help on this stream: only on a terminal, and sized to it.
    /// Piped or redirected help has none, whatever the colour.
    pub fn logo(self, color: bool) -> Option<String> {
        if !self.is_terminal() {
            return None;
        }
        let size = match self {
            Self::Stdout => terminal_size::terminal_size_of(io::stdout()),
            Self::Stderr => terminal_size::terminal_size_of(io::stderr()),
        };
        logo(usize::from(size?.0.0), color)
    }
}

/// The top-level help: the logo, when there is one, a blank line, then `help`.
pub fn with_logo(logo: Option<String>, help: &str) -> String {
    match logo {
        Some(logo) => format!("{logo}\n{help}"),
        None => help.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Removes SGR sequences, the only escapes the logo holds.
    fn strip(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                chars.by_ref().find(|&c| c == 'm');
            } else {
                out.push(c);
            }
        }
        out
    }

    fn asset() -> String {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/logo/logo.txt");
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn a_line_opening_with_one_capitalised_word_and_a_colon_is_headed() {
        let h = |label: &str| format!("\x1b[1m\x1b[33m{label}\x1b[0m");
        let text = "Examples:\n  neomail list   List: mail\nGroups: main, receipts\n\
                    It needs the server: nothing is queued.\nhttps://example.com/docs\n\
                    neomail: a client\nGuide: https://example.com";
        let want = format!(
            "{}\n  neomail list   List: mail\n{} main, receipts\n\
             It needs the server: nothing is queued.\nhttps://example.com/docs\n\
             neomail: a client\n{} https://example.com",
            h("Examples:"),
            h("Groups:"),
            h("Guide:")
        );
        assert_eq!(head(text), want);
    }

    #[test]
    fn every_commands_after_help_is_headed_and_its_text_unchanged() {
        let cmd = || {
            Command::new("top")
                .after_help("Examples:\n  top x")
                .subcommand(
                    Command::new("sub")
                        .after_long_help("Examples:\n  top sub")
                        .subcommand(Command::new("leaf").after_help("Guide: URL")),
                )
        };
        let headed = headed(cmd());
        let sub = headed.find_subcommand("sub").unwrap();
        let leaf = sub.find_subcommand("leaf").unwrap();
        let ansi = |t: Option<&clap::builder::StyledStr>| t.unwrap().ansi().to_string();
        assert!(ansi(headed.get_after_help()).starts_with("\x1b[1m\x1b[33mExamples:"));
        assert!(ansi(sub.get_after_long_help()).starts_with("\x1b[1m\x1b[33mExamples:"));
        assert!(ansi(leaf.get_after_help()).starts_with("\x1b[1m\x1b[33mGuide:"));
        assert_eq!(
            headed.get_after_help().unwrap().to_string(),
            "Examples:\n  top x"
        );
        assert_eq!(sub.get_after_help(), None);
    }

    #[test]
    fn the_lockup_is_the_asset_with_or_without_colour() {
        let asset = asset();
        assert_eq!(logo(59, false).unwrap(), asset);
        assert_eq!(logo(200, false).unwrap(), asset);
        assert_eq!(strip(&logo(59, true).unwrap()), asset);
    }

    #[test]
    fn narrower_than_the_lockup_is_the_mark_and_narrower_than_the_mark_is_nothing() {
        let mark: String = asset()
            .lines()
            .map(|l| format!("{}\n", l.chars().take(21).collect::<String>()))
            .collect();
        assert_eq!(logo(58, false).unwrap(), mark);
        assert_eq!(logo(21, false).unwrap(), mark);
        assert_eq!(strip(&logo(21, true).unwrap()), mark);
        assert_eq!(logo(20, false), None);
        assert_eq!(logo(0, true), None);
    }

    #[test]
    fn no_row_is_wider_than_the_terminal() {
        for width in 0..80 {
            for row in logo(width, true).unwrap_or_default().lines() {
                assert!(strip(row).width() <= width, "{width}: {row}");
            }
        }
    }

    #[test]
    fn the_logo_uses_only_the_16_colours_and_attributes() {
        let colored = logo(59, true).unwrap();
        assert!(
            colored.contains("\x1b[30m\x1b[43mNORMAL\x1b[0m "),
            "{colored}"
        );
        assert!(colored.contains("\x1b[1mneomail\x1b[0m"), "{colored}");
        assert!(colored.contains("\x1b[2ma fast, modal"), "{colored}");
        assert!(
            !colored.contains("38;2") && !colored.contains("48;2"),
            "{colored}"
        );
        // The window rows keep the terminal's foreground.
        assert!(colored.starts_with(asset().lines().next().unwrap()));
    }
}
