//! The Gmail wizard's commands route (#110): a script the user runs with one short paste, that
//! signs in to the Google Cloud CLI, finds or makes the project neomail's OAuth client goes in,
//! enables the Gmail API there, and hands the project ID back through
//! [`crate::wizard::capture`] (DEC-2026-10-16-commands-route). Only those three steps have a
//! command: the Google Auth Platform pages and the client stay browser steps on both routes
//! (DEC-2026-10-16-gcloud-route). It is plain POSIX sh, laid out and commented to be read
//! before it runs. It:
//! - leaves out the user's own `CLOUDSDK_*` settings and signs in with a throwaway
//!   `CLOUDSDK_CONFIG` (`mktemp -d`), so an earlier session never decides the result and the
//!   user's own gcloud setup is never read or changed; at the end it revokes that sign-in and
//!   removes the folder, even after a failure or Ctrl-C (`trap :`, as the Azure script);
//! - turns off, for its own gcloud runs only, every prompt that waits for a key
//!   (`core/disable_prompts`: each `PromptContinue` takes its default, googlecloudsdk
//!   `core/console/console_io.py`), the usage statistics, which gcloud sends from a process it
//!   leaves running (`ReportMetrics` in `core/metrics.py`), and the update check;
//! - uses the project ID the wizard holds, else `neomail-` and the first 10 hex digits of the
//!   sha256 of `neomail-gmail:` and the signed-in address, the same on every run; it looks the
//!   project up with `gcloud projects describe` (projects.get), since projects.list lags behind
//!   a create, and makes it only when it is not there;
//! - prints `ok` and the project ID, or `failed`, the stage and what gcloud said, and copies that
//!   to the result file.

use crate::wizard::capture::sh_quote;
use crate::wizard::{AnswerOf, FileValues};

/// The script, from the wizard's answers: `project-id` when it holds one. `{result}` stays in
/// it for the directory the result goes to.
pub fn script(answer: &AnswerOf<'_>) -> String {
    let want = answer("project-id")
        .filter(|id| !id.is_empty())
        .map(|id| sh_quote(&id))
        .unwrap_or_else(|| "''".to_owned());
    SCRIPT.replace("{want}", &want)
}

const SCRIPT: &str = r#"#!/bin/sh
# neomail's Gmail setup (issue #110): signs in to the Google Cloud CLI, finds or makes the
# Google Cloud project for neomail's OAuth client, enables the Gmail API in it, and hands the
# project ID back to the wizard. It holds no secret. The Google Auth Platform pages and the
# client stay in the browser: Google offers no command for them.
# Run it with: sh setup.sh
set -f

# Where the result goes: the folder neomail made for this step and watches. On another
# machine it does not exist, and the project ID is only printed.
result={result}

# The project to use: an ID after the script's name (sh setup.sh my-project-123), else the
# one you gave neomail, if any. Empty: the project gets an ID made from your Google address,
# neomail- and 10 letters and digits, the same on every run.
want=${1:-{want}}
case $want in
  *[!a-z0-9-]*) echo "neomail: not a project ID: $want" >&2; exit 2 ;;
esac

# Leave out your own Google Cloud CLI settings (a project, an account, a configuration), so
# none of them decides where the setup goes. Kept: where Python is, the proxy, certificates.
for v in $(env | sed -n 's/^\(CLOUDSDK_[A-Z0-9_]*\)=.*/\1/p'); do
  case $v in
    CLOUDSDK_PYTHON*|CLOUDSDK_ROOT_DIR|CLOUDSDK_ENCODING|CLOUDSDK_BQ_PYTHON) ;;
    CLOUDSDK_GSUTIL_PYTHON|CLOUDSDK_PROXY_*|CLOUDSDK_CORE_CUSTOM_CA_CERTS_FILE) ;;
    *) unset "$v" ;;
  esac
done

# A throwaway Google Cloud CLI folder: gcloud keeps sign-ins and an active project in its
# folder, so with one of its own an earlier session never decides the result, and your own
# gcloud setup is left as it was. The sign-in made here is revoked at the end.
tmp=$(mktemp -d) || exit
export CLOUDSDK_CONFIG="$tmp/gcloud"
# For these gcloud runs only: no question that waits for a key (each takes its default
# answer; the sign-in still asks for a code when there is no browser), no usage statistics,
# which a process gcloud leaves running would send after this script ends, and no check
# for updates.
export CLOUDSDK_CORE_DISABLE_PROMPTS=1
export CLOUDSDK_CORE_DISABLE_USAGE_REPORTING=true
export CLOUDSDK_COMPONENT_MANAGER_DISABLE_UPDATE_CHECK=true

# Ctrl-C stops the setup below but not the script, so it still cleans up.
trap : INT HUP TERM

(
  echo gcloud > "$tmp/stage"
  command -v gcloud > /dev/null 2>&1 || exit 1
  mkdir "$CLOUDSDK_CONFIG" || exit 1

  # Sign in, in your browser, or with a code when there is none.
  echo signin > "$tmp/stage"
  gcloud auth login || exit 1

  # The project ID: the one you gave, else one made from the address you signed in with.
  id=$want
  if [ -z "$id" ]; then
    echo account > "$tmp/stage"
    account=$(gcloud config get account 2>> "$tmp/said") && [ -n "$account" ] || exit 1
    hash=$(printf 'neomail-gmail:%s' "$account" \
      | { sha256sum 2> /dev/null || shasum -a 256; } | cut -c1-10)
    [ ${#hash} -eq 10 ] || exit 1
    id=neomail-$hash
  fi

  # Look the project up by its ID: a list of projects can lag behind one just made, so a
  # second run reading it could make another.
  echo project > "$tmp/stage"
  state=$(gcloud projects describe "$id" --format='value(lifecycleState)' 2> "$tmp/lookup")
  case $state in
    ACTIVE) echo "Using project $id." ;;
    DELETE_REQUESTED) echo pending > "$tmp/stage"; echo "$id" > "$tmp/said"; exit 1 ;;
    *)
      # A project you named that cannot be opened: stop, with what gcloud said.
      [ -z "$want" ] || { echo "$id" > "$tmp/said"; cat "$tmp/lookup" >> "$tmp/said"; exit 1; }
      echo create > "$tmp/stage"
      echo "$id" > "$tmp/said"
      echo "Creating project $id..."
      gcloud projects create "$id" --name=neomail --labels=neomail=gmail 2>> "$tmp/said" \
        || exit 1
      ;;
  esac

  # Enable the Gmail API in it: a second time changes nothing.
  echo api > "$tmp/stage"
  echo "$id" > "$tmp/said"
  echo "Enabling the Gmail API in $id..."
  gcloud services enable gmail.googleapis.com --project="$id" 2>> "$tmp/said" || exit 1
  echo "$id" > "$tmp/id"
)
status=$?

# Hand the project ID back: print it, and write it into the result folder under another name
# first, then rename it, so neomail never reads half a file.
case $status in
  0) echo ok; cat "$tmp/id" ;;
  129|130|143) echo failed; echo stopped ;;
  *) echo failed; cat "$tmp/stage" "$tmp/said" ;;
esac > "$tmp/result" 2> /dev/null
cat "$tmp/result"
[ -d "$result" ] && cp "$tmp/result" "$result/result.part" \
  && mv "$result/result.part" "$result/result"

# Clean up: revoke the sign-in made here, at Google and in the throwaway folder, then
# remove the folder.
[ -f "$CLOUDSDK_CONFIG/credentials.db" ] && gcloud auth revoke --all > /dev/null 2>&1
rm -rf "$tmp"
exit "$status"
"#;

/// What a failed run means and what to do next, from the stage it failed at and what the
/// script kept: the project ID first, where it had one, then what gcloud said.
pub fn failure(stage: &str, said: &str) -> String {
    let mut lines = said.lines().map(str::trim).filter(|l| !l.is_empty());
    let id = match stage {
        "project" | "pending" | "create" | "api" => lines.next().unwrap_or("the project"),
        _ => "",
    };
    let rest: Vec<&str> = lines.collect();
    let message = match google_said(&rest) {
        Some(said) => format!("gcloud said: {said}\n"),
        None => String::new(),
    };
    // Google's quota on requests per minute, which gcloud's own project shares with every user.
    if rest
        .iter()
        .any(|l| l.contains("RESOURCE_EXHAUSTED") || l.contains("RATE_LIMIT_EXCEEDED"))
    {
        return format!(
            "Google turned the request down for now: too many requests in a minute.\n{message}\
             Wait a minute, then run the line again: it goes on from where it stopped."
        );
    }
    match stage {
        "gcloud" => "The Google Cloud CLI (gcloud) is not installed where the line ran, so \
                     nothing was changed.\nInstall it (cloud.google.com/sdk/docs/install), then \
                     run the line again, or Esc to follow the browser steps."
            .to_owned(),
        "signin" => "The Google Cloud CLI sign-in did not finish, so nothing was changed.\nRun \
                     the line again and sign in with your Google account."
            .to_owned(),
        "account" => format!(
            "gcloud signed in but did not say with which address, so nothing was changed.\n\
             {message}Run the line again."
        ),
        "pending" => format!(
            "Project {id} is pending deletion, so nothing was changed.\nRestore it with gcloud \
             projects undelete {id}, then run the line again with {id} after it (sh \
             .../setup.sh {id}), or with another project's ID."
        ),
        "project" => format!(
            "gcloud could not open project {id}, so nothing was changed.\n{message}Check the ID \
             and that the account you signed in with may use it, then run the line again, or \
             run it with another project's ID after it (sh .../setup.sh my-project-123)."
        ),
        "create" => format!(
            "gcloud could not create project {id}.\n{message}If a run was interrupted, run the \
             line again in a minute. Otherwise create a project in the browser \
             (console.cloud.google.com/projectcreate) and run the line again with its ID after \
             it (sh .../setup.sh my-project-123)."
        ),
        "api" => format!(
            "Enabling the Gmail API in project {id} failed.\n{message}Fix what gcloud names, \
             then run the line again."
        ),
        _ => "The commands stopped before they ended, and nothing was picked up. Run them again."
            .to_owned(),
    }
}

/// What gcloud said, in a line. When Google refuses a request, gcloud's `HttpError` line holds
/// the response's headers and the JSON body follows, so the body's `"message"` is the part that
/// says what went wrong; otherwise its first lines.
fn google_said(lines: &[&str]) -> Option<String> {
    let message = lines.iter().find_map(|line| {
        let text = line
            .strip_prefix("\"message\":")?
            .trim()
            .trim_end_matches(',');
        let text = text.strip_prefix('"')?.strip_suffix('"')?;
        (!text.is_empty()).then(|| text.replace("\\\"", "\""))
    });
    message.or_else(|| (!lines.is_empty()).then(|| lines[..lines.len().min(3)].join(" ")))
}

/// The Desktop client's file as the console offers it when the client is made
/// (`client_secret_<client ID>.json`, with `installed.client_id`, `installed.client_secret` and
/// `installed.project_id`): its client ID and secret, in that order. A file of another kind of
/// client, or of a project other than the one the wizard holds (`project-id`), is not the one.
/// `None` while it is not whole JSON yet, as a browser may make the file before it writes it.
pub fn client_file(text: &str, answer: &AnswerOf<'_>) -> Option<FileValues> {
    let json: serde_json::Value = serde_json::from_str(text).ok()?;
    let Some(installed) = json.get("installed") else {
        return Some(Err(
            "it is not a Desktop app client's file, so it was ignored: create a \
                         client of type Desktop app and download its JSON."
                .to_owned(),
        ));
    };
    let field = |key: &str| {
        installed
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    };
    let (Some(id), Some(secret)) = (field("client_id"), field("client_secret")) else {
        return Some(Err(
            "it holds no client ID and secret, so it was ignored.".to_owned()
        ));
    };
    if let Some(want) = answer("project-id").filter(|p| !p.is_empty()) {
        let project = field("project_id");
        if project.as_deref() != Some(want.as_str()) {
            return Some(Err(format!(
                "it is the client of project {}, not {want}, so it was ignored: download the \
                 client you make in {want}.",
                project.as_deref().unwrap_or("another project")
            )));
        }
    }
    Some(Ok(vec![id, secret]))
}

/// What the client step says it picked up: the client ID, never the secret.
pub fn picked(values: &[String]) -> String {
    format!(
        "client ID {} and its secret (not shown)",
        values.first().map_or("", String::as_str)
    )
}

#[cfg(test)]
mod tests;
