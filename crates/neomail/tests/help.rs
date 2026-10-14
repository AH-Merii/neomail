//! Help: coloured by the rules all of neomail's output follows, and opening with the logo when the
//! top-level help goes to a terminal.

#![allow(clippy::unwrap_used)] // Test helpers outside #[test] functions.

use std::path::Path;
use std::process::Command;
#[cfg(target_os = "linux")]
use std::process::Stdio;

/// The lockup the top-level help opens with.
#[cfg(target_os = "linux")]
const LOCKUP: &str = include_str!("../../../assets/logo/logo.txt");
/// The first line of the top-level help after the logo.
#[cfg(target_os = "linux")]
const ABOUT: &str = "neomail: a terminal email client\n";
/// The ways to ask for the top-level help, besides bare `neomail` with a config.
const TOP: [&[&str]; 3] = [&["--help"], &["-h"], &["help"]];
/// A command's own help, asked for both ways.
const COMMAND: [&[&str]; 3] = [&["list", "--help"], &["help", "list"], &["account", "-h"]];

/// The mark alone: the first 21 columns of each lockup row.
#[cfg(target_os = "linux")]
fn mark() -> String {
    let rows: Vec<String> = LOCKUP
        .lines()
        .map(|l| l.chars().take(21).collect())
        .collect();
    assert_eq!(rows.len(), 8);
    rows.iter().map(|r| format!("{r}\n")).collect()
}

/// The binary with its config in `dir` and none of the caller's colour settings.
fn isolate(c: &mut Command, dir: &Path) {
    c.env("NEOMAIL_DATA_DIR", dir)
        .env("NEOMAIL_CONFIG", dir.join("config.toml"))
        .env("HOME", dir)
        .env("TERM", "xterm-256color")
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent");
    for var in [
        "XDG_CONFIG_HOME",
        "NO_COLOR",
        "CLICOLOR",
        "CLICOLOR_FORCE",
        "COLUMNS",
    ] {
        c.env_remove(var);
    }
}

/// neomail's exit code and everything it printed, standard output then standard error.
fn piped(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> (i32, String) {
    let mut c = Command::new(env!("CARGO_BIN_EXE_neomail"));
    isolate(&mut c, dir);
    let o = c.args(args).envs(env.iter().copied()).output().unwrap();
    let mut text = String::from_utf8(o.stdout).unwrap();
    text.push_str(&String::from_utf8(o.stderr).unwrap());
    (o.status.code().unwrap(), text)
}

/// neomail's exit code and what it printed on a pseudo-terminal `cols` columns wide.
#[cfg(target_os = "linux")]
fn on_terminal(dir: &Path, cols: u16, args: &[&str], env: &[(&str, &str)]) -> (i32, String) {
    in_shell(
        dir,
        cols,
        &format!("exec \"$NEOMAIL\" {}", args.join(" ")),
        env,
    )
}

/// The exit code and output of the shell command `run`, with neomail as `$NEOMAIL`, on a
/// pseudo-terminal `cols` columns wide, so `run` can send one stream elsewhere.
#[cfg(target_os = "linux")]
fn in_shell(dir: &Path, cols: u16, run: &str, env: &[(&str, &str)]) -> (i32, String) {
    let run = format!("stty rows 24 cols {cols}; {run}");
    let mut c = Command::new("script");
    isolate(&mut c, dir);
    let o = c
        .args(["-qec", &run, "/dev/null"])
        .env("SHELL", "/bin/sh")
        .env("NEOMAIL", env!("CARGO_BIN_EXE_neomail"))
        .envs(env.iter().copied())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let text = String::from_utf8(o.stdout).unwrap().replace("\r\n", "\n");
    (o.status.code().unwrap(), text)
}

/// `text` without its SGR sequences.
fn strip(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            chars.by_ref().find(|&c| c == 'm');
        } else {
            out.push(c);
        }
    }
    out
}

/// A directory with a config file, so bare `neomail` prints the help.
fn configured() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"), "").unwrap();
    dir
}

#[test]
#[cfg(target_os = "linux")]
fn top_level_help_on_a_terminal_opens_with_the_lockup_in_colour() {
    let dir = configured();
    for (args, code) in TOP.iter().map(|a| (*a, 0)).chain([(&[][..], 2)]) {
        let (status, out) = on_terminal(dir.path(), 80, args, &[]);
        assert_eq!(status, code, "{args:?}: {out}");
        assert!(
            strip(&out).starts_with(&format!("{LOCKUP}\n{ABOUT}")),
            "{args:?}: {out}"
        );
        // 16 colours only: the chip black on yellow, the wordmark bold, the tagline dim.
        assert!(
            out.contains("\x1b[30m\x1b[43mNORMAL\x1b[0m "),
            "{args:?}: {out}"
        );
        assert!(out.contains("\x1b[1mneomail\x1b[0m\n"), "{args:?}: {out}");
        assert!(out.contains("\x1b[2ma fast, modal"), "{args:?}: {out}");
        assert!(
            !out.contains("38;2;") && !out.contains("48;2;"),
            "{args:?}: {out}"
        );
        // Headings bold yellow, names bold green, placeholders magenta.
        assert!(
            out.contains("\x1b[1m\x1b[33mCommands:\x1b[0m"),
            "{args:?}: {out}"
        );
        assert!(
            out.contains("\x1b[1m\x1b[32mlist\x1b[0m"),
            "{args:?}: {out}"
        );
        assert!(
            out.contains("\x1b[1m\x1b[32m--color\x1b[0m"),
            "{args:?}: {out}"
        );
        assert!(out.contains("\x1b[35m<WHEN>\x1b[0m"), "{args:?}: {out}");
    }
}

#[test]
#[cfg(target_os = "linux")]
fn a_commands_help_has_no_logo_but_is_coloured() {
    let dir = configured();
    for args in COMMAND {
        let (status, out) = on_terminal(dir.path(), 80, args, &[]);
        assert_eq!(status, 0, "{args:?}: {out}");
        assert!(
            out.contains("\x1b[1m\x1b[33mUsage:\x1b[0m"),
            "{args:?}: {out}"
        );
        assert!(!out.contains("NORMAL"), "{args:?}: {out}");
        assert!(
            !out.contains(LOCKUP.lines().next().unwrap()),
            "{args:?}: {out}"
        );
    }
}

#[test]
#[cfg(target_os = "linux")]
fn the_terminal_width_picks_the_lockup_the_mark_or_no_logo() {
    let dir = configured();
    let mark = mark();
    for (cols, logo) in [
        (80, LOCKUP),
        (59, LOCKUP),
        (58, &mark),
        (21, &mark),
        (20, ""),
    ] {
        for args in [&["--help", "--color", "never"][..], &["--color", "never"]] {
            let (_, out) = on_terminal(dir.path(), cols, args, &[]);
            let want = if logo.is_empty() {
                ABOUT.to_owned()
            } else {
                format!("{logo}\n{ABOUT}")
            };
            assert!(out.starts_with(&want), "{cols} {args:?}: {out}");
            // No logo row is wider than the terminal, so none wraps.
            for row in out[..logo.len()].lines() {
                assert!(row.chars().count() <= usize::from(cols), "{cols}: {row}");
            }
        }
    }
}

#[test]
fn piped_help_has_no_logo_and_colour_only_when_forced() {
    let dir = configured();
    for args in TOP.iter().copied().chain([&[][..]]).chain(COMMAND) {
        let (_, plain) = piped(dir.path(), args, &[]);
        assert!(!plain.contains('\x1b'), "{args:?}: {plain}");
        assert!(!plain.contains("NORMAL"), "{args:?}: {plain}");
        // Before the arguments: `neomail help --color always` asks for help on "--color".
        let mut always = vec!["--color", "always"];
        always.extend(args);
        for (args, env) in [
            (always, &[][..]),
            (args.to_vec(), &[("CLICOLOR_FORCE", "1")]),
        ] {
            let (_, forced) = piped(dir.path(), &args, env);
            assert!(
                forced.contains("\x1b[1m\x1b[33mUsage:"),
                "{args:?} {env:?}: {forced}"
            );
            assert!(!forced.contains("NORMAL"), "{args:?} {env:?}: {forced}");
            // Colour adds styling and nothing else.
            assert_eq!(strip(&forced), plain, "{args:?} {env:?}");
        }
    }
}

#[test]
#[cfg(target_os = "linux")]
fn help_on_a_terminal_follows_each_colour_rule() {
    let dir = configured();
    type Case<'a> = (&'a [&'a str], &'a [(&'a str, &'a str)], bool);
    let cases: &[Case] = &[
        (&["--help", "--color", "never"], &[], false),
        (&["--color=never", "help"], &[], false),
        (&["list", "--help", "--color", "never"], &[], false),
        (&["--color", "never"], &[], false),
        (&["--help"], &[("NO_COLOR", "1")], false),
        (&[], &[("NO_COLOR", "1")], false),
        (&["--help", "--color", "always"], &[("NO_COLOR", "1")], true),
        (&["--help"], &[("CLICOLOR", "0")], false),
        (
            &["--help"],
            &[("CLICOLOR", "0"), ("CLICOLOR_FORCE", "1")],
            true,
        ),
        (&["help", "list"], &[("TERM", "dumb")], false),
        (&[], &[("TERM", "dumb")], false),
        (&["--help", "--color", "auto"], &[], true),
    ];
    for &(args, env, colour) in cases {
        let (_, out) = on_terminal(dir.path(), 80, args, env);
        assert_eq!(out.contains('\x1b'), colour, "{args:?} {env:?}: {out}");
        assert!(
            strip(&out).contains("Usage: neomail"),
            "{args:?} {env:?}: {out}"
        );
    }
}

#[test]
#[cfg(target_os = "linux")]
fn the_logo_is_the_only_thing_a_terminal_adds_to_the_help() {
    let dir = configured();
    for args in TOP.iter().copied().chain([&[][..]]) {
        let (piped_code, plain) = piped(dir.path(), args, &[]);
        let (code, out) = on_terminal(dir.path(), 80, args, &[]);
        assert_eq!(code, piped_code, "{args:?}");
        assert_eq!(strip(&out), format!("{LOCKUP}\n{plain}"), "{args:?}");
    }
}

#[test]
#[cfg(target_os = "linux")]
fn the_stream_the_help_goes_to_decides_the_logo_and_the_colour() {
    let dir = configured();
    // `--help`, `-h` and `help` print on standard output: piped, it has neither, though standard
    // error stays the terminal.
    for args in TOP {
        let run = format!("\"$NEOMAIL\" {} | cat", args.join(" "));
        let (_, out) = in_shell(dir.path(), 80, &run, &[]);
        assert!(out.starts_with(ABOUT), "{args:?}: {out}");
        assert!(!out.contains('\x1b'), "{args:?}: {out}");
    }
    // Bare `neomail` prints on standard error: it decides there, whatever standard output is.
    let (_, out) = in_shell(dir.path(), 80, "\"$NEOMAIL\" 2>&1 >/dev/tty | cat", &[]);
    assert!(out.starts_with(ABOUT), "{out}");
    assert!(!out.contains('\x1b'), "{out}");
    let (_, out) = in_shell(dir.path(), 80, "\"$NEOMAIL\" >/dev/null", &[]);
    assert!(
        strip(&out).starts_with(&format!("{LOCKUP}\n{ABOUT}")),
        "{out}"
    );
    assert!(out.contains("\x1b[1m\x1b[33mCommands:"), "{out}");
}

#[test]
fn the_examples_after_a_help_are_headed_like_its_other_headings() {
    let dir = configured();
    let heading = |label: &str| format!("\x1b[1m\x1b[33m{label}\x1b[0m");
    for args in TOP
        .iter()
        .copied()
        .chain([&[][..], &["list", "--help"], &["help", "send"]])
    {
        let mut always = vec!["--color", "always"];
        always.extend(args);
        let (_, forced) = piped(dir.path(), &always, &[]);
        let mut labels = Vec::new();
        for line in forced.lines() {
            let plain = strip(line);
            let Some((word, after)) = plain.split_once(':') else {
                continue;
            };
            let capitalised = word.chars().next().is_some_and(char::is_uppercase);
            if capitalised
                && word.chars().all(char::is_alphabetic)
                && (after.is_empty() || after.starts_with(' '))
            {
                assert!(
                    line.starts_with(&heading(&format!("{word}:"))),
                    "{args:?}: {line:?}"
                );
                labels.push(word.to_owned());
            }
        }
        assert!(labels.iter().any(|l| l == "Examples"), "{args:?}: {forced}");
        let (_, never) = piped(dir.path(), &[&["--color", "never"][..], args].concat(), &[]);
        assert!(!never.contains('\x1b'), "{args:?}: {never}");
        assert!(never.contains("\nExamples:\n"), "{args:?}: {never}");
    }
}
