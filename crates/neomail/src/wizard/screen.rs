//! Text for the wizard's screens, without I/O: paragraphs wrapped to the width, `code` and links
//! styled, and the escapes stripped again for the tests.

use unicode_width::UnicodeWidthStr;

use crate::style::{Paint, Symbols};

/// How the screens look: the colour decision for standard error, the symbols for the locale,
/// and whether links are OSC 8 hyperlinks (on a terminal).
#[derive(Clone, Copy, Debug)]
pub struct Look {
    pub paint: Paint,
    pub sym: Symbols,
    pub hyperlinks: bool,
}

/// The widest a paragraph gets on a wide terminal: a readable measure.
pub const MEASURE: usize = 100;

/// Display width, escapes not counted.
pub fn width(text: &str) -> usize {
    UnicodeWidthStr::width(plain(text).as_str())
}

/// `text` without its escape sequences (SGR and OSC 8).
pub fn plain(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            Some(']') => {
                // OSC: up to BEL or ESC \.
                while let Some(c) = chars.next() {
                    if c == '\x07' {
                        break;
                    }
                    if c == '\x1b' {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// The terminal rows `lines` take on a terminal `cols` wide: a line wider than that (a long
/// URL, kept whole) wraps onto more.
pub fn rows(lines: &[String], cols: usize) -> usize {
    let cols = cols.max(1);
    lines.iter().map(|l| width(l).div_ceil(cols).max(1)).sum()
}

/// `text` wrapped at spaces to `width` columns. A word wider than that (a URL) gets a line of
/// its own and is never broken, so it stays whole to copy and click.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(10);
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let w = UnicodeWidthStr::width(word);
        let used = UnicodeWidthStr::width(line.as_str());
        if !line.is_empty() && used + 1 + w > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

/// The width paragraphs wrap to on a terminal `cols` wide, after an indent of `indent`.
pub fn measure(cols: usize, indent: usize) -> usize {
    cols.min(MEASURE).saturating_sub(indent + 1).max(20)
}

/// A link: underlined, and on a terminal an OSC 8 hyperlink, so it can be clicked even where it
/// wraps.
pub fn link(look: &Look, url: &str) -> String {
    let shown = look.paint.link(url);
    if look.hyperlinks {
        format!("\x1b]8;;{url}\x1b\\{shown}\x1b]8;;\x1b\\")
    } else {
        shown
    }
}

/// Wrapped lines of one paragraph with `code` in the literal style (bold green, what you type)
/// and URLs as links. The backticks stay when there is no colour, so the text says the same.
/// A code span may run over a line break.
pub fn style(look: &Look, lines: &[String]) -> Vec<String> {
    let mut code = false;
    lines
        .iter()
        .map(|line| {
            let mut out = String::new();
            for (i, word) in line.split(' ').enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                out.push_str(&style_word(look, word, &mut code));
            }
            out
        })
        .collect()
}

fn style_word(look: &Look, word: &str, code: &mut bool) -> String {
    if !*code && !word.contains('`') {
        if let Some(start) = word.find("https://").or_else(|| word.find("http://")) {
            let (head, rest) = word.split_at(start);
            let end = rest.trim_end_matches(['.', ',', ';', ':', ')', '"']).len();
            let (url, tail) = rest.split_at(end);
            return format!("{head}{}{tail}", link(look, url));
        }
        return word.to_owned();
    }
    let on = look.paint.is_on();
    let mut out = String::new();
    let mut run = String::new();
    for c in word.chars() {
        if c == '`' {
            let styled = if *code {
                code_word(look, &run)
            } else {
                run.clone()
            };
            out.push_str(&styled);
            run.clear();
            if !on {
                out.push('`');
            }
            *code = !*code;
        } else {
            run.push(c);
        }
    }
    out.push_str(&if *code { code_word(look, &run) } else { run });
    out
}

/// A word of a command: a placeholder for a value of yours (`NAME`, `ID`) in magenta, the
/// rest in the literal style, as help shows them.
fn code_word(look: &Look, word: &str) -> String {
    let letters = word.trim_end_matches(['.', ',']);
    if letters.len() >= 2 && letters.chars().all(|c| c.is_ascii_uppercase() || c == '_') {
        format!(
            "{}{}",
            look.paint.placeholder(letters),
            &word[letters.len()..]
        )
    } else {
        look.paint.literal(word)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn look(paint: Paint) -> Look {
        Look {
            paint,
            sym: Symbols::UNICODE,
            hyperlinks: false,
        }
    }

    #[test]
    fn wraps_to_the_width_and_keeps_a_long_url_whole() {
        let url = "https://console.developers.google.com/auth/branding?project=my-project";
        let lines = wrap(&format!("Open {url} and sign in with your account."), 30);
        assert_eq!(lines[0], "Open");
        assert_eq!(lines[1], url);
        assert!(lines.iter().skip(2).all(|l| width(l) <= 30), "{lines:?}");
        assert_eq!(measure(230, 2), 97);
        assert_eq!(measure(80, 2), 77);
    }

    #[test]
    fn code_is_literal_with_colour_and_keeps_its_backticks_without() {
        let lines = vec![
            "run `neomail sync".to_owned(),
            "--account NAME` now".to_owned(),
        ];
        assert_eq!(style(&look(Paint::OFF), &lines), lines);
        let on = style(&look(Paint::ON), &lines);
        let literal = |w| Paint::ON.literal(w);
        assert_eq!(
            on[0],
            format!("run {} {}", literal("neomail"), literal("sync"))
        );
        assert!(on[1].starts_with(&Paint::ON.literal("--account")), "{on:?}");
        assert!(on[1].contains(&Paint::ON.placeholder("NAME")), "{on:?}");
        assert_eq!(plain(&on.join(" ")), "run neomail sync --account NAME now");
    }

    #[test]
    fn a_url_is_a_hyperlink_on_a_terminal_without_its_punctuation() {
        let mut l = look(Paint::OFF);
        l.hyperlinks = true;
        let lines = style(&l, &["(https://example.com/a)."].map(str::to_owned));
        assert_eq!(
            lines[0],
            "(\x1b]8;;https://example.com/a\x1b\\https://example.com/a\x1b]8;;\x1b\\)."
        );
        assert_eq!(plain(&lines[0]), "(https://example.com/a).");
    }
}
