//! The Outlook wizard's commands route (#105): a script the user runs with one short paste,
//! that signs in to the Azure CLI, sets up the Entra app as the browser steps would and hands
//! its client ID back through [`crate::wizard::capture`] (DEC-2026-10-16-commands-route). It is
//! plain POSIX sh, laid out and commented to be read before it runs. It:
//! - signs in with a throwaway `AZURE_CONFIG_DIR` of its own (`mktemp -d`), so an earlier sign-in
//!   or default subscription never decides where the app goes and is never touched, and removes
//!   it when it ends, even after a failure or Ctrl-C (`trap :` keeps the script alive while its
//!   subshell dies), so no sign-in is left behind;
//! - turns off, for its own az runs only, the subscription selector `az login` shows on a
//!   terminal (it would wait for Enter) and the telemetry uploader, which az detaches and which
//!   writes into the config directory after the script removed it;
//! - `az ad app create` picks up the one app named exactly `neomail` and patches it, or makes
//!   it, and fails without changing anything when two have the name (azure-cli's
//!   `create_application`); `--public-client-redirect-uris` replaces the whole list, so it reads
//!   the list first and adds `http://localhost` only when it is missing;
//! - prints `ok` and the client ID, or `failed`, the stage and what az said, and copies that
//!   to the result file.

use super::steps::{COMMANDS_APP, DEVICE_CODE};
use crate::wizard::AnswerOf;
use crate::wizard::capture::sh_quote;

/// The app's settings, as the browser steps set them: any Entra tenant and personal accounts,
/// with version 2 access tokens, which personal accounts need.
const SETTINGS: &str =
    "--sign-in-audience AzureADandPersonalMicrosoftAccount --requested-access-token-version 2";

/// What the device code sign-in adds: Allow public client flows.
const PUBLIC_CLIENT: &str = " --is-fallback-public-client true";

/// The script, from the wizard's answers: `directory`, `sign-in-method` and `app` (the name
/// `neomail` or a client ID). `{result}` stays in it for the directory the result goes to.
pub fn script(answer: &AnswerOf<'_>) -> String {
    let device = answer("sign-in-method").as_deref() == Some(DEVICE_CODE);
    // Always the directory the user named: without --tenant az signs in to every tenant the
    // account reaches and acts on the first with a subscription, which may be another's.
    let tenant = answer("directory")
        .map(|dir| format!(" --tenant {}", sh_quote(&dir)))
        .unwrap_or_default();
    let app = answer("app").unwrap_or_else(|| COMMANDS_APP.to_owned());
    let public = if device { PUBLIC_CLIENT } else { "" };
    let app_q = sh_quote(&app);
    let setup = match app == COMMANDS_APP {
        true => format!(
            r#"  # Set up the app: az picks up the one app named exactly {app} and updates it, or
  # makes it. With two of that name it changes nothing and fails, and their IDs are listed.
  id=$(az ad app create --display-name {app_q} \
    {SETTINGS}{public} \
    --query appId -o tsv 2>>"$tmp/said") || {{
    az ad app list --display-name {app_q} --query '[].[displayName,appId]' -o tsv \
      > "$tmp/list" 2>/dev/null
    exit 1
  }}
  [ -n "$id" ] || exit 1
"#
        ),
        false => format!(
            r#"  # Set up the app you named by its client ID.
  id={app_q}
  az ad app update --id "$id" \
    {SETTINGS}{public} 2>>"$tmp/said" || exit
"#
        ),
    };
    let redirect = match device {
        true => String::new(),
        false => r#"
  # The browser sign-in needs the redirect URI http://localhost. Setting the list replaces it
  # whole, so read it first and add the URI only when it is missing (set -f above keeps
  # $uris, one word per URI, from being read as file patterns).
  uris=$(az ad app show --id "$id" --query publicClient.redirectUris -o tsv \
    2>>"$tmp/said") || exit
  found=
  for uri in $uris; do [ "$uri" = http://localhost ] && found=1; done
  [ -n "$found" ] || az ad app update --id "$id" \
    --public-client-redirect-uris $uris http://localhost 2>>"$tmp/said" || exit
"#
        .to_owned(),
    };
    [
        HEAD.to_owned(),
        format!(
            r#"  # Sign in, in your browser, to the directory you named.
  az login{tenant} --allow-no-subscriptions -o none \
    || {{ echo signin > "$tmp/stage"; exit 1; }}
  echo app > "$tmp/stage"

"#
        ),
        setup,
        redirect,
        TAIL.to_owned(),
    ]
    .concat()
}

/// The script up to the sign-in.
const HEAD: &str = r#"#!/bin/sh
# neomail's Outlook setup (issue #105): signs in to the Azure CLI, sets up the app neomail
# signs in with, and hands its client ID back to the wizard. It holds no secret.
# Run it with: sh setup.sh
set -f

# Where the result goes: the folder neomail made for this step and watches. On another
# machine it does not exist, and the client ID is only printed.
result={result}

# A throwaway Azure CLI folder: az keeps sign-ins and a default subscription in its folder,
# so with one of its own an earlier sign-in never decides where the app goes, your own az
# setup is left as it was, and removing it at the end leaves no sign-in behind.
tmp=$(mktemp -d) || exit

# Ctrl-C stops the setup below but not the script, so it still cleans up.
trap : INT HUP TERM

(
  export AZURE_CONFIG_DIR="$tmp"
  # For these az runs only: no subscription selector, which az login shows on a terminal
  # and which would wait for Enter, and no telemetry, which a detached uploader would write
  # into the throwaway folder after it is removed.
  export AZURE_CORE_LOGIN_EXPERIENCE_V2=off
  export AZURE_CORE_COLLECT_TELEMETRY=no

"#;

/// The script after the setup: hand the result back and clean up.
const TAIL: &str = r#"
  echo "$id" > "$tmp/id"
)
status=$?

# Hand the client ID back: print it, and write it into the result folder under another name
# first, then rename it, so neomail never reads half a file.
if [ "$status" = 0 ]; then echo ok; cat "$tmp/id"
else echo failed; cat "$tmp/stage" "$tmp/said" "$tmp/list"
fi > "$tmp/result" 2>/dev/null
cat "$tmp/result"
[ -d "$result" ] && cp "$tmp/result" "$result/result.part" \
  && mv "$result/result.part" "$result/result"

# Clean up: remove the throwaway folder, and the sign-in with it.
rm -rf "$tmp"
exit "$status"
"#;

/// What a failed run means and what to do next, from the stage it failed at (`signin`, `app`,
/// or none when it was stopped) and what az said.
pub fn failure(stage: &str, said: &str) -> String {
    let twins: Vec<&str> = said
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .filter(|(name, _)| *name == COMMANDS_APP)
        .map(|(_, id)| id.trim())
        .collect();
    let message: Vec<&str> = said
        .lines()
        .filter(|l| !l.contains('\t') && !l.trim().is_empty())
        .take(3)
        .collect();
    let message = match message.is_empty() {
        true => String::new(),
        false => format!("az said: {}\n", message.join(" ")),
    };
    match stage {
        "signin" => format!(
            "The Azure CLI sign-in failed, so nothing was changed.\n{message}Check the directory \
             and the account you signed in with (Esc twice goes back to Your directory), then run \
             the commands again."
        ),
        "app" if twins.len() > 1 || said.contains("More than one application") => format!(
            "More than one app is named exactly {COMMANDS_APP}, so nothing was changed.\nEsc \
             goes back to Which app: type the client ID of the one to use ({}), or delete the \
             others at entra.microsoft.com and run the commands again.",
            match twins.is_empty() {
                true => "the Entra admin center lists them".to_owned(),
                false => twins.join(", "),
            }
        ),
        "app" => format!(
            "Setting up the app failed.\n{message}Fix what az names, then run the commands again, \
             or Esc to follow the browser steps instead."
        ),
        _ => "The commands stopped before they ended, and nothing was picked up. Run them again."
            .to_owned(),
    }
}

#[cfg(test)]
mod tests;
