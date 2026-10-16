//! The script, written as the step writes it and run by its paste in each shell that is
//! installed (fish, zsh, bash, dash, sh), against a fake `gcloud` first on PATH that records
//! its calls and keeps its projects in files; and the client file the console offers. No real
//! gcloud or Google is reached.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::*;
use crate::oauth::azure::tests::{Shell, files, said, shells};
use crate::wizard::capture::{self, Inbox, Outcome};

/// A stand-in for the few Google Cloud CLI commands the script runs. It refuses everything but
/// `auth login` before a sign-in in the same CLOUDSDK_CONFIG, keeps projects as directories
/// under `$FAKE_GCLOUD/projects` (a `state` file, and `apis`, one enabled API per line) and
/// records each run's CLOUDSDK_CONFIG, the CLOUDSDK_CORE_PROJECT and _ACCOUNT it saw, and its
/// arguments.
///
/// Like gcloud, it reads its settings from CLOUDSDK_<SECTION>_<NAME> variables: unless
/// `core/disable_prompts` is on, a run on a GCE machine stops at a "Do you want to continue
/// (Y/n)?" until a key (recorded in `$FAKE_GCLOUD/prompts`; `ShouldContinueLogin` in gcloud's
/// `auth login`), and unless `core/disable_usage_reporting` is on, each run leaves a reporting
/// process that outlives it (recorded in `$FAKE_GCLOUD/reports`, run by [`World::report`]).
/// `$FAKE_GCLOUD/hang` makes `auth login` wait until it is interrupted, as at the browser
/// sign-in; `$FAKE_GCLOUD/no-create` makes `projects create` fail, as for an ID that is taken.
const FAKE_GCLOUD: &str = r#"#!/bin/sh
S=$FAKE_GCLOUD
echo "${CLOUDSDK_CONFIG:-unset}" >> "$S/dirs"
echo "${CLOUDSDK_CORE_PROJECT:-}|${CLOUDSDK_CORE_ACCOUNT:-}" >> "$S/env"
echo "$*" >> "$S/calls"
on() { case "$1" in 1|true|True|TRUE|yes|on) return 0 ;; esac; return 1; }
on "$CLOUDSDK_CORE_DISABLE_USAGE_REPORTING" || echo "$CLOUDSDK_CONFIG" >> "$S/reports"
creds="$CLOUDSDK_CONFIG/credentials.db"
signed() { [ -f "$creds" ] || { echo "ERROR: (gcloud) You do not currently have an active account selected." >&2; exit 1; }; }
opt() { for a in "$@"; do case "$a" in "$1"=*) echo "${a#*=}"; return ;; esac; done; }
case "$1 $2" in
"auth login")
  on "$CLOUDSDK_CORE_DISABLE_PROMPTS" || echo "Do you want to continue (Y/n)?" >> "$S/prompts"
  [ -f "$S/hang" ] && { : > "$S/at-login"; exec sleep 600; }
  [ -f "$S/fail-login" ] && { echo "ERROR: (gcloud.auth.login) the fake sign-in was refused" >&2; exit 1; }
  [ -d "$CLOUDSDK_CONFIG" ] || { echo "fake gcloud: no config dir" >&2; exit 1; }
  : > "$creds"; echo "You are now logged in as [$(cat "$S/account")]." >&2 ;;
"auth revoke")
  signed; rm -f "$creds"; echo revoked >> "$S/revoked" ;;
"config get")
  signed; cat "$S/account" ;;
"projects describe")
  signed; p="$S/projects/$3"
  if [ -d "$p" ]; then cat "$p/state"
  else echo "ERROR: (gcloud.projects.describe) User [x] does not have permission to access projects instance [$3] (or it may not exist)" >&2; exit 1
  fi ;;
"projects create")
  signed
  [ -f "$S/no-create" ] && { echo "ERROR: (gcloud.projects.create) Project ID $3 is already in use." >&2; exit 1; }
  mkdir "$S/projects/$3" && echo ACTIVE > "$S/projects/$3/state" && : > "$S/projects/$3/apis"
  echo "$3" >> "$S/created" ;;
"services enable")
  signed; id=$(opt --project "$@"); p="$S/projects/$id"
  [ -d "$p" ] || { echo "ERROR: (gcloud.services.enable) no project $id" >&2; exit 1; }
  grep -qx "$3" "$p/apis" || echo "$3" >> "$p/apis" ;;
*) echo "fake gcloud: unknown command: $*" >&2; exit 2 ;;
esac
"#;

/// The address the fake signs in with, and the ID the script must derive from it: `neomail-`
/// and the first 10 hex digits of the sha256 of `neomail-gmail:you@example.com`, worked out
/// with `printf 'neomail-gmail:%s' you@example.com | sha256sum | cut -c1-10`.
const ACCOUNT: &str = "you@example.com";
const DERIVED: &str = "neomail-18736f4126";
const GMAIL_API: &str = "gmail.googleapis.com";

/// One world for the script: the fake gcloud first on PATH, its state, a temporary directory
/// for `mktemp`, the user's own CLOUDSDK_CONFIG and the step's directory the script goes into.
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
        let root = tempfile::tempdir().unwrap();
        let dir = |name: &str| {
            let p = root.path().join(name);
            std::fs::create_dir_all(&p).unwrap();
            p
        };
        let (bin, state, tmp, home) = (dir("bin"), dir("state"), dir("tmp"), dir("home"));
        let outside = dir("outside");
        let inbox = Inbox::new(&root.path().join("data")).unwrap();
        std::fs::create_dir_all(state.join("projects")).unwrap();
        std::fs::write(state.join("account"), format!("{ACCOUNT}\n")).unwrap();
        let gcloud = bin.join("gcloud");
        std::fs::write(&gcloud, FAKE_GCLOUD).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&gcloud, std::fs::Permissions::from_mode(0o755)).unwrap();
        // The user's own gcloud setup: a sign-in and an active project it must not touch.
        std::fs::write(outside.join("credentials.db"), "theirs").unwrap();
        std::fs::write(outside.join("active_config"), "default").unwrap();
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

    /// A project made earlier, in the browser or by an earlier run, in `state`, with `apis`.
    fn project(&self, id: &str, state: &str, apis: &[&str]) {
        let p = self.state.join("projects").join(id);
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("state"), format!("{state}\n")).unwrap();
        let lines: String = apis.iter().map(|a| format!("{a}\n")).collect();
        std::fs::write(p.join("apis"), lines).unwrap();
    }

    /// Every project's files, by project and file name.
    fn projects(&self) -> BTreeMap<String, String> {
        files(&self.state.join("projects"))
    }

    fn mark(&self, name: &str) {
        std::fs::write(self.state.join(name), "").unwrap();
    }

    fn record(&self, name: &str) -> String {
        std::fs::read_to_string(self.state.join(name)).unwrap_or_default()
    }

    /// Writes `script` as the step does and runs its paste, with `args` after it, in `shell`.
    fn run(&self, shell: &Shell, script: &str, args: &str) -> std::process::Output {
        let paste = self.inbox.paste(script).expect("no paste");
        self.command(shell, &format!("{paste}{args}"))
            .output()
            .unwrap()
    }

    /// `line` in `shell`, in this world, not yet started. With dash, the paste's own `sh` is
    /// dash too.
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
            .env("FAKE_GCLOUD", &self.state)
            .env("TMPDIR", &self.tmp)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            // The user's own gcloud settings, which the script must neither use nor change.
            .env("CLOUDSDK_CONFIG", &self.outside)
            .env("CLOUDSDK_CORE_PROJECT", "their-project")
            .env("CLOUDSDK_CORE_ACCOUNT", "them@example.com")
            .env("CLOUDSDK_CORE_DISABLE_PROMPTS", "0")
            .env("CLOUDSDK_CORE_DISABLE_USAGE_REPORTING", "false");
        command
    }

    /// The result the script wrote, read and removed as the wizard does.
    fn outcome(&self) -> Option<Outcome> {
        self.inbox.take()
    }

    /// What the reporting processes gcloud leaves do once the script has ended: write into the
    /// config directory of each run that left one, making it again if needed.
    fn report(&self) {
        for dir in self.record("reports").lines() {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(Path::new(dir).join("metrics"), "").unwrap();
        }
    }

    /// What the script must leave as it found it: the user's gcloud setup untouched and never
    /// used, no throwaway config directory (and so no sign-in) left, even after a reporting
    /// process ran, nothing half written, and no gcloud prompt waited for a key.
    fn assert_left_clean(&self, how: &str) {
        self.report();
        assert!(
            self.record("prompts").is_empty(),
            "{how}: gcloud waited for a key"
        );
        let outside = files(&self.outside);
        assert_eq!(outside.len(), 2, "{how}: {outside:?}");
        assert_eq!(outside["credentials.db"], "theirs", "{how}");
        let dirs = self.record("dirs");
        assert!(!dirs.is_empty(), "{how}: gcloud never ran");
        let outside = self.outside.to_str().unwrap();
        for dir in dirs.lines() {
            assert!(
                dir != outside && dir != "unset",
                "{how}: gcloud ran in {dir}"
            );
            assert!(!Path::new(dir).exists(), "{how}: {dir} was left behind");
        }
        for seen in self.record("env").lines() {
            assert_eq!(
                seen, "|",
                "{how}: gcloud saw the user's own project or account"
            );
        }
        let left: Vec<_> = std::fs::read_dir(&self.tmp).unwrap().collect();
        assert!(left.is_empty(), "{how}: {left:?}");
        for entry in std::fs::read_dir(self.inbox.dir()).unwrap() {
            assert_eq!(entry.unwrap().file_name(), capture::SCRIPT, "{how}");
        }
    }
}

/// The script for a project ID the wizard holds, if any.
fn script_for(project: Option<&str>) -> String {
    let project = project.map(str::to_owned);
    script(&|id| (id == "project-id").then(|| project.clone()).flatten())
}

fn stdout(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// What the browser steps set up, by a run in each shell: one project, its ID made from the
/// address signed in with, with the Gmail API enabled. A second run changes nothing, makes no
/// second project and picks up the first.
#[test]
fn a_new_project_is_made_once_in_every_shell_and_a_second_run_changes_nothing() {
    for shell in shells() {
        let how = shell.0;
        let w = World::new();
        let line = script_for(None);
        let out = w.run(&shell, &line, "");
        assert!(out.status.success(), "{how}: {}", said(&out));
        assert_eq!(w.outcome(), Some(Outcome::Value(DERIVED.into())), "{how}");
        assert!(stdout(&out).contains(DERIVED), "{how}");
        let projects = w.projects();
        assert_eq!(projects[&format!("{DERIVED}/state")], "ACTIVE\n", "{how}");
        assert_eq!(
            projects[&format!("{DERIVED}/apis")],
            format!("{GMAIL_API}\n"),
            "{how}"
        );
        let again = w.run(&shell, &line, "");
        assert!(again.status.success(), "{how}: {}", said(&again));
        assert_eq!(w.outcome(), Some(Outcome::Value(DERIVED.into())), "{how}");
        assert_eq!(
            w.projects(),
            projects,
            "{how}: the second run changed a project"
        );
        assert_eq!(w.record("created"), format!("{DERIVED}\n"), "{how}");
        // Each run's own sign-in is revoked when it ends.
        assert_eq!(w.record("revoked"), "revoked\nrevoked\n", "{how}");
        w.assert_left_clean(how);
    }
}

/// A project the user names, in the browser or for another route, as the wizard holds it or
/// after the line, is set up as it is: the Gmail API enabled and no project made.
#[test]
fn a_given_project_is_used_without_a_create() {
    for shell in shells() {
        for (held, after) in [(Some("my-project-123"), ""), (None, " my-project-123")] {
            let how = format!("{} {held:?} {after:?}", shell.0);
            let w = World::new();
            w.project("my-project-123", "ACTIVE", &["drive.googleapis.com"]);
            let out = w.run(&shell, &script_for(held), after);
            assert!(out.status.success(), "{how}: {}", said(&out));
            assert_eq!(
                w.outcome(),
                Some(Outcome::Value("my-project-123".into())),
                "{how}"
            );
            assert_eq!(
                w.projects()["my-project-123/apis"],
                format!("drive.googleapis.com\n{GMAIL_API}\n"),
                "{how}"
            );
            assert_eq!(w.projects().len(), 2, "{how}: {:?}", w.projects());
            assert!(!w.record("calls").contains("projects create"), "{how}");
            w.assert_left_clean(&how);
        }
    }
}

/// A given project the account cannot open is never made in its place: the run fails with
/// what gcloud said and the ID, and changes nothing.
#[test]
fn a_given_project_that_cannot_be_opened_fails_and_creates_nothing() {
    for shell in shells() {
        let w = World::new();
        let out = w.run(&shell, &script_for(Some("not-mine-123")), "");
        assert!(!out.status.success(), "{}", shell.0);
        let Some(Outcome::Failed { stage, said }) = w.outcome() else {
            panic!("{}: no failure handed back", shell.0)
        };
        let advice = failure(&stage, &said);
        assert!(
            advice.contains("could not open project not-mine-123"),
            "{advice}"
        );
        assert!(advice.contains("does not have permission"), "{advice}");
        assert!(w.projects().is_empty(), "{}", shell.0);
        w.assert_left_clean(shell.0);
    }
}

/// The derived project pending deletion is not made again or used: the run says how to
/// restore it.
#[test]
fn a_project_pending_deletion_is_not_replaced() {
    for shell in shells() {
        let w = World::new();
        w.project(DERIVED, "DELETE_REQUESTED", &[]);
        let before = w.projects();
        let out = w.run(&shell, &script_for(None), "");
        assert!(!out.status.success(), "{}", shell.0);
        let Some(Outcome::Failed { stage, said }) = w.outcome() else {
            panic!("{}: no failure handed back", shell.0)
        };
        let advice = failure(&stage, &said);
        assert!(
            advice.contains(&format!("gcloud projects undelete {DERIVED}")),
            "{advice}"
        );
        // Run again without the ID, the line would derive one; the advice names it.
        assert!(advice.contains(&format!("setup.sh {DERIVED})")), "{advice}");
        assert_eq!(w.projects(), before, "{}", shell.0);
        w.assert_left_clean(shell.0);
    }
}

/// A sign-in that fails, a project that cannot be made and a missing gcloud each hand back
/// what to do next, and leave no throwaway folder or sign-in behind.
#[test]
fn a_failed_run_says_what_to_do_and_leaves_nothing_behind() {
    for shell in shells() {
        for (mark, says) in [
            ("fail-login", "sign-in did not finish"),
            ("no-create", "is already in use"),
        ] {
            let how = format!("{} {mark}", shell.0);
            let w = World::new();
            w.mark(mark);
            let out = w.run(&shell, &script_for(None), "");
            assert!(!out.status.success(), "{how}");
            let Some(Outcome::Failed { stage, said }) = w.outcome() else {
                panic!("{how}: no failure handed back")
            };
            let advice = failure(&stage, &said);
            assert!(advice.contains(says), "{how}: {advice}");
            assert!(w.projects().is_empty(), "{how}");
            w.assert_left_clean(&how);
        }
    }
}

/// gcloud reports a request Google refused as one line that holds the response's headers,
/// then the JSON body, as the live check of #110 saw with a quota hit. The advice gives the
/// body's message and never the headers, and for a quota says to wait and run the line again,
/// whichever stage it stopped at.
#[test]
fn a_refused_request_is_told_in_googles_words_and_a_quota_says_to_wait() {
    let refused = |command: &str, code: u16, status: &str, message: &str| {
        format!(
            "my-project-123\nERROR: ({command}) HttpError accessing \
             <https://cloudresourcemanager.googleapis.com/v1/projects?alt=json>: response: \
             <{{'vary': 'Origin, X-Origin, Referer', 'content-type': 'application/json; \
             charset=UTF-8', 'status': {code}}}>, content <{{\n  \"error\": {{\n    \"code\": \
             {code},\n    \"message\": \"{message}\",\n    \"status\": \"{status}\"\n  }}\n}}\n>\n"
        )
    };
    let quota = "Quota exceeded for quota metric 'Write requests' and limit 'Write requests per \
                 minute' of service 'cloudresourcemanager.googleapis.com' for consumer \
                 'project_number:123'.";
    for (stage, command) in [
        ("create", "gcloud.projects.create"),
        ("project", "gcloud.projects.describe"),
        ("api", "gcloud.services.enable"),
    ] {
        let advice = failure(stage, &refused(command, 429, "RESOURCE_EXHAUSTED", quota));
        assert!(advice.contains(quota), "{stage}: {advice}");
        assert!(advice.contains("Wait a minute"), "{stage}: {advice}");
        assert!(!advice.contains("'vary'"), "{stage}: {advice}");
    }
    let taken = "Requested entity already exists";
    let said = refused("gcloud.projects.create", 409, "ALREADY_EXISTS", taken);
    let advice = failure("create", &said);
    assert!(
        advice.contains("could not create project my-project-123"),
        "{advice}"
    );
    assert!(advice.contains(taken), "{advice}");
    assert!(!advice.contains("'vary'"), "{advice}");
    assert!(!advice.contains("Wait a minute"), "{advice}");
}

/// Without gcloud on PATH the run says to install it, and leaves nothing behind. The PATH is
/// this machine's with no gcloud in it: CI runners come with the Google Cloud CLI installed.
#[test]
fn without_gcloud_the_run_says_to_install_it() {
    let shell = shells().into_iter().find(|s| s.0 == "sh").unwrap();
    let w = World::new();
    let paste = w.inbox.paste(&script_for(None)).unwrap();
    let path = tempfile::tempdir().unwrap();
    link_path_but_gcloud(path.path());
    let out = w
        .command(&shell, &paste)
        .env("PATH", path.path())
        .output()
        .unwrap();
    assert!(!out.status.success(), "{}", said(&out));
    let Some(Outcome::Failed { stage, said }) = w.outcome() else {
        panic!("no failure handed back")
    };
    assert!(failure(&stage, &said).contains("not installed"), "{stage}");
    let left: Vec<_> = std::fs::read_dir(&w.tmp).unwrap().collect();
    assert!(left.is_empty(), "{left:?}");
}

/// Links in `dir` to every program on this machine's PATH but gcloud, so that `dir` alone is
/// the PATH of the same machine without the Google Cloud CLI.
fn link_path_but_gcloud(dir: &Path) {
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    for entries in std::env::split_paths(&inherited).filter_map(|d| std::fs::read_dir(d).ok()) {
        for entry in entries.flatten().filter(|e| e.file_name() != "gcloud") {
            // A program of the same name from an earlier directory stays, as PATH finds it first.
            let _ = std::os::unix::fs::symlink(entry.path(), dir.join(entry.file_name()));
        }
    }
}

/// Anything but a project ID after the line is refused before gcloud runs.
#[test]
fn a_word_after_the_line_that_is_no_project_id_is_refused() {
    let shell = shells().into_iter().find(|s| s.0 == "sh").unwrap();
    let w = World::new();
    let out = w.run(&shell, &script_for(None), " 'my project;rm'");
    assert_eq!(out.status.code(), Some(2), "{}", said(&out));
    assert!(w.record("calls").is_empty(), "{}", w.record("calls"));
}

/// Ctrl-C at the sign-in, which reaches every process of the terminal's foreground group: the
/// script still hands back that it stopped and removes its sign-in folder. Linux only: it needs
/// GNU env's --default-signal, which macOS's env does not have.
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
        w.mark("hang");
        let paste = w.inbox.paste(&script_for(None)).unwrap();
        // The paste's own process group, as a terminal gives the job it runs, with Ctrl-C's
        // signal at its default, as a terminal job has it.
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
            assert!(
                Instant::now() < deadline,
                "{}: gcloud auth login never ran",
                shell.0
            );
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

/// A client file as the console offers it, for `project`.
fn client_json(project: &str) -> String {
    format!(
        r#"{{"installed":{{"client_id":"1234-abc.apps.googleusercontent.com","project_id":"{project}","auth_uri":"https://accounts.google.com/o/oauth2/auth","token_uri":"https://oauth2.googleapis.com/token","client_secret":"made-up-secret","redirect_uris":["http://localhost"]}}}}"#
    )
}

fn read_with(text: &str, project: Option<&str>) -> Option<Result<Vec<String>, String>> {
    let project = project.map(str::to_owned);
    client_file(text, &|id| {
        (id == "project-id").then(|| project.clone()).flatten()
    })
}

/// The Desktop client's file gives its client ID and secret when it belongs to the project the
/// wizard holds, or to any when it holds none.
#[test]
fn the_client_file_of_the_project_gives_its_id_and_secret() {
    let want = Ok(vec![
        "1234-abc.apps.googleusercontent.com".to_owned(),
        "made-up-secret".to_owned(),
    ]);
    assert_eq!(
        read_with(&client_json("my-project-123"), Some("my-project-123")),
        Some(want.clone())
    );
    assert_eq!(read_with(&client_json("my-project-123"), None), Some(want));
}

/// A file of another project or of another kind of client is not the one, and says so; a
/// file that is not whole yet is waited for.
#[test]
fn a_client_file_of_another_project_or_kind_is_refused() {
    let other = read_with(&client_json("their-project"), Some("my-project-123"));
    let Some(Err(why)) = other else {
        panic!("{other:?}")
    };
    assert!(why.contains("their-project, not my-project-123"), "{why}");
    let web = client_json("my-project-123").replace("installed", "web");
    assert!(matches!(read_with(&web, None), Some(Err(_))));
    assert_eq!(read_with(r#"{"installed":{"client_id""#, None), None);
    assert_eq!(read_with("", None), None);
}

/// What the step says it picked up names the client ID and never the secret.
#[test]
fn what_is_picked_up_shows_the_client_id_but_never_the_secret() {
    let said = picked(&[
        "1234-abc.apps.googleusercontent.com".into(),
        "made-up-secret".into(),
    ]);
    assert!(
        said.contains("1234-abc.apps.googleusercontent.com"),
        "{said}"
    );
    assert!(!said.contains("made-up-secret"), "{said}");
}
