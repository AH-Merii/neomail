//! `neomail mark read|unread`: the CLI face of the mark actions. [`Marker`] is the
//! [`MailWriter`] that reaches the servers with the logins `neomail sync` uses, under the same
//! account locks; [`finish`] prints the report. Online only: nothing is queued.

use std::io::{self, Write};
use std::path::Path;

use anyhow::{Result, bail};
use nm_core::action::{Failure, Report};
use nm_core::{Action, Flags, MailWriter, MessageId, RemoteId, Uid, UidValidity, Written};
use serde::Serialize;
use tokio::runtime::Runtime;

use crate::config::{Account, Config, Server};
use crate::graph;
use crate::lock;
use crate::output::{self, Mode};
use crate::style::Paint;
use crate::sync::{Credential, Logins};

/// Writes to the configured accounts' servers.
pub struct Marker<'a> {
    pub runtime: &'a Runtime,
    pub config: &'a Config,
    pub data_dir: &'a Path,
    pub logins: Logins,
}

/// The configured account `name`, whose messages are in the cache.
fn configured<'c>(config: &'c Config, name: &str) -> Result<&'c Account> {
    match config.accounts.iter().find(|a| a.name == name) {
        Some(account) => Ok(account),
        None => bail!("no account named {name:?} in the config; its cached mail cannot be changed"),
    }
}

impl MailWriter for Marker<'_> {
    type Lock = lock::Held;
    type Error = anyhow::Error;

    fn lock(&mut self, account: &str) -> Result<lock::Held> {
        lock::wait_for_account(self.data_dir, account, || {
            eprintln!("neomail: waiting for neomail daemon to finish syncing {account}");
        })
    }

    fn set_seen_by_uid(
        &mut self,
        name: &str,
        uid_validity: UidValidity,
        uids: &[Uid],
        seen: bool,
    ) -> Result<Written<Uid>> {
        let account = configured(self.config, name)?;
        let Server::Imap(imap) = &account.server else {
            bail!(
                "the cache holds IMAP mail for this account, which now reads Microsoft Graph; run `neomail sync`"
            );
        };
        let runtime = self.runtime;
        let mut warnings = Vec::new();
        let result = self
            .logins
            .credential(runtime, account, &mut warnings)
            .and_then(|credential| {
                let login = credential.imap();
                Ok(runtime.block_on(nm_imap::store_seen(imap, login, uid_validity, uids, seen))?)
            });
        for warning in warnings {
            eprintln!("neomail: {name}: {warning}");
        }
        if result.is_err() {
            self.logins.forget(name);
        }
        result
    }

    fn set_seen_by_id(
        &mut self,
        name: &str,
        ids: &[RemoteId],
        seen: bool,
    ) -> Result<Written<RemoteId>> {
        let account = configured(self.config, name)?;
        if !matches!(account.server, Server::Graph { .. }) {
            bail!(
                "the cache holds Microsoft Graph mail for this account, which now reads IMAP; run `neomail sync`"
            );
        }
        let runtime = self.runtime;
        let mut warnings = Vec::new();
        let result = self
            .logins
            .credential(runtime, account, &mut warnings)
            .and_then(|credential| match credential {
                Credential::AccessToken(token) => {
                    runtime.block_on(graph::set_read(name, token, ids, seen))
                }
                Credential::Password(_) => {
                    bail!("a Microsoft Graph account signs in with OAuth, not a password")
                }
            });
        for warning in warnings {
            eprintln!("neomail: {name}: {warning}");
        }
        if result.is_err() {
            self.logins.forget(name);
        }
        result
    }
}

/// One marked message in `--json` and `--jsonl`: the fields mean what they mean in `list --json`.
#[derive(Serialize)]
struct MarkItem<'a> {
    id: MessageId,
    account: &'a str,
    unread: bool,
    flags: &'a Flags,
}

/// Prints `report`: the results on `out` in `mode`, then one `neomail:` line per failure on
/// `err`. Returns the exit status: 0 when every id is in the asked state, else 1.
pub fn finish(
    action: Action,
    report: &Report<anyhow::Error>,
    mode: Mode,
    out: &mut dyn Write,
    err: &mut dyn Write,
    paint: Paint,
) -> io::Result<u8> {
    let state = if action.seen() { "read" } else { "unread" };
    let items: Vec<MarkItem<'_>> = report
        .done
        .iter()
        .map(|d| MarkItem {
            id: d.id,
            account: &d.account,
            unread: !d.flags.is_seen(),
            flags: &d.flags,
        })
        .collect();
    output::write_listing(
        &mut &mut *out,
        mode,
        &items,
        |out| {
            let n = items.len();
            let s = if n == 1 { "" } else { "s" };
            writeln!(out, "Marked {n} message{s} {state}.")
        },
        |out| items.iter().try_for_each(|i| writeln!(out, "{}", i.id)),
    )?;
    out.flush()?;
    let prefix = paint.error("neomail");
    for failure in &report.failures {
        let line = match failure {
            Failure::Unknown(id) => format!("no cached message with id {id}; see `neomail list`"),
            Failure::Gone { account, id } => {
                format!("{account}: message {id} is gone from the server; the next sync removes it")
            }
            Failure::NotConfirmed { account, id } => format!(
                "{account}: the server did not confirm message {id} as {state}; run `neomail \
                 sync` to see its state, then try again"
            ),
            Failure::Account {
                account,
                ids,
                error,
            } => {
                // The server may have changed some of them before the error: see `MailWriter`.
                let list: Vec<String> = ids.iter().map(MessageId::to_string).collect();
                let s = if ids.len() == 1 { "" } else { "s" };
                format!(
                    "{account}: message{s} {} may not be marked {state}: {error:#}",
                    list.join(", ")
                )
            }
        };
        writeln!(err, "{prefix}: {line}")?;
    }
    Ok(u8::from(!report.failures.is_empty()))
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;
    use nm_core::action::Done;

    use super::*;

    fn report() -> Report<anyhow::Error> {
        Report {
            done: vec![
                Done {
                    id: MessageId::new(4),
                    account: "work".to_owned(),
                    flags: Flags::new(["\\Flagged", "\\Seen"]),
                },
                Done {
                    id: MessageId::new(2),
                    account: "home".to_owned(),
                    flags: Flags::new(["\\Seen"]),
                },
            ],
            failures: Vec::new(),
        }
    }

    fn render(action: Action, report: &Report<anyhow::Error>, mode: Mode) -> (String, String, u8) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = finish(action, report, mode, &mut out, &mut err, Paint::OFF).unwrap();
        (
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
            code,
        )
    }

    #[test]
    fn each_mode_prints_the_marked_messages_only() {
        let r = report();
        assert_eq!(
            render(Action::MarkRead, &r, Mode::Human),
            ("Marked 2 messages read.\n".to_owned(), String::new(), 0)
        );
        assert_eq!(render(Action::MarkRead, &r, Mode::Plain).0, "4\n2\n");
        let json: serde_json::Value =
            serde_json::from_str(&render(Action::MarkRead, &r, Mode::Json).0).unwrap();
        assert_eq!(
            json,
            serde_json::json!([
                {"id": 4, "account": "work", "unread": false, "flags": ["\\Flagged", "\\Seen"]},
                {"id": 2, "account": "home", "unread": false, "flags": ["\\Seen"]},
            ])
        );
        let jsonl = render(Action::MarkRead, &r, Mode::Jsonl).0;
        let lines: Vec<serde_json::Value> = jsonl
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(serde_json::Value::Array(lines), json);
        let one = Report {
            done: vec![Done {
                id: MessageId::new(7),
                account: "work".to_owned(),
                flags: Flags::default(),
            }],
            failures: Vec::new(),
        };
        assert_eq!(
            render(Action::MarkUnread, &one, Mode::Human).0,
            "Marked 1 message unread.\n"
        );
        let json = render(Action::MarkUnread, &one, Mode::Json).0;
        assert!(json.contains("\"unread\": true"), "{json}");
    }

    #[test]
    fn every_failure_is_one_line_on_standard_error_and_exits_1() {
        let mut r = report();
        r.failures = vec![
            Failure::Unknown(MessageId::new(99)),
            Failure::Gone {
                account: "work".to_owned(),
                id: MessageId::new(5),
            },
            Failure::NotConfirmed {
                account: "work".to_owned(),
                id: MessageId::new(6),
            },
            Failure::Account {
                account: "home".to_owned(),
                ids: vec![MessageId::new(8), MessageId::new(9)],
                error: anyhow!("refused").context("cannot connect"),
            },
        ];
        let (out, err, code) = render(Action::MarkRead, &r, Mode::Plain);
        assert_eq!(code, 1);
        assert_eq!(out, "4\n2\n", "the others are still printed");
        assert_eq!(
            err.lines().collect::<Vec<_>>(),
            [
                "neomail: no cached message with id 99; see `neomail list`",
                "neomail: work: message 5 is gone from the server; the next sync removes it",
                "neomail: work: the server did not confirm message 6 as read; run `neomail sync` \
                 to see its state, then try again",
                "neomail: home: messages 8, 9 may not be marked read: cannot connect: refused",
            ]
        );
    }
}
