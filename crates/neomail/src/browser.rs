//! Opening the sign-in URL in the user's browser: `$BROWSER` when set, else `wslview` or
//! `explorer.exe` on WSL (the Windows browser; WSL2 forwards its localhost to the loopback
//! listener), else `xdg-open`. The URL is always printed as well, so a failure here only costs a
//! copy and paste. Choosing the command ([`command_for`]) is separate from running it, so the
//! tests check the choice without launching anything.

use std::process::{Command, Stdio};

use anyhow::{Context, Result};

/// The command line that opens `url`. `browser` is `$BROWSER`: a `:`-separated list of
/// commands, of which the first is used, with `%s` replaced by the URL or the URL appended.
/// `on_path` says whether a program can be found on `PATH`.
pub fn command_for(
    url: &str,
    browser: Option<&str>,
    wsl: bool,
    on_path: impl Fn(&str) -> bool,
) -> Vec<String> {
    if let Some(first) = browser.and_then(|b| b.split(':').map(str::trim).find(|c| !c.is_empty())) {
        let mut words: Vec<String> = first.split_whitespace().map(str::to_owned).collect();
        if words.iter().any(|w| w.contains("%s")) {
            for w in &mut words {
                *w = w.replace("%s", url);
            }
        } else {
            words.push(url.to_owned());
        }
        return words;
    }
    let program = if wsl {
        if on_path("wslview") {
            "wslview"
        } else {
            "explorer.exe"
        }
    } else {
        "xdg-open"
    };
    vec![program.to_owned(), url.to_owned()]
}

/// Starts the browser on `url` without waiting for it, and returns the program's name.
pub fn open(url: &str) -> Result<String> {
    let browser = std::env::var("BROWSER").ok();
    let words = command_for(url, browser.as_deref(), crate::keyring::is_wsl(), on_path);
    let (program, args) = words.split_first().context("$BROWSER is empty")?;
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("cannot run {program}"))?;
    Ok(program.clone())
}

/// Whether this is an SSH session without a display: `SSH_CONNECTION` or `SSH_TTY` is set, and
/// `DISPLAY` and `WAYLAND_DISPLAY` are both unset or empty. A browser opened here would not
/// reach the user, so sign-in uses a device code where the provider has one. `env` reads a
/// variable.
pub fn ssh_without_display(env: impl Fn(&str) -> Option<String>) -> bool {
    let set = |name: &str| env(name).is_some_and(|v| !v.is_empty());
    (set("SSH_CONNECTION") || set("SSH_TTY")) && !set("DISPLAY") && !set("WAYLAND_DISPLAY")
}

pub(crate) fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str = "https://accounts.google.com/o/oauth2/v2/auth?a=1&b=2";

    #[test]
    fn picks_browser_then_wsl_then_xdg_open() {
        let none = |_: &str| false;
        assert_eq!(
            command_for(URL, Some("firefox --new-window:chromium"), false, none),
            ["firefox", "--new-window", URL]
        );
        assert_eq!(
            command_for(URL, Some("open-it %s --flag"), true, none),
            ["open-it", URL, "--flag"]
        );
        assert_eq!(command_for(URL, Some(""), false, none), ["xdg-open", URL]);
        assert_eq!(
            command_for(URL, None, true, |p| p == "wslview"),
            ["wslview", URL]
        );
        assert_eq!(command_for(URL, None, true, none), ["explorer.exe", URL]);
        assert_eq!(command_for(URL, None, false, |_| true), ["xdg-open", URL]);
    }

    #[test]
    fn device_code_is_automatic_only_over_ssh_without_a_display() {
        // (variables set, with their values; expected)
        let cases: [(&[(&str, &str)], bool); 9] = [
            (&[], false),
            (&[("SSH_CONNECTION", "10.0.0.2 5000 10.0.0.1 22")], true),
            (&[("SSH_TTY", "/dev/pts/3")], true),
            (&[("SSH_CONNECTION", "")], false),
            (&[("SSH_TTY", "/dev/pts/3"), ("DISPLAY", ":0")], false),
            (
                &[("SSH_TTY", "/dev/pts/3"), ("WAYLAND_DISPLAY", "wayland-1")],
                false,
            ),
            (
                &[
                    ("SSH_TTY", "/dev/pts/3"),
                    ("DISPLAY", ""),
                    ("WAYLAND_DISPLAY", ""),
                ],
                true,
            ),
            (&[("DISPLAY", ":0")], false),
            (
                &[
                    ("SSH_CONNECTION", "x"),
                    ("SSH_TTY", "y"),
                    ("DISPLAY", "localhost:10.0"),
                ],
                false,
            ),
        ];
        for (vars, want) in cases {
            let env = |name: &str| {
                vars.iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| (*v).to_owned())
            };
            assert_eq!(ssh_without_display(env), want, "{vars:?}");
        }
    }
}
