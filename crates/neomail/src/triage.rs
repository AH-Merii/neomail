//! `neomail mark read|unread`, `neomail archive`, `neomail delete` and `neomail move`: the CLI face
//! of the actions. [`Writer`] is the [`MailWriter`] that reaches the servers with the logins
//! `neomail sync` uses, under the same account locks; [`finish`] prints the report. Online only:
//! nothing is queued.

use std::io::{self, Write};
use std::path::Path;

use anyhow::{Result, bail};
use nm_core::action::{Failure, Report, State};
use nm_core::{
    Action, Change, Destination, Flags, Folder, MailWriter, MessageId, Moved, RemoteId, Uid,
    UidValidity, Written,
};
use serde::Serialize;
use tokio::runtime::Runtime;

use crate::config::{Account, Config, Server};
use crate::graph;
use crate::lock;
use crate::output::{self, Mode};
use crate::style::Paint;
use crate::sync::{AccessToken, Credential, Logins};

/// Writes to the configured accounts' servers.
pub struct Writer<'a> {
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

impl Writer<'_> {
    /// Runs `write` with the IMAP account `name`'s server and login; a failure forgets the login,
    /// so the next account's write starts afresh.
    fn imap<T>(
        &mut self,
        name: &str,
        write: impl FnOnce(&Runtime, &nm_imap::ImapConfig, nm_imap::Login<'_>) -> Result<T>,
    ) -> Result<T> {
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
            .and_then(|credential| write(runtime, imap, credential.imap()));
        for warning in warnings {
            eprintln!("neomail: {name}: {warning}");
        }
        if result.is_err() {
            self.logins.forget(name);
        }
        result
    }

    /// Runs `write` with the Microsoft Graph account `name`'s access token, as [`Self::imap`]
    /// does with an IMAP login.
    fn graph<T>(
        &mut self,
        name: &str,
        write: impl FnOnce(&Runtime, &AccessToken) -> Result<T>,
    ) -> Result<T> {
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
                Credential::AccessToken(token) => write(runtime, token),
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

impl MailWriter for Writer<'_> {
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
        self.imap(name, |runtime, imap, login| {
            Ok(runtime.block_on(nm_imap::store_seen(imap, login, uid_validity, uids, seen))?)
        })
    }

    fn move_by_uid(
        &mut self,
        name: &str,
        uid_validity: UidValidity,
        uids: &[Uid],
        to: &Destination,
    ) -> Result<Moved<Uid>> {
        self.imap(name, |runtime, imap, login| {
            runtime
                .block_on(nm_imap::move_to(imap, login, uid_validity, uids, to))
                .map_err(|e| explain_imap_move(e, name))
        })
    }

    fn move_by_id(
        &mut self,
        name: &str,
        ids: &[RemoteId],
        to: &Destination,
    ) -> Result<Moved<RemoteId>> {
        self.graph(name, |runtime, token| {
            runtime.block_on(graph::move_to(name, token, ids, to))
        })
    }

    fn set_seen_by_id(
        &mut self,
        name: &str,
        ids: &[RemoteId],
        seen: bool,
    ) -> Result<Written<RemoteId>> {
        self.graph(name, |runtime, token| {
            runtime.block_on(graph::set_read(name, token, ids, seen))
        })
    }
}

/// An IMAP error from a move, with what to do next.
fn explain_imap_move(e: nm_imap::ImapError, account: &str) -> anyhow::Error {
    match e {
        e @ (nm_imap::ImapError::NoSuchFolder(_) | nm_imap::ImapError::NotSelectable(_)) => {
            crate::folders::point_at_folder_list(e, account)
        }
        e => e.into(),
    }
}

/// One done message in `--json` and `--jsonl`. A marked one's fields mean what they mean in
/// `list --json`; a moved one names the folder it is now in, as `folder list` shows it.
#[derive(Serialize)]
#[serde(untagged)]
enum Item<'a> {
    Marked {
        id: MessageId,
        account: &'a str,
        unread: bool,
        flags: &'a Flags,
    },
    Moved {
        id: MessageId,
        account: &'a str,
        folder: &'a str,
    },
}

impl Item<'_> {
    fn id(&self) -> MessageId {
        match self {
            Self::Marked { id, .. } | Self::Moved { id, .. } => *id,
        }
    }
}

/// How `action` reads in a sentence: done to `n` messages, the state or folder it asks for, and
/// what may not have happened to some ids.
struct Words {
    change: Change,
}

impl Words {
    fn done(&self, n: usize) -> String {
        let s = if n == 1 { "" } else { "s" };
        match &self.change {
            Change::Seen(seen) => format!("Marked {n} message{s} {}.", state(*seen)),
            Change::Move(Destination::Role(Folder::Archive)) => format!("Archived {n} message{s}."),
            Change::Move(Destination::Role(Folder::Trash)) => {
                format!("Moved {n} message{s} to the trash.")
            }
            Change::Move(Destination::Role(Folder::Junk)) => {
                format!("Moved {n} message{s} to Junk.")
            }
            Change::Move(Destination::Name(name)) => {
                format!(
                    "Moved {n} message{s} to {}.",
                    output::one_line(name.as_str())
                )
            }
        }
    }

    /// Where a move puts a message, in a sentence: "the archive", "the trash" or the name.
    fn to(destination: &Destination) -> String {
        match destination {
            Destination::Role(role) => format!("the {role}"),
            Destination::Name(name) => output::one_line(name.as_str()),
        }
    }

    fn not_confirmed(&self, id: MessageId) -> String {
        match &self.change {
            Change::Seen(seen) => format!(
                "the server did not confirm message {id} as {}; run `neomail sync` to see its \
                 state, then try again",
                state(*seen)
            ),
            Change::Move(to @ Destination::Role(_)) => format!(
                "the server did not confirm message {id} moved to {}; run `neomail sync` to see \
                 where it is, then try again",
                Self::to(to)
            ),
            // Moving into a label (Proton Bridge's Labels/...) may only add the label, so the
            // message stays in the INBOX every time: trying again would not help.
            Change::Move(to @ Destination::Name(_)) => format!(
                "the server did not take message {id} out of the INBOX after moving it to {} \
                 (moving to a label, such as Proton Bridge's Labels/..., may only add the \
                 label); run `neomail sync` to see where it is",
                Self::to(to)
            ),
        }
    }

    fn may_not(&self) -> String {
        match &self.change {
            Change::Seen(seen) => format!("be marked {}", state(*seen)),
            Change::Move(Destination::Role(Folder::Archive)) => "be archived".to_owned(),
            Change::Move(to) => format!("be moved to {}", Self::to(to)),
        }
    }
}

fn state(seen: bool) -> &'static str {
    if seen { "read" } else { "unread" }
}

/// Prints `report`: the results on `out` in `mode`, then one `neomail:` line per failure on
/// `err`. Returns the exit status: 0 when every id is in the asked state, else 1.
pub fn finish(
    action: &Action,
    report: &Report<anyhow::Error>,
    mode: Mode,
    out: &mut dyn Write,
    err: &mut dyn Write,
    paint: Paint,
) -> io::Result<u8> {
    let words = Words {
        change: action.change(),
    };
    let items: Vec<Item<'_>> = report
        .done
        .iter()
        .map(|d| match &d.state {
            State::Flags(flags) => Item::Marked {
                id: d.id,
                account: &d.account,
                unread: !flags.is_seen(),
                flags,
            },
            State::MovedTo(folder) => Item::Moved {
                id: d.id,
                account: &d.account,
                folder,
            },
        })
        .collect();
    output::write_listing(
        &mut &mut *out,
        mode,
        &items,
        |out| writeln!(out, "{}", words.done(items.len())),
        |out| items.iter().try_for_each(|i| writeln!(out, "{}", i.id())),
    )?;
    out.flush()?;
    let prefix = paint.error("neomail");
    for failure in &report.failures {
        let line = match failure {
            Failure::Unknown(id) => format!("no cached message with id {id}; see `neomail list`"),
            Failure::Gone { account, id } => {
                format!("{account}: message {id} is gone from the server; the next sync removes it")
            }
            Failure::NotConfirmed { account, id } => {
                format!("{account}: {}", words.not_confirmed(*id))
            }
            Failure::Account {
                account,
                ids,
                error,
            } => {
                // The server may have changed some of them before the error: see `MailWriter`.
                let list: Vec<String> = ids.iter().map(MessageId::to_string).collect();
                let s = if ids.len() == 1 { "" } else { "s" };
                format!(
                    "{account}: message{s} {} may not {}: {error:#}",
                    list.join(", "),
                    words.may_not()
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
                    state: State::Flags(Flags::new(["\\Flagged", "\\Seen"])),
                },
                Done {
                    id: MessageId::new(2),
                    account: "home".to_owned(),
                    state: State::Flags(Flags::new(["\\Seen"])),
                },
            ],
            failures: Vec::new(),
        }
    }

    fn render(action: Action, report: &Report<anyhow::Error>, mode: Mode) -> (String, String, u8) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = finish(&action, report, mode, &mut out, &mut err, Paint::OFF).unwrap();
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
                state: State::Flags(Flags::default()),
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

    fn moved() -> Report<anyhow::Error> {
        let done = |id, account: &str, folder: &str| Done {
            id: MessageId::new(id),
            account: account.to_owned(),
            state: State::MovedTo(folder.to_owned()),
        };
        Report {
            done: vec![
                done(4, "gmail", "[Gmail]/All Mail"),
                done(2, "proton", "Archive"),
            ],
            failures: Vec::new(),
        }
    }

    #[test]
    fn each_mode_prints_the_moved_messages_and_their_folders() {
        let r = moved();
        assert_eq!(
            render(Action::Archive, &r, Mode::Human),
            ("Archived 2 messages.\n".to_owned(), String::new(), 0)
        );
        assert_eq!(
            render(Action::Delete, &r, Mode::Human).0,
            "Moved 2 messages to the trash.\n"
        );
        assert_eq!(render(Action::Archive, &r, Mode::Plain).0, "4\n2\n");
        let json: serde_json::Value =
            serde_json::from_str(&render(Action::Archive, &r, Mode::Json).0).unwrap();
        assert_eq!(
            json,
            serde_json::json!([
                {"id": 4, "account": "gmail", "folder": "[Gmail]/All Mail"},
                {"id": 2, "account": "proton", "folder": "Archive"},
            ])
        );
        let jsonl = render(Action::Delete, &r, Mode::Jsonl).0;
        let lines: Vec<serde_json::Value> = jsonl
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(serde_json::Value::Array(lines), json);
        let none = Report {
            done: Vec::new(),
            failures: vec![Failure::Unknown(MessageId::new(1))],
        };
        assert_eq!(
            render(Action::Delete, &none, Mode::Human).0,
            "Moved 0 messages to the trash.\n"
        );
        let one = Report {
            done: moved().done.split_off(1),
            failures: Vec::new(),
        };
        assert_eq!(
            render(Action::Archive, &one, Mode::Human).0,
            "Archived 1 message.\n"
        );
    }

    fn receipts() -> Action {
        Action::Move(nm_core::FolderName::new("Folders/Receipts").unwrap())
    }

    #[test]
    fn a_named_move_prints_the_folder_in_each_mode() {
        let done = |id, account: &str| Done {
            id: MessageId::new(id),
            account: account.to_owned(),
            state: State::MovedTo("Folders/Receipts".to_owned()),
        };
        let r = Report {
            done: vec![done(4, "proton"), done(2, "proton")],
            failures: Vec::new(),
        };
        assert_eq!(
            render(receipts(), &r, Mode::Human),
            (
                "Moved 2 messages to Folders/Receipts.\n".to_owned(),
                String::new(),
                0
            )
        );
        assert_eq!(render(receipts(), &r, Mode::Plain).0, "4\n2\n");
        let json: serde_json::Value =
            serde_json::from_str(&render(receipts(), &r, Mode::Json).0).unwrap();
        assert_eq!(
            json,
            serde_json::json!([
                {"id": 4, "account": "proton", "folder": "Folders/Receipts"},
                {"id": 2, "account": "proton", "folder": "Folders/Receipts"},
            ])
        );
        let one = Report {
            done: vec![done(4, "proton")],
            failures: Vec::new(),
        };
        assert_eq!(
            render(receipts(), &one, Mode::Human).0,
            "Moved 1 message to Folders/Receipts.\n"
        );
    }

    #[test]
    fn every_move_failure_is_one_line_that_says_what_to_do_and_exits_1() {
        let failures = || {
            vec![
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
                    ids: vec![MessageId::new(8)],
                    error: anyhow!("refused"),
                },
            ]
        };
        // A named move may be into a label, which stays in the INBOX every time, so its line
        // does not say to try again.
        for (action, not_confirmed, may_not) in [
            (
                Action::Archive,
                "the server did not confirm message 6 moved to the archive; run `neomail sync` \
                 to see where it is, then try again",
                "be archived",
            ),
            (
                Action::Delete,
                "the server did not confirm message 6 moved to the trash; run `neomail sync` to \
                 see where it is, then try again",
                "be moved to the trash",
            ),
            (
                receipts(),
                "the server did not take message 6 out of the INBOX after moving it to \
                 Folders/Receipts (moving to a label, such as Proton Bridge's Labels/..., may \
                 only add the label); run `neomail sync` to see where it is",
                "be moved to Folders/Receipts",
            ),
        ] {
            let r = Report {
                done: Vec::new(),
                failures: failures(),
            };
            let (out, err, code) = render(action, &r, Mode::Plain);
            assert_eq!((out.as_str(), code), ("", 1));
            assert_eq!(
                err.lines().collect::<Vec<_>>(),
                [
                    "neomail: work: message 5 is gone from the server; the next sync removes it"
                        .to_owned(),
                    format!("neomail: work: {not_confirmed}"),
                    format!("neomail: home: message 8 may not {may_not}: refused"),
                ]
            );
        }
    }

    /// On every provider, a folder that is missing or cannot hold messages is one line that
    /// names the account and points at `folder list`; any other move error passes through as it
    /// is.
    #[test]
    fn a_folder_that_cannot_be_moved_to_says_to_list_the_accounts_folders_on_every_provider() {
        let name = |n: &str| nm_core::FolderName::new(n).unwrap();
        let refused: [(&str, anyhow::Error); 4] = [
            (
                "the server has no folder named \"Receipts\"; nothing was moved; run `neomail \
                 folder list --account work` to see its folders",
                explain_imap_move(nm_imap::ImapError::NoSuchFolder(name("Receipts")), "work"),
            ),
            (
                "\"Folders\" cannot hold messages (the server marks it \\Noselect or \\NonExistent); \
                 nothing was moved; run `neomail folder list --account work` to see its folders",
                explain_imap_move(nm_imap::ImapError::NotSelectable(name("Folders")), "work"),
            ),
            (
                "the mailbox has no folder named \"Receipts\"; nothing was moved; run `neomail \
                 folder list --account outlook` to see its folders",
                graph::explain_write(
                    nm_graph::GraphError::NoSuchFolder(name("Receipts")),
                    "outlook",
                ),
            ),
            (
                "\"Unread\" is a search folder, which cannot hold messages; nothing was moved; run \
                 `neomail folder list --account outlook` to see its folders",
                graph::explain_write(
                    nm_graph::GraphError::SearchFolder(name("Unread")),
                    "outlook",
                ),
            ),
        ];
        for (want, e) in refused {
            assert_eq!(e.to_string(), want);
        }
        assert_eq!(
            explain_imap_move(nm_imap::ImapError::IntoInbox, "work").to_string(),
            "the messages are in the INBOX already; nothing was moved"
        );
    }
}
