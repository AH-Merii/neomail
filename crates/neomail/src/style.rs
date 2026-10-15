//! Colour: one decision per stream, made once in main.rs from `--color`, NO_COLOR,
//! CLICOLOR_FORCE, CLICOLOR, TERM and whether the stream is a terminal, then passed down as a
//! [`Paint`]. Only the 16 ANSI colours plus bold, dim and underline, so the terminal's theme picks the shades.
//! Colour never carries meaning alone: the text says the same without it.

use anstyle::{AnsiColor, Style};

/// `--color WHEN`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum ColorWhen {
    /// Colour a terminal, following NO_COLOR, CLICOLOR_FORCE, CLICOLOR and TERM
    Auto,
    /// Always colour, even plain output in a pipe (never JSON, JSONL or --raw)
    Always,
    /// Never colour or style
    Never,
}

/// What the precedence rules decided for one stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// No styling at all.
    Off,
    /// Colour, because the stream is a terminal (auto).
    Terminal,
    /// Colour wherever the output goes: `--color always` or CLICOLOR_FORCE.
    Forced,
}

/// The precedence: `--color always|never` on the command line, then NO_COLOR set and non-empty
/// (off), then CLICOLOR_FORCE set, non-empty and not "0" (forced), then CLICOLOR=0 (off), then
/// colour when the stream is a terminal and TERM is not "dumb". `var` reads the environment.
pub fn decide(
    flag: Option<ColorWhen>,
    var: impl Fn(&str) -> Option<String>,
    terminal: bool,
) -> Decision {
    match flag {
        Some(ColorWhen::Always) => return Decision::Forced,
        Some(ColorWhen::Never) => return Decision::Off,
        Some(ColorWhen::Auto) | None => {}
    }
    let set = |name: &str| var(name).filter(|v| !v.is_empty());
    if set("NO_COLOR").is_some() {
        Decision::Off
    } else if set("CLICOLOR_FORCE").is_some_and(|v| v != "0") {
        Decision::Forced
    } else if var("CLICOLOR").as_deref() == Some("0") {
        Decision::Off
    } else if terminal && var("TERM").as_deref() != Some("dumb") {
        Decision::Terminal
    } else {
        Decision::Off
    }
}

/// Reads an environment variable for [`decide`].
pub fn env_var(name: &str) -> Option<String> {
    std::env::var_os(name).map(|v| v.to_string_lossy().into_owned())
}

/// The colour choices for this run, made once in main.rs.
#[derive(Clone, Copy, Debug)]
pub struct Colors {
    /// Human output on standard output.
    pub stdout: Paint,
    /// Plain (TSV) output: only when forced, so `--plain` on a terminal stays plain.
    pub plain: Paint,
    /// Standard error.
    pub stderr: Paint,
}

impl Colors {
    pub fn new(stdout: Decision, stderr: Decision) -> Self {
        Self {
            stdout: Paint(stdout != Decision::Off),
            plain: Paint(stdout == Decision::Forced),
            stderr: Paint(stderr != Decision::Off),
        }
    }
}

/// Styles text when on; passes it through unchanged when off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Paint(bool);

impl Paint {
    #[cfg(test)]
    pub const OFF: Self = Self(false);
    #[cfg(test)]
    pub const ON: Self = Self(true);

    pub fn is_on(self) -> bool {
        self.0
    }

    fn paint(self, style: Style, text: &str) -> String {
        if self.0 && !text.is_empty() {
            format!("{style}{text}{style:#}")
        } else {
            text.to_owned()
        }
    }

    fn fg(self, color: AnsiColor, text: &str) -> String {
        self.paint(Style::new().fg_color(Some(color.into())), text)
    }

    /// A message id.
    pub fn id(self, text: &str) -> String {
        self.fg(AnsiColor::Yellow, text)
    }

    /// A date.
    pub fn date(self, text: &str) -> String {
        self.fg(AnsiColor::Green, text)
    }

    /// An account name.
    pub fn account(self, text: &str) -> String {
        self.fg(AnsiColor::Magenta, text)
    }

    /// A sender.
    pub fn from(self, text: &str) -> String {
        self.fg(AnsiColor::Cyan, text)
    }

    /// A redaction placeholder such as `<email>`: bold blue. The placeholder's words carry the
    /// meaning; the colour only makes it stand out.
    pub fn mask(self, text: &str) -> String {
        self.paint(
            Style::new().bold().fg_color(Some(AnsiColor::Blue.into())),
            text,
        )
    }

    /// Unread mail and labels: bold.
    pub fn strong(self, text: &str) -> String {
        self.paint(Style::new().bold(), text)
    }

    /// The `neomail` before an error: bold red; the words say it is an error.
    pub fn error(self, text: &str) -> String {
        self.paint(
            Style::new().bold().fg_color(Some(AnsiColor::Red.into())),
            text,
        )
    }

    // The palette's roles, shared by help (crate::help) and the setup wizards
    // (DEC-2026-10-15-wizard-ui). A symbol or a word next to the text always says the same.

    /// A heading, and where you are: bold yellow, as help's `Usage:`.
    pub fn heading(self, text: &str) -> String {
        self.paint(
            Style::new().bold().fg_color(Some(AnsiColor::Yellow.into())),
            text,
        )
    }

    /// What you type or press: commands, flags, values, keys, the item a list points at. Bold
    /// green, as help's command and flag names.
    pub fn literal(self, text: &str) -> String {
        self.paint(
            Style::new().bold().fg_color(Some(AnsiColor::Green.into())),
            text,
        )
    }

    /// A placeholder for a value of yours, such as `NAME`: magenta, as in help.
    pub fn placeholder(self, text: &str) -> String {
        self.fg(AnsiColor::Magenta, text)
    }

    /// A page to open: underlined cyan.
    pub fn link(self, text: &str) -> String {
        self.paint(
            Style::new()
                .underline()
                .fg_color(Some(AnsiColor::Cyan.into())),
            text,
        )
    }

    /// Something you do: bold, after an action bullet.
    pub fn action(self, text: &str) -> String {
        self.strong(text)
    }

    /// A side note: dim.
    pub fn note(self, text: &str) -> String {
        self.paint(Style::new().dimmed(), text)
    }

    /// A warning: yellow, after a warning sign.
    pub fn warning(self, text: &str) -> String {
        self.fg(AnsiColor::Yellow, text)
    }

    /// Something that worked: green, after a check mark.
    pub fn success(self, text: &str) -> String {
        self.fg(AnsiColor::Green, text)
    }

    /// Something that failed: red, after a cross. [`Paint::error`] is its bold form.
    pub fn failure(self, text: &str) -> String {
        self.fg(AnsiColor::Red, text)
    }
}

/// The symbols that carry the meaning colour only underlines: Unicode when the locale is UTF-8,
/// else ASCII.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Symbols {
    /// Done, saved, passed.
    pub check: &'static str,
    /// Failed, wrong.
    pub cross: &'static str,
    /// The current step, the header.
    pub arrow: &'static str,
    /// An action to take.
    pub bullet: &'static str,
    pub warning: &'static str,
    /// A page being opened.
    pub open: &'static str,
    /// A step left, and the separator in a header.
    pub dot: &'static str,
    /// The item a list points at.
    pub pointer: &'static str,
    /// A check that is running.
    pub running: &'static str,
    /// More above or below.
    pub up: &'static str,
    pub down: &'static str,
}

impl Symbols {
    pub const UNICODE: Self = Self {
        check: "✓",
        cross: "✗",
        arrow: "▸",
        bullet: "•",
        warning: "⚠",
        open: "↗",
        dot: "·",
        pointer: "❯",
        running: "…",
        up: "↑",
        down: "↓",
    };

    pub const ASCII: Self = Self {
        check: "+",
        cross: "x",
        arrow: ">",
        bullet: "*",
        warning: "!",
        open: "->",
        dot: "-",
        pointer: ">",
        running: "...",
        up: "^",
        down: "v",
    };

    /// Unicode when the locale's character set is UTF-8: the first of LC_ALL, LC_CTYPE and LANG
    /// that is set and not empty names it. `var` reads the environment.
    pub fn for_locale(var: impl Fn(&str) -> Option<String>) -> Self {
        let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
            .into_iter()
            .find_map(|name| var(name).filter(|v| !v.is_empty()))
            .unwrap_or_default()
            .to_ascii_lowercase();
        if locale.contains("utf-8") || locale.contains("utf8") {
            Self::UNICODE
        } else {
            Self::ASCII
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Var = Option<&'static str>;
    /// (flag, NO_COLOR, CLICOLOR_FORCE, CLICOLOR, TERM, terminal) and the decision.
    type Row = (Option<ColorWhen>, Var, Var, Var, Var, bool, Decision);

    #[test]
    fn precedence() {
        use ColorWhen::{Always, Auto, Never};
        use Decision::{Forced, Off, Terminal};
        #[rustfmt::skip]
        let table: &[Row] = &[
            (None,         None,      None,      None,      None,          true,  Terminal),
            (None,         None,      None,      None,      None,          false, Off),
            (Some(Auto),   None,      None,      None,      Some("xterm"), true,  Terminal),
            (None,         None,      None,      None,      Some("dumb"),  true,  Off),
            (Some(Always), Some("1"), None,      Some("0"), Some("dumb"),  false, Forced),
            (Some(Never),  None,      Some("1"), None,      None,          true,  Off),
            (None,         Some("1"), Some("1"), None,      None,          true,  Off),
            (None,         Some(""),  None,      None,      None,          true,  Terminal),
            (None,         None,      Some("1"), Some("0"), Some("dumb"),  false, Forced),
            (None,         None,      Some("0"), None,      None,          false, Off),
            (None,         None,      Some(""),  None,      None,          false, Off),
            (None,         None,      None,      Some("0"), None,          true,  Off),
            (None,         None,      None,      Some("1"), None,          true,  Terminal),
        ];
        for &(flag, no_color, force, clicolor, term, terminal, want) in table {
            let var = |name: &str| {
                match name {
                    "NO_COLOR" => no_color,
                    "CLICOLOR_FORCE" => force,
                    "CLICOLOR" => clicolor,
                    "TERM" => term,
                    _ => None,
                }
                .map(str::to_owned)
            };
            assert_eq!(
                decide(flag, var, terminal),
                want,
                "{flag:?} NO_COLOR={no_color:?} CLICOLOR_FORCE={force:?} CLICOLOR={clicolor:?} TERM={term:?} terminal={terminal}"
            );
        }
    }

    #[test]
    fn plain_is_coloured_only_when_forced() {
        let on_terminal = Colors::new(Decision::Terminal, Decision::Off);
        assert!(on_terminal.stdout.is_on() && !on_terminal.plain.is_on());
        assert!(!on_terminal.stderr.is_on());
        let forced = Colors::new(Decision::Forced, Decision::Forced);
        assert!(forced.plain.is_on() && forced.stderr.is_on());
    }

    #[test]
    fn only_the_16_colours_and_bold() {
        let p = Paint::ON;
        assert_eq!(p.id("42"), "\x1b[33m42\x1b[0m");
        assert_eq!(p.strong("x"), "\x1b[1mx\x1b[0m");
        assert_eq!(p.error("neomail"), "\x1b[1m\x1b[31mneomail\x1b[0m");
        assert_eq!(Paint::OFF.strong("x"), "x");
        assert_eq!(p.id(""), "");
        assert_eq!(p.heading("h"), "\x1b[1m\x1b[33mh\x1b[0m");
        assert_eq!(p.literal("v"), "\x1b[1m\x1b[32mv\x1b[0m");
        assert_eq!(p.note("n"), "\x1b[2mn\x1b[0m");
        assert_eq!(
            Paint::OFF.link("https://example.com"),
            "https://example.com"
        );
    }

    #[test]
    fn symbols_are_ascii_unless_the_locale_is_utf_8() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| (*v).to_owned())
            }
        };
        assert_eq!(
            Symbols::for_locale(env(&[("LANG", "en_GB.UTF-8")])),
            Symbols::UNICODE
        );
        assert_eq!(
            Symbols::for_locale(env(&[("LC_ALL", "C.utf8")])),
            Symbols::UNICODE
        );
        assert_eq!(Symbols::for_locale(env(&[])), Symbols::ASCII);
        // LC_ALL wins over LANG, and an empty one is skipped.
        let c = env(&[("LC_ALL", "C"), ("LANG", "en_GB.UTF-8")]);
        assert_eq!(Symbols::for_locale(c), Symbols::ASCII);
        let empty = env(&[("LC_ALL", ""), ("LANG", "en_GB.UTF-8")]);
        assert_eq!(Symbols::for_locale(empty), Symbols::UNICODE);
    }
}
