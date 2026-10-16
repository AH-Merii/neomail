//! The script, written as the step writes it and run by its paste in each shell that is
//! installed (fish, zsh, bash, dash, sh), against a fake `az` first on PATH that records its
//! calls and keeps its apps in files. No real az or Azure is reached.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::*;
use crate::oauth::steps::PERSONAL;
use crate::wizard::capture::{self, Inbox, Outcome};

/// A stand-in for the few Azure CLI commands the line runs. It refuses `az ad` before a
/// sign-in in the same AZURE_CONFIG_DIR, keeps apps as directories under `$FAKE_AZ/apps`, and,
/// like azure-cli's `create_application`, patches the one app with the exact display name,
/// makes one when there is none, and fails without a change when there are more.
///
/// Like azure-cli, it reads its settings from AZURE_CORE_* variables (knack's `CLIConfig.get`):
/// unless `core.collect_telemetry` is off, each run leaves its config directory to a detached
/// telemetry uploader (recorded in `$FAKE_AZ/uploads`, run by [`World::upload_telemetry`]), and
/// unless `core.login_experience_v2` is off, `az login` on a terminal stops at the subscription
/// selector until Enter (recorded in `$FAKE_AZ/prompts`; azure-cli's profile `login`). With
/// `$FAKE_AZ/hang`, `az login` waits until it is interrupted, as at the browser sign-in.
const FAKE_AZ: &str = r#"#!/bin/sh
S=$FAKE_AZ
echo "${AZURE_CONFIG_DIR:-unset}" >> "$S/dirs"
echo "$*" >> "$S/calls"
off() { case "$1" in off|no|false|0|OFF|No|NO|False|FALSE) return 0 ;; esac; return 1; }
off "$AZURE_CORE_COLLECT_TELEMETRY" || echo "$AZURE_CONFIG_DIR" >> "$S/uploads"
opt() { want=$1; shift; while [ $# -gt 0 ]; do [ "$1" = "$want" ] && { echo "$2"; return; }; shift; done; }
signed() { [ -f "$AZURE_CONFIG_DIR/token" ] || { echo "ERROR: Please run az login to setup account." >&2; exit 1; }; }
known() { [ -d "$S/apps/$1" ] || { echo "ERROR: Resource $1 does not exist." >&2; exit 1; }; }
patch() {
  a=$1; shift
  v=$(opt --sign-in-audience "$@"); [ -n "$v" ] && echo "$v" > "$a/audience"
  v=$(opt --requested-access-token-version "$@"); [ -n "$v" ] && echo "$v" > "$a/version"
  v=$(opt --is-fallback-public-client "$@"); [ -n "$v" ] && echo "$v" > "$a/public"
  on=
  for x in "$@"; do
    case "$x" in
      --public-client-redirect-uris) on=1; : > "$a/redirects.new"; continue ;;
      --*) on= ;;
    esac
    [ -n "$on" ] && echo "$x" >> "$a/redirects.new"
  done
  [ -f "$a/redirects.new" ] && mv "$a/redirects.new" "$a/redirects"
  return 0
}
if [ "$1" = login ]; then
  off "$AZURE_CORE_LOGIN_EXPERIENCE_V2" || echo "Select a subscription and tenant (Type a number or Enter for no changes):" >> "$S/prompts"
  [ -f "$S/hang" ] && { : > "$S/at-login"; exec sleep 600; }
  [ -f "$S/fail-login" ] && { echo "ERROR: AADSTS50020: the fake sign-in was refused" >&2; exit 1; }
  mkdir -p "$AZURE_CONFIG_DIR" && : > "$AZURE_CONFIG_DIR/token"
  exit 0
fi
case "$1 $2 $3" in
"ad app create")
  signed; name=$(opt --display-name "$@"); n=0; found=
  for a in "$S"/apps/*; do
    [ -d "$a" ] && [ "$(cat "$a/name")" = "$name" ] && { n=$((n+1)); found=$a; }
  done
  [ $n -gt 1 ] && { echo "ERROR: More than one application have the same display name, please remove them first" >&2; exit 1; }
  if [ $n = 0 ]; then
    count=$(ls "$S/apps" | wc -l)
    found="$S/apps/aaaaaaaa-0000-0000-0000-00000000000$((count+1))"
    mkdir "$found"; echo "$name" > "$found/name"; : > "$found/redirects"; echo false > "$found/public"
  fi
  patch "$found" "$@"; basename "$found" ;;
"ad app show")
  signed; id=$(opt --id "$@"); known "$id"; cat "$S/apps/$id/redirects" ;;
"ad app update")
  signed; id=$(opt --id "$@"); known "$id"; patch "$S/apps/$id" "$@" ;;
"ad app list")
  signed; want=$(opt --display-name "$@" | tr A-Z a-z)
  for a in "$S"/apps/*; do
    [ -d "$a" ] || continue
    case "$(tr A-Z a-z < "$a/name")" in "$want"*) printf "%s\t%s\n" "$(cat "$a/name")" "$(basename "$a")" ;; esac
  done ;;
*) echo "fake az: unknown command: $*" >&2; exit 2 ;;
esac
"#;

type Shell = (&'static str, PathBuf, &'static [&'static str]);

/// The shells found on PATH, and how to run a line in each without the user's own config.
fn shells() -> Vec<Shell> {
    let found = |name: &str| {
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|dir| dir.join(name))
            .find(|p| p.is_file())
    };
    let all: [(&'static str, &'static [&'static str]); 5] = [
        ("fish", &["--no-config", "-c"]),
        ("zsh", &["-f", "-c"]),
        ("bash", &["--norc", "--noprofile", "-c"]),
        ("dash", &["-c"]),
        ("sh", &["-c"]),
    ];
    let out: Vec<Shell> = all
        .into_iter()
        .filter_map(|(name, args)| Some((name, found(name)?, args)))
        .collect();
    let names: Vec<&str> = out.iter().map(|(n, _, _)| *n).collect();
    eprintln!("shells found: {names:?}");
    assert!(names.contains(&"sh"), "no sh on PATH");
    out
}

/// One world for the script: the fake az first on PATH, its state, a temporary directory for
/// `mktemp`, the user's own AZURE_CONFIG_DIR and the step's directory the script goes into.
struct World {
    _root: tempfile::TempDir,
    bin: PathBuf,
    state: PathBuf,
    tmp: PathBuf,
    home: PathBuf,
    outside: PathBuf,
    inbox: Inbox,
}

impl World {
    fn new() -> Self {
        Self::with_data("data")
    }

    /// A world whose data directory is `data` under its root.
    fn with_data(data: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let dir = |name: &str| {
            let p = root.path().join(name);
            std::fs::create_dir_all(&p).unwrap();
            p
        };
        let (bin, state, tmp, home) = (dir("bin"), dir("state"), dir("tmp"), dir("home"));
        let outside = dir("outside");
        let inbox = Inbox::new(&root.path().join(data)).unwrap();
        std::fs::create_dir_all(state.join("apps")).unwrap();
        let az = bin.join("az");
        std::fs::write(&az, FAKE_AZ).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&az, std::fs::Permissions::from_mode(0o755)).unwrap();
        // The user's own az setup: a sign-in and a default subscription it must not touch.
        std::fs::write(
            outside.join("azureProfile.json"),
            r#"{"subscriptions":[1]}"#,
        )
        .unwrap();
        std::fs::write(outside.join("token"), "").unwrap();
        Self {
            _root: root,
            bin,
            state,
            tmp,
            home,
            outside,
            inbox,
        }
    }

    /// An app made earlier, in the browser or by an earlier run.
    fn app(&self, id: &str, name: &str, redirects: &[&str]) {
        let a = self.state.join("apps").join(id);
        std::fs::create_dir_all(&a).unwrap();
        std::fs::write(a.join("name"), format!("{name}\n")).unwrap();
        let lines: String = redirects.iter().map(|r| format!("{r}\n")).collect();
        std::fs::write(a.join("redirects"), lines).unwrap();
        std::fs::write(a.join("public"), "false\n").unwrap();
    }

    /// Every app's files, by app and file name.
    fn apps(&self) -> BTreeMap<String, String> {
        files(&self.state.join("apps"))
    }

    fn app_file(&self, id: &str, file: &str) -> String {
        let path = self.state.join("apps").join(id).join(file);
        std::fs::read_to_string(path).unwrap_or_default()
    }

    /// Writes `script` as the step does and runs its paste in `shell`. With dash, the paste's
    /// own `sh` is dash too.
    fn run(&self, shell: &Shell, script: &str) -> std::process::Output {
        let paste = self.inbox.paste(script).expect("no paste");
        self.command(shell, &paste).output().unwrap()
    }

    /// `line` in `shell`, in this world, not yet started.
    fn command(&self, shell: &Shell, line: &str) -> Command {
        self.command_via(None, shell, line)
    }

    /// [`World::command`], run through `via` (a program and its arguments) when one is given.
    fn command_via(&self, via: Option<&[&str]>, shell: &Shell, line: &str) -> Command {
        let (name, path, args) = shell;
        let sh = self.bin.join("sh");
        let _ = std::fs::remove_file(&sh);
        if *name == "dash" {
            std::os::unix::fs::symlink(path, &sh).unwrap();
        }
        let inherited = std::env::var_os("PATH").unwrap_or_default();
        let paths = std::env::split_paths(&inherited);
        let path_var = std::env::join_paths(std::iter::once(self.bin.clone()).chain(paths));
        let mut command = match via {
            Some([program, rest @ ..]) => {
                let mut c = Command::new(program);
                c.args(rest).arg(path);
                c
            }
            _ => Command::new(path),
        };
        command
            .args(*args)
            .arg(line)
            .env("PATH", path_var.unwrap())
            .env("FAKE_AZ", &self.state)
            .env("TMPDIR", &self.tmp)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("XDG_DATA_HOME", self.home.join(".local/share"))
            .env("AZURE_CONFIG_DIR", &self.outside)
            // The user's own az settings, which the script must not inherit.
            .env("AZURE_CORE_COLLECT_TELEMETRY", "yes")
            .env("AZURE_CORE_LOGIN_EXPERIENCE_V2", "on");
        command
    }

    /// The result the script wrote, read and removed as the wizard does.
    fn outcome(&self) -> Option<Outcome> {
        self.inbox.take()
    }

    fn calls(&self) -> String {
        std::fs::read_to_string(self.state.join("calls")).unwrap_or_default()
    }

    fn forget_calls(&self) {
        let _ = std::fs::remove_file(self.state.join("calls"));
    }

    /// What az's detached telemetry uploader does once the script has ended: it writes into the
    /// config directory of each run that left it one, making the directory again if needed.
    fn upload_telemetry(&self) {
        let text = std::fs::read_to_string(self.state.join("uploads")).unwrap_or_default();
        for dir in text.lines() {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(Path::new(dir).join("telemetry.txt"), "").unwrap();
        }
    }

    /// What the script must leave as it found it: the user's az setup untouched and never used,
    /// no throwaway config directory (and so no sign-in) left, even after az's telemetry
    /// uploader ran, nothing half written, and no az prompt waited for a key.
    fn assert_left_clean(&self, how: &str) {
        self.upload_telemetry();
        let prompts = std::fs::read_to_string(self.state.join("prompts")).unwrap_or_default();
        assert!(prompts.is_empty(), "{how}: az waited for a key: {prompts}");
        let outside = files(&self.outside);
        assert_eq!(outside.len(), 2, "{how}: {outside:?}");
        assert_eq!(
            outside["azureProfile.json"], r#"{"subscriptions":[1]}"#,
            "{how}"
        );
        let text = std::fs::read_to_string(self.state.join("dirs")).unwrap_or_default();
        assert!(!text.is_empty(), "{how}: az never ran");
        let outside = self.outside.to_str().unwrap();
        for dir in text.lines() {
            assert!(dir != outside && dir != "unset", "{how}: az ran in {dir}");
            assert!(!Path::new(dir).exists(), "{how}: {dir} was left behind");
        }
        let left: Vec<_> = std::fs::read_dir(&self.tmp).unwrap().collect();
        assert!(left.is_empty(), "{how}: {left:?}");
        // In the step's directory, only the script itself: no half written result.
        for entry in std::fs::read_dir(self.inbox.dir()).unwrap() {
            let name = entry.unwrap().file_name();
            assert_eq!(name, capture::SCRIPT, "{how}");
        }
    }
}

/// Every file under `dir`, by its path relative to it.
fn files(dir: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let rel = path
                    .strip_prefix(dir)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                out.insert(rel, std::fs::read_to_string(&path).unwrap());
            }
        }
    }
    out
}

/// The script for these answers.
fn script_of(account: &str, directory: Option<&str>, method: &str, app: &str) -> String {
    let answers: BTreeMap<&str, String> = [
        ("account-type", Some(account.to_owned())),
        ("directory", directory.map(str::to_owned)),
        ("sign-in-method", Some(method.to_owned())),
        ("app", Some(app.to_owned())),
    ]
    .into_iter()
    .filter_map(|(k, v)| Some((k, v?)))
    .collect();
    script(&|id| answers.get(id).cloned())
}

const NEW_ID: &str = "aaaaaaaa-0000-0000-0000-000000000001";
const OLD: &str = "bbbbbbbb-0000-0000-0000-000000000002";
const OTHER: &str = "cccccccc-0000-0000-0000-000000000003";
const ANY: &str = "AzureADandPersonalMicrosoftAccount\n";

fn said(out: &std::process::Output) -> String {
    format!(
        "status {:?}\nstdout {}\nstderr {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// The app the browser steps make, by a run in each shell: one app named neomail, for any
/// Entra tenant and personal accounts, with http://localhost for the browser sign-in or public
/// client flows for the device code. A second run changes nothing and makes no second app.
#[test]
fn a_new_app_is_set_up_once_in_every_shell_and_a_second_run_changes_nothing() {
    for shell in shells() {
        for (method, redirects, public) in [
            ("browser", "http://localhost\n", "false\n"),
            (DEVICE_CODE, "", "true\n"),
        ] {
            let how = format!("{} {method}", shell.0);
            let w = World::new();
            let line = script_of(
                PERSONAL,
                Some("contoso.onmicrosoft.com"),
                method,
                COMMANDS_APP,
            );
            let out = w.run(&shell, &line);
            assert!(out.status.success(), "{how}: {}", said(&out));
            assert_eq!(w.outcome(), Some(Outcome::Value(NEW_ID.into())), "{how}");
            assert!(
                String::from_utf8_lossy(&out.stdout).contains(NEW_ID),
                "{how}"
            );
            let apps = w.apps();
            assert_eq!(apps[&format!("{NEW_ID}/audience")], ANY, "{how}");
            assert_eq!(apps[&format!("{NEW_ID}/version")], "2\n", "{how}");
            assert_eq!(apps[&format!("{NEW_ID}/redirects")], redirects, "{how}");
            assert_eq!(apps[&format!("{NEW_ID}/public")], public, "{how}");
            assert!(
                w.calls()
                    .starts_with("login --tenant contoso.onmicrosoft.com --allow-no-subscriptions"),
                "{how}: {}",
                w.calls()
            );
            w.forget_calls();
            let again = w.run(&shell, &line);
            assert!(again.status.success(), "{how}: {}", said(&again));
            assert_eq!(w.outcome(), Some(Outcome::Value(NEW_ID.into())), "{how}");
            assert_eq!(w.apps(), apps, "{how}: the second run changed an app");
            assert!(
                !w.calls().contains("--public-client-redirect-uris"),
                "{how}"
            );
            w.assert_left_clean(&how);
        }
    }
}

/// An app named neomail made earlier, in the browser or by an earlier run, is the one set up:
/// its redirect URIs are kept, http://localhost is added, and no second app is made.
#[test]
fn an_app_made_earlier_is_picked_up_and_keeps_its_redirects() {
    for shell in shells() {
        let w = World::new();
        w.app(OLD, COMMANDS_APP, &["http://localhost:8400"]);
        w.app(OTHER, "Neomail", &[]);
        let out = w.run(
            &shell,
            &script_of("work", Some("contoso.com"), "browser", COMMANDS_APP),
        );
        assert!(out.status.success(), "{}: {}", shell.0, said(&out));
        assert_eq!(w.outcome(), Some(Outcome::Value(OLD.into())), "{}", shell.0);
        assert_eq!(w.apps().len(), 8, "{}: {:?}", shell.0, w.apps());
        assert_eq!(w.app_file(OLD, "audience"), ANY, "{}", shell.0);
        assert_eq!(
            w.app_file(OLD, "redirects"),
            "http://localhost:8400\nhttp://localhost\n",
            "{}",
            shell.0
        );
        // A work account signs in to the directory named too: without --tenant az acts on the
        // first tenant with a subscription, which may be another organisation's.
        assert!(
            w.calls()
                .starts_with("login --tenant contoso.com --allow-no-subscriptions"),
            "{}",
            w.calls()
        );
        w.assert_left_clean(shell.0);
    }
}

/// An app with another name is set up by its client ID, as the app named neomail would be.
#[test]
fn an_app_given_by_client_id_is_set_up_whatever_its_name() {
    for shell in shells() {
        let w = World::new();
        w.app(OTHER, "my mail app", &[]);
        let out = w.run(
            &shell,
            &script_of(
                PERSONAL,
                Some("contoso.onmicrosoft.com"),
                DEVICE_CODE,
                OTHER,
            ),
        );
        assert!(out.status.success(), "{}: {}", shell.0, said(&out));
        assert_eq!(
            w.outcome(),
            Some(Outcome::Value(OTHER.into())),
            "{}",
            shell.0
        );
        assert_eq!(w.app_file(OTHER, "audience"), ANY, "{}", shell.0);
        assert_eq!(w.app_file(OTHER, "public"), "true\n", "{}", shell.0);
        assert!(!w.calls().contains("create"), "{}", w.calls());
        w.assert_left_clean(shell.0);
    }
}

/// Two apps named exactly neomail: the script fails, changes nothing, hands back both client IDs,
/// and still removes its sign-in.
#[test]
fn two_apps_with_the_name_fail_with_both_ids_and_change_nothing() {
    for shell in shells() {
        let w = World::new();
        w.app(OLD, COMMANDS_APP, &["http://localhost:8400"]);
        w.app(OTHER, COMMANDS_APP, &[]);
        let before = w.apps();
        let out = w.run(
            &shell,
            &script_of(
                PERSONAL,
                Some("contoso.onmicrosoft.com"),
                "browser",
                COMMANDS_APP,
            ),
        );
        assert!(!out.status.success(), "{}: {}", shell.0, said(&out));
        assert!(
            said(&out).contains("More than one application"),
            "{}: {}",
            shell.0,
            said(&out)
        );
        let Some(Outcome::Failed { stage, said }) = w.outcome() else {
            panic!("{}: no failure handed back", shell.0)
        };
        assert_eq!(stage, "app", "{}", shell.0);
        let advice = failure(&stage, &said);
        assert!(
            advice.contains(OLD) && advice.contains(OTHER),
            "{}: {advice}",
            shell.0
        );
        assert_eq!(w.apps(), before, "{}", shell.0);
        w.assert_left_clean(shell.0);
    }
}

/// A refused sign-in fails at the sign-in, sets up nothing and leaves no config directory.
#[test]
fn a_failed_sign_in_says_so_and_leaves_nothing_behind() {
    for shell in shells() {
        let w = World::new();
        std::fs::write(w.state.join("fail-login"), "").unwrap();
        let out = w.run(
            &shell,
            &script_of(
                PERSONAL,
                Some("contoso.onmicrosoft.com"),
                "browser",
                COMMANDS_APP,
            ),
        );
        assert!(!out.status.success(), "{}: {}", shell.0, said(&out));
        let Some(Outcome::Failed { stage, .. }) = w.outcome() else {
            panic!("{}: no failure handed back", shell.0)
        };
        assert_eq!(stage, "signin", "{}", shell.0);
        assert!(w.apps().is_empty(), "{}", shell.0);
        w.assert_left_clean(shell.0);
    }
}

/// A data directory with a space, `"`, `$` and a backtick in its path: the script quotes it,
/// so the paste runs it in every shell and its result is picked up.
#[test]
fn a_data_directory_with_characters_sh_reads_is_quoted() {
    for shell in shells() {
        let w = World::with_data(r#"my "mail" $HOME `x`"#);
        let out = w.run(
            &shell,
            &script_of("work", Some("contoso.com"), DEVICE_CODE, COMMANDS_APP),
        );
        assert!(out.status.success(), "{}: {}", shell.0, said(&out));
        assert_eq!(
            w.outcome(),
            Some(Outcome::Value(NEW_ID.into())),
            "{}",
            shell.0
        );
        w.assert_left_clean(shell.0);
    }
}

/// The script copied to another machine, where the step's directory does not exist, still sets
/// up the app and prints the client ID to type.
#[test]
fn on_another_machine_the_client_id_is_printed() {
    let shell = shells().into_iter().find(|s| s.0 == "sh").unwrap();
    let w = World::new();
    w.inbox
        .paste(&script_of(
            "work",
            Some("contoso.com"),
            "browser",
            COMMANDS_APP,
        ))
        .unwrap();
    let copy = w.home.join("setup.sh");
    std::fs::copy(w.inbox.dir().join(capture::SCRIPT), &copy).unwrap();
    std::fs::remove_dir_all(w.inbox.dir()).unwrap();
    let out = w
        .command(&shell, &format!("sh '{}'", copy.display()))
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", said(&out));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains(NEW_ID),
        "{}",
        said(&out)
    );
    std::fs::create_dir(w.inbox.dir()).unwrap();
    std::fs::remove_file(&copy).unwrap();
    w.assert_left_clean("sh");
}

/// Ctrl-C at the sign-in, which reaches every process of the terminal's foreground group:
/// the script still hands back that it stopped and removes its sign-in folder. Linux only: it
/// needs GNU env's --default-signal, which macOS's env does not have.
#[cfg(target_os = "linux")]
#[test]
fn ctrl_c_at_the_sign_in_still_cleans_up_and_hands_back_a_stop() {
    use std::os::unix::process::CommandExt;
    use std::time::{Duration, Instant};
    for shell in shells()
        .into_iter()
        .filter(|s| matches!(s.0, "sh" | "dash"))
    {
        let w = World::new();
        std::fs::write(w.state.join("hang"), "").unwrap();
        let paste = w
            .inbox
            .paste(&script_of(
                PERSONAL,
                Some("contoso.onmicrosoft.com"),
                "browser",
                COMMANDS_APP,
            ))
            .unwrap();
        // The paste's own process group, as a terminal gives the job it runs, and Ctrl-C's
        // signal as a terminal job has it: the test runner may have been started with it
        // ignored, which a shell cannot undo (GNU env's --default-signal resets it).
        let via: &[&str] = &["env", "--default-signal=INT"];
        let mut child = w
            .command_via(Some(via), &shell, &paste)
            .process_group(0)
            .spawn()
            .unwrap();
        let group = child.id();
        assert!(group > 1, "{group}");
        let deadline = Instant::now() + Duration::from_secs(20);
        while !w.state.join("at-login").exists() {
            assert!(Instant::now() < deadline, "{}: az login never ran", shell.0);
            std::thread::sleep(Duration::from_millis(20));
        }
        let kill = |signal: &str| {
            Command::new("kill")
                .args([signal, "--", &format!("-{group}")])
                .status()
                .unwrap()
        };
        assert!(kill("-INT").success(), "{}", shell.0);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() > deadline {
                kill("-KILL");
                panic!("{}: the script did not end after Ctrl-C", shell.0);
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(!status.success(), "{}", shell.0);
        let Some(Outcome::Failed { stage, said }) = w.outcome() else {
            panic!("{}: no stop handed back", shell.0)
        };
        assert!(
            failure(&stage, &said).contains("stopped before"),
            "{}",
            shell.0
        );
        w.assert_left_clean(shell.0);
    }
}
