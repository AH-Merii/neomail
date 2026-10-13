//! `neomail folder list`: each account's folders, read from its server with the logins `neomail
//! sync` uses. A read: it changes nothing on the server and does not touch the cache.

use std::fmt::Display;
use std::io::{self, Write};

use anyhow::{Result, anyhow, bail};
use nm_core::ListedFolder;
use tokio::runtime::Runtime;

use crate::config::{Account, Server};
use crate::graph;
use crate::output::{self, FolderItem, Mode};
use crate::style::Colors;
use crate::sync::{Credential, Logins};

/// The folders a message of `account` can be in, in the server's order.
pub fn list(
    runtime: &Runtime,
    account: &Account,
    logins: &mut Logins,
) -> Result<Vec<ListedFolder>> {
    let mut warnings = Vec::new();
    let result = logins
        .credential(runtime, account, &mut warnings)
        .and_then(|credential| match (&account.server, credential) {
            (Server::Imap(imap), credential) => {
                Ok(runtime.block_on(nm_imap::list_folders(imap, credential.imap()))?)
            }
            (Server::Graph { .. }, Credential::AccessToken(token)) => {
                runtime.block_on(graph::folders(&account.name, token))
            }
            (Server::Graph { .. }, Credential::Password(_)) => {
                bail!("a Microsoft Graph account signs in with OAuth, not a password")
            }
        });
    for warning in warnings {
        eprintln!("neomail: {}: {warning}", account.name);
    }
    if result.is_err() {
        logins.forget(&account.name);
    }
    result
}

/// A move to a folder name `account` cannot take (missing, or a folder that cannot hold messages),
/// with what to do next: every provider's such error goes through here, so they all point at
/// `folder list`, which lists exactly the folders a message can be moved to.
pub fn point_at_folder_list(e: impl Display, account: &str) -> anyhow::Error {
    anyhow!("{e}; run `neomail folder list --account {account}` to see its folders")
}

/// Prints every account's folders on `out` in `mode`, in the order of `listed`, then one
/// `neomail:` line on `err` per account that failed.
/// Returns the exit status: 1 when any failed.
pub fn finish(
    listed: &[(&str, Result<Vec<ListedFolder>>)],
    mode: Mode,
    out: &mut dyn Write,
    err: &mut dyn Write,
    colors: Colors,
) -> io::Result<u8> {
    let items: Vec<FolderItem<'_>> = listed
        .iter()
        .filter_map(|(account, result)| Some((*account, result.as_ref().ok()?)))
        .flat_map(|(account, folders)| {
            folders.iter().map(move |f| FolderItem {
                name: f.name.as_str(),
                account,
                role: f.role.map(nm_core::Role::name),
            })
        })
        .collect();
    output::write_listing(
        &mut &mut *out,
        mode,
        &items,
        |out| output::write_folders(out, &items, colors.stdout),
        |out| output::write_folders_plain(out, &items, colors.plain),
    )?;
    out.flush()?;
    let prefix = colors.stderr.error("neomail");
    let mut failed = false;
    for (account, result) in listed {
        if let Err(e) = result {
            writeln!(err, "{prefix}: {account}: {e:#}")?;
            failed = true;
        }
    }
    Ok(u8::from(failed))
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;
    use nm_core::{FolderName, Role};

    use super::*;
    use crate::style::Paint;

    const OFF: Colors = Colors {
        stdout: Paint::OFF,
        plain: Paint::OFF,
        stderr: Paint::OFF,
    };

    fn folder(name: &str, role: Option<Role>) -> ListedFolder {
        ListedFolder {
            name: FolderName::new(name).unwrap(),
            role,
        }
    }

    fn listed() -> Vec<(&'static str, Result<Vec<ListedFolder>>)> {
        vec![
            (
                "proton",
                Ok(vec![
                    folder("INBOX", Some(Role::Inbox)),
                    folder("Folders/Re\u{E7}us", None),
                ]),
            ),
            (
                "outlook",
                Err(anyhow!("access token refused; run the command again")),
            ),
            (
                "gmail",
                Ok(vec![folder("[Gmail]/All Mail", Some(Role::All))]),
            ),
        ]
    }

    fn render(mode: Mode) -> (String, String, u8) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = finish(&listed(), mode, &mut out, &mut err, OFF).unwrap();
        (
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
            code,
        )
    }

    #[test]
    fn each_mode_prints_the_folders_and_a_failed_account_is_one_line_after_them() {
        let error = "neomail: outlook: access token refused; run the command again\n";
        assert_eq!(
            render(Mode::Human),
            (
                "INBOX             proton  inbox\n\
                 Folders/Re\u{E7}us     proton\n\
                 [Gmail]/All Mail  gmail   all\n"
                    .to_owned(),
                error.to_owned(),
                1
            )
        );
        assert_eq!(
            render(Mode::Plain).0,
            "INBOX\tproton\tinbox\nFolders/Re\u{E7}us\tproton\t\n[Gmail]/All Mail\tgmail\tall\n"
        );
        let json: serde_json::Value = serde_json::from_str(&render(Mode::Json).0).unwrap();
        assert_eq!(
            json,
            serde_json::json!([
                {"name": "INBOX", "account": "proton", "role": "inbox"},
                {"name": "Folders/Re\u{E7}us", "account": "proton", "role": null},
                {"name": "[Gmail]/All Mail", "account": "gmail", "role": "all"},
            ])
        );
        let (jsonl, err, code) = render(Mode::Jsonl);
        let lines: Vec<serde_json::Value> = jsonl
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(serde_json::Value::Array(lines), json);
        assert_eq!((err.as_str(), code), (error, 1));
    }

    #[test]
    fn every_account_listed_exits_0() {
        let ok = vec![("gmail", Ok(vec![folder("INBOX", Some(Role::Inbox))]))];
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = finish(&ok, Mode::Plain, &mut out, &mut err, OFF).unwrap();
        assert_eq!(
            (out.as_slice(), err.as_slice(), code),
            (&b"INBOX\tgmail\tinbox\n"[..], &b""[..], 0)
        );
    }
}
