//! The label writes on the wire, against a scripted server that answers as Gmail, Proton Mail
//! Bridge or a plain IMAP server would: which commands go out, and that none of them can take a
//! label neomail did not write, replace a message's labels, or remove a message.
#![allow(clippy::unwrap_used)] // Test helpers outside #[test] functions.

use std::collections::BTreeMap;

use async_imap::Client;
use nm_core::label::LabelPrefix;
use nm_core::{Uid, UidValidity};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

use super::{LabelEdit, LabelKind, LabelsWritten, NoJunk, Takes, probe_on, write_labels_on};
use crate::{ImapError, Login, greeting, log_in};

const GMAIL_CAPABILITY: &str = "IMAP4rev1 UNSELECT IDLE NAMESPACE QUOTA ID XLIST CHILDREN \
    X-GM-EXT-1 UIDPLUS COMPRESS=DEFLATE ENABLE MOVE CONDSTORE ESEARCH UTF8=ACCEPT LIST-EXTENDED \
    LIST-STATUS LITERAL- SPECIAL-USE APPENDLIMIT=35651584";

/// Gluon's, as docs/research/proton.md lists it from its source.
const BRIDGE_CAPABILITY: &str = "IMAP4rev1 UNSELECT UIDPLUS MOVE ID IDLE AUTH=PLAIN";

const GMAIL_LIST: &str = "* LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\n\
    * LIST (\\HasNoChildren) \"/\" \"Red\"\r\n\
    * LIST (\\HasNoChildren) \"/\" \"nm-main\"\r\n\
    * LIST (\\HasChildren \\Noselect) \"/\" \"[Gmail]\"\r\n\
    * LIST (\\All \\HasNoChildren) \"/\" \"[Gmail]/All Mail\"\r\n\
    * LIST (\\HasNoChildren \\Trash) \"/\" \"[Gmail]/Trash\"\r\n\
    * LIST (\\HasNoChildren \\Junk) \"/\" \"[Gmail]/Spam\"\r\n";

const BRIDGE_LIST: &str = "* LIST (\\Noselect) \"/\" \"Folders\"\r\n\
    * LIST (\\Noselect) \"/\" \"Labels\"\r\n\
    * LIST () \"/\" \"Labels/nm-main\"\r\n\
    * LIST () \"/\" \"Labels/nm-receipts-invoices\"\r\n\
    * LIST () \"/\" \"Labels/keep\"\r\n\
    * LIST () \"/\" \"INBOX\"\r\n\
    * LIST (\\Archive) \"/\" \"Archive\"\r\n\
    * LIST (\\Trash) \"/\" \"Trash\"\r\n\
    * LIST (\\All) \"/\" \"All Mail\"\r\n\
    * LIST (\\Junk) \"/\" \"Spam\"\r\n";

/// A server's state and answers. INBOX's messages have a Message-ID and Gmail labels; each
/// `Labels/` mailbox holds (UID, Message-ID) pairs. The selected mailbox decides what UID FETCH,
/// SEARCH, STORE and EXPUNGE see.
struct Script {
    capability: &'static str,
    list: &'static str,
    inbox: Vec<(u32, String, Vec<&'static str>)>,
    labels: BTreeMap<String, Vec<(u32, String)>>,
    selected: String,
    /// INBOX's UIDVALIDITY.
    validity: u32,
}

impl Script {
    fn new(capability: &'static str, list: &'static str) -> Self {
        Self {
            capability,
            list,
            inbox: Vec::new(),
            labels: BTreeMap::new(),
            selected: String::new(),
            validity: 7,
        }
    }

    fn message(mut self, uid: u32, id: &str, labels: &[&'static str]) -> Self {
        self.inbox.push((uid, id.to_owned(), labels.to_vec()));
        self
    }

    fn labelled(mut self, label: &str, copies: &[(u32, &str)]) -> Self {
        let copies = copies.iter().map(|&(u, id)| (u, id.to_owned())).collect();
        self.labels.insert(format!("Labels/{label}"), copies);
        self
    }

    /// The selected mailbox's (UID, Message-ID) pairs.
    fn selected(&self) -> Vec<(u32, String)> {
        if self.selected == "INBOX" {
            self.inbox
                .iter()
                .map(|(u, id, _)| (*u, id.clone()))
                .collect()
        } else {
            self.labels.get(&self.selected).cloned().unwrap_or_default()
        }
    }
}

/// The UIDs of a UID command: its third word, as `1,5,9`.
fn set_of(command: &str) -> Vec<u32> {
    let set = command.split_whitespace().nth(2).unwrap();
    set.split(',').map(|u| u.parse().unwrap()).collect()
}

/// The quoted argument after `prefix` in `command`, unescaped.
fn quoted_after(command: &str, prefix: &str) -> String {
    let rest = &command[command.find(prefix).unwrap() + prefix.len()..];
    let rest = rest.trim_start().strip_prefix('"').unwrap();
    rest[..rest.rfind('"').unwrap()]
        .replace("\\\"", "\"")
        .replace("\\\\", "\\")
}

async fn server(stream: DuplexStream, mut script: Script) -> Vec<String> {
    let (read, mut write) = tokio::io::split(stream);
    let mut lines = BufReader::new(read).lines();
    let mut seen = Vec::new();
    write
        .write_all(b"* OK scripted IMAP ready\r\n")
        .await
        .unwrap();
    while let Ok(Some(line)) = lines.next_line().await {
        let (tag, command) = line.split_once(' ').unwrap_or((&line, ""));
        let upper = command.to_ascii_uppercase();
        seen.push(command.to_owned());
        let untagged = if upper.starts_with("LOGIN") || upper.starts_with("CREATE") {
            String::new()
        } else if upper == "CAPABILITY" {
            format!("* CAPABILITY {}\r\n", script.capability)
        } else if upper.starts_with("LIST") {
            script.list.to_owned()
        } else if upper.starts_with("SELECT") {
            script.selected = quoted_after(command, "SELECT");
            let validity = if script.selected == "INBOX" {
                script.validity
            } else {
                9
            };
            let exists = script.selected().len();
            format!("* {exists} EXISTS\r\n* OK [UIDVALIDITY {validity}] ok\r\n")
        } else if upper.starts_with("UID FETCH") && upper.contains("X-GM-LABELS") {
            set_of(command)
                .into_iter()
                .filter_map(|u| {
                    let n = script.inbox.iter().position(|m| m.0 == u)?;
                    let labels = script.inbox[n].2.join(" ");
                    Some(format!(
                        "* {} FETCH (UID {u} X-GM-LABELS ({labels}))\r\n",
                        n + 1
                    ))
                })
                .collect()
        } else if upper.starts_with("UID FETCH") && upper.contains("HEADER.FIELDS") {
            let here = script.selected();
            set_of(command)
                .into_iter()
                .filter_map(|u| {
                    let n = here.iter().position(|m| m.0 == u)?;
                    let header = format!("Message-ID: <{}>\r\n\r\n", here[n].1);
                    Some(format!(
                        "* {} FETCH (UID {u} BODY[HEADER.FIELDS (MESSAGE-ID)] {{{}}}\r\n{header})\r\n",
                        n + 1,
                        header.len()
                    ))
                })
                .collect()
        } else if upper.starts_with("UID FETCH") {
            let here = script.selected();
            set_of(command)
                .into_iter()
                .filter_map(|u| {
                    let n = here.iter().position(|m| m.0 == u)? + 1;
                    Some(format!("* {n} FETCH (UID {u} FLAGS ())\r\n"))
                })
                .collect()
        } else if upper.starts_with("UID SEARCH") {
            // A substring match, as RFC 3501 section 6.4.4 has SEARCH HEADER do.
            let wanted = quoted_after(command, "Message-ID");
            let hits: Vec<String> = script
                .selected()
                .iter()
                .filter(|(_, id)| id.contains(&wanted))
                .map(|(u, _)| u.to_string())
                .collect();
            format!("* SEARCH {}\r\n", hits.join(" "))
        } else if upper.starts_with("UID STORE") || upper.starts_with("UID COPY") {
            String::new()
        } else if upper.starts_with("UID EXPUNGE") {
            let asked = set_of(command);
            let mailbox = script.labels.get_mut(&script.selected).unwrap();
            let mut untagged = String::new();
            for u in asked {
                if let Some(n) = mailbox.iter().position(|m| m.0 == u) {
                    mailbox.remove(n);
                    untagged += &format!("* {} EXPUNGE\r\n", n + 1);
                }
            }
            untagged
        } else if upper.starts_with("UID MOVE") {
            let asked = set_of(command);
            let mut untagged = String::new();
            while let Some(n) = script.inbox.iter().rposition(|m| asked.contains(&m.0)) {
                script.inbox.remove(n);
                untagged += &format!("* {} EXPUNGE\r\n", n + 1);
            }
            untagged
        } else if upper.starts_with("LOGOUT") {
            "* BYE logging out\r\n".to_owned()
        } else {
            let reply = format!("{tag} BAD not scripted\r\n");
            write.write_all(reply.as_bytes()).await.unwrap();
            continue;
        };
        let reply = format!("{untagged}{tag} OK done\r\n");
        write.write_all(reply.as_bytes()).await.unwrap();
        if upper.starts_with("LOGOUT") {
            break;
        }
    }
    seen
}

/// A logged-in session against [`server`] running `script`.
async fn session(
    script: Script,
) -> (
    async_imap::Session<DuplexStream>,
    tokio::task::JoinHandle<Vec<String>>,
) {
    let (client_end, server_end) = tokio::io::duplex(64 * 1024);
    let server = tokio::spawn(server(server_end, script));
    let mut client = Client::new(client_end);
    greeting(&mut client).await.unwrap();
    let session = log_in(client, "user", Login::Password("password"))
        .await
        .unwrap();
    (session, server)
}

fn edit<'a>(uid: u32, want: Option<&'a str>, last: Option<&'a str>) -> LabelEdit<'a> {
    LabelEdit {
        uid: Uid::new(uid).unwrap(),
        want,
        last,
        message_id: None,
        junk: false,
    }
}

/// Writes `edits` with the default prefix and `bridge` as the config's shape; the result and
/// every command after LOGIN.
async fn write(
    script: Script,
    bridge: bool,
    edits: &[LabelEdit<'_>],
) -> (Result<LabelsWritten, ImapError>, Vec<String>) {
    let (session, server) = session(script).await;
    let validity = UidValidity::new(7).unwrap();
    let result = write_labels_on(session, bridge, validity, &LabelPrefix::default(), edits).await;
    let mut commands = server.await.unwrap();
    commands.remove(0);
    (result, commands)
}

/// The verb of a command line, past a `UID` prefix.
fn verb(command: &str) -> String {
    let mut words = command.split_whitespace();
    match words.next() {
        Some(w) if w.eq_ignore_ascii_case("UID") => words.next().unwrap_or("").to_ascii_uppercase(),
        Some(w) => w.to_ascii_uppercase(),
        None => String::new(),
    }
}

/// No command can remove a message, rename or delete a mailbox, or replace a Gmail message's
/// labels: no plain EXPUNGE, no CLOSE, DELETE or RENAME, and every X-GM-LABELS STORE adds or
/// removes. A `\Deleted` is stored, and a UID EXPUNGE sent, only while a `Labels/` mailbox is
/// selected, and the EXPUNGE names exactly the UIDs marked.
fn assert_cannot_lose_mail(commands: &[String]) {
    let mut selected = String::new();
    let mut marked: Vec<u32> = Vec::new();
    for c in commands {
        let upper = c.to_ascii_uppercase();
        let verb = verb(c);
        assert!(
            !["CLOSE", "DELETE", "RENAME"].contains(&verb.as_str()),
            "sent {c}"
        );
        if verb == "EXPUNGE" {
            assert!(upper.starts_with("UID EXPUNGE"), "a plain EXPUNGE: {c}");
            assert!(
                selected.starts_with("Labels/"),
                "expunged in {selected}: {c}"
            );
            assert_eq!(set_of(c), marked, "expunged other UIDs: {c}");
        }
        if verb == "SELECT" {
            selected = quoted_after(c, "SELECT");
            marked.clear();
        }
        if upper.contains("\\DELETED") {
            assert!(
                selected.starts_with("Labels/"),
                "\\Deleted in {selected}: {c}"
            );
            marked = set_of(c);
        }
        if upper.contains("X-GM-LABELS") && verb == "STORE" {
            assert!(
                upper.contains("+X-GM-LABELS") || upper.contains("-X-GM-LABELS"),
                "replaced a message's labels: {c}"
            );
        }
    }
}

#[tokio::test]
async fn gmail_adds_and_removes_single_names_and_never_names_the_user_s() {
    let script = Script::new(GMAIL_CAPABILITY, GMAIL_LIST)
        .message(1, "a@x", &["\\Inbox", "Red", "nm-follow-up"])
        .message(2, "b@x", &["\\Inbox", "\\Important"])
        .message(3, "c@x", &["\\Inbox", "nm-spam"])
        .message(4, "d@x", &["\\Inbox", "old-main", "\"nm-main\""]);
    let edits = [
        edit(1, Some("nm-follow-up-old"), Some("nm-follow-up")),
        edit(2, Some("nm-main"), None),
        edit(3, Some("nm-spam"), Some("nm-spam")),
        // A name written under an older prefix goes too: it is the exact one neomail wrote.
        edit(4, Some("nm-spam"), Some("old-main")),
    ];
    let (written, commands) = write(script, false, &edits).await;
    let written = written.unwrap();
    assert_eq!(
        written.labelled,
        [(0, true), (1, true), (2, false), (3, true)]
    );
    assert!(written.gone.is_empty() && written.kept.is_empty());
    assert_eq!(
        commands,
        [
            "CAPABILITY",
            "LIST \"\" *",
            "SELECT \"INBOX\"",
            "UID FETCH 1,2,3,4 (UID FLAGS)",
            "UID FETCH 1,2,3,4 (UID X-GM-LABELS)",
            // nm-main is listed; the others are created first.
            "CREATE \"nm-follow-up-old\"",
            "CREATE \"nm-spam\"",
            // Removals before additions.
            "UID STORE 1 -X-GM-LABELS (\"nm-follow-up\")",
            "UID STORE 4 -X-GM-LABELS (\"nm-main\")",
            "UID STORE 4 -X-GM-LABELS (\"old-main\")",
            "UID STORE 1 +X-GM-LABELS (\"nm-follow-up-old\")",
            "UID STORE 2 +X-GM-LABELS (\"nm-main\")",
            "UID STORE 4 +X-GM-LABELS (\"nm-spam\")",
            "LOGOUT",
        ]
    );
    assert_cannot_lose_mail(&commands);
}

#[tokio::test]
async fn a_message_already_labelled_as_wanted_gets_no_store() {
    let script =
        Script::new(GMAIL_CAPABILITY, GMAIL_LIST).message(1, "a@x", &["\\Inbox", "nm-main", "Red"]);
    let (written, commands) = write(script, false, &[edit(1, Some("nm-main"), None)]).await;
    assert_eq!(written.unwrap().labelled, [(0, false)]);
    assert!(
        !commands
            .iter()
            .any(|c| verb(c) == "STORE" || verb(c) == "CREATE"),
        "{commands:?}"
    );
}

#[tokio::test]
async fn spam_is_labelled_then_moved_to_junk_and_a_message_gone_from_inbox_is_left() {
    let script = Script::new(GMAIL_CAPABILITY, GMAIL_LIST)
        .message(1, "a@x", &["\\Inbox"])
        .message(2, "b@x", &["\\Inbox", "nm-spam"]);
    let mut spam = edit(1, Some("nm-spam"), None);
    spam.junk = true;
    // Labelled earlier; only its move is due.
    let mut labelled = edit(2, None, Some("nm-spam"));
    labelled.junk = true;
    let mut gone = edit(9, Some("nm-spam"), None);
    gone.junk = true;
    let (written, commands) = write(script, false, &[spam, labelled, gone]).await;
    let written = written.unwrap();
    assert_eq!(written.moved, [0, 1]);
    assert_eq!(written.gone, [2]);
    let at = |prefix: &str| commands.iter().position(|c| c.starts_with(prefix)).unwrap();
    assert!(at("UID STORE 1 +X-GM-LABELS") < at("UID MOVE 1,2 \"[Gmail]/Spam\""));
    // UID 9 is asked about once, and nothing is written for it.
    let nine = commands
        .iter()
        .filter(|c| c.starts_with("UID ") && set_of(c).contains(&9))
        .collect::<Vec<_>>();
    assert_eq!(nine, ["UID FETCH 1,2,9 (UID FLAGS)"]);
    assert_cannot_lose_mail(&commands);
}

#[tokio::test]
async fn proton_copies_into_the_label_and_expunges_only_the_earlier_label_s_copy() {
    let script = Script::new(BRIDGE_CAPABILITY, BRIDGE_LIST)
        .message(1, "a@x", &[])
        .message(2, "b@x", &[])
        // In the old label: message 1's copy, and another message whose Message-ID holds 1's.
        .labelled(
            "nm-receipts-invoices",
            &[(50, "a@x"), (51, "xa@x"), (52, "c@x")],
        )
        .labelled("keep", &[(60, "a@x")]);
    let mut first = edit(1, Some("nm-newsletters"), Some("nm-receipts-invoices"));
    first.message_id = Some("a@x");
    let mut second = edit(2, Some("nm-main"), None);
    second.message_id = Some("b@x");
    let (written, commands) = write(script, true, &[first, second]).await;
    let written = written.unwrap();
    assert_eq!(written.labelled, [(0, true), (1, true)]);
    assert!(written.kept.is_empty());
    assert_eq!(
        commands,
        [
            "CAPABILITY",
            "LIST \"\" *",
            "SELECT \"INBOX\"",
            "UID FETCH 1,2 (UID FLAGS)",
            "UID COPY 2 \"Labels/nm-main\"",
            "CREATE \"Labels/nm-newsletters\"",
            "UID COPY 1 \"Labels/nm-newsletters\"",
            "SELECT \"Labels/nm-receipts-invoices\"",
            "UID SEARCH HEADER Message-ID \"a@x\"",
            "UID FETCH 50,51 (UID BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)])",
            "UID STORE 50 +FLAGS.SILENT (\\Deleted)",
            "UID EXPUNGE 50",
            "LOGOUT",
        ]
    );
    assert_cannot_lose_mail(&commands);
}

#[tokio::test]
async fn without_uidplus_or_a_message_id_proton_s_earlier_label_stays() {
    let no_uidplus = "IMAP4rev1 UNSELECT MOVE ID IDLE AUTH=PLAIN";
    let script = Script::new(no_uidplus, BRIDGE_LIST)
        .message(1, "a@x", &[])
        .labelled("nm-main", &[(50, "a@x")]);
    let mut first = edit(1, Some("nm-spam"), Some("nm-main"));
    first.message_id = Some("a@x");
    let (written, commands) = write(script, true, &[first]).await;
    assert_eq!(written.unwrap().kept, [0]);
    // A message without a Message-ID cannot be found in the label's mailbox.
    let script = Script::new(BRIDGE_CAPABILITY, BRIDGE_LIST)
        .message(1, "a@x", &[])
        .labelled("nm-main", &[(50, "a@x")]);
    let (written, more) = write(script, true, &[edit(1, Some("nm-spam"), Some("nm-main"))]).await;
    assert_eq!(written.unwrap().kept, [0]);
    for c in commands.iter().chain(&more) {
        let v = verb(c);
        assert!(
            !["STORE", "EXPUNGE", "SEARCH"].contains(&v.as_str()),
            "sent {c}"
        );
        assert!(c != "SELECT \"Labels/nm-main\"", "sent {c}");
    }
    assert_cannot_lose_mail(&commands);
}

#[tokio::test]
async fn a_server_that_is_neither_gmail_nor_bridge_gets_no_labels_but_moves_spam() {
    // Bridge's LIST without Bridge's shape in the config: a plain server with such folders.
    let script = Script::new(BRIDGE_CAPABILITY, BRIDGE_LIST).message(1, "a@x", &[]);
    let mut spam = edit(1, Some("nm-spam"), None);
    spam.junk = true;
    let (written, commands) = write(script, false, &[spam]).await;
    let written = written.unwrap();
    assert_eq!(written.takes.unwrap().labels, LabelKind::None);
    assert!(written.labelled.is_empty());
    // Its label is not written, so it does not move either: the pass asks for moves alone.
    assert!(written.moved.is_empty());
    assert!(
        !commands
            .iter()
            .any(|c| ["STORE", "COPY", "CREATE", "MOVE"].contains(&verb(c).as_str())),
        "{commands:?}"
    );
    let script = Script::new(BRIDGE_CAPABILITY, BRIDGE_LIST).message(1, "a@x", &[]);
    let mut spam = edit(1, None, None);
    spam.junk = true;
    let (written, commands) = write(script, false, &[spam]).await;
    assert_eq!(written.unwrap().moved, [0]);
    assert!(commands.contains(&"UID MOVE 1 \"Spam\"".to_owned()));
    assert_cannot_lose_mail(&commands);
}

#[tokio::test]
async fn a_changed_uidvalidity_writes_nothing() {
    let mut script = Script::new(GMAIL_CAPABILITY, GMAIL_LIST).message(1, "a@x", &["\\Inbox"]);
    script.validity = 8;
    let mut spam = edit(1, Some("nm-spam"), None);
    spam.junk = true;
    let (written, commands) = write(script, false, &[spam]).await;
    assert!(matches!(written, Err(ImapError::UidValidityChanged)));
    assert!(
        !commands
            .iter()
            .any(|c| ["STORE", "CREATE", "MOVE"].contains(&verb(c).as_str())),
        "{commands:?}"
    );
}

async fn takes(capability: &'static str, list: &'static str, bridge: bool) -> Takes {
    let (session, server) = session(Script::new(capability, list)).await;
    let takes = probe_on(session, bridge).await.unwrap();
    let commands = server.await.unwrap();
    assert_eq!(&commands[1..], ["CAPABILITY", "LIST \"\" *", "LOGOUT"]);
    takes
}

#[tokio::test]
async fn the_probe_tells_gmail_proton_and_other_servers_apart_and_where_spam_can_go() {
    let gmail = takes(GMAIL_CAPABILITY, GMAIL_LIST, false).await;
    assert_eq!(
        gmail,
        Takes {
            labels: LabelKind::Gmail,
            no_junk: None,
            uidplus: true
        }
    );
    let bridge = takes(BRIDGE_CAPABILITY, BRIDGE_LIST, true).await;
    assert_eq!(bridge.labels, LabelKind::Proton);
    assert_eq!(bridge.no_junk, None);
    // Bridge's folders, but not Bridge's shape in the config; or Bridge's shape without them.
    assert_eq!(
        takes(BRIDGE_CAPABILITY, BRIDGE_LIST, false).await.labels,
        LabelKind::None
    );
    let plain_list = "* LIST () \"/\" \"INBOX\"\r\n* LIST (\\Junk) \"/\" \"Junk\"\r\n";
    assert_eq!(
        takes(BRIDGE_CAPABILITY, plain_list, true).await.labels,
        LabelKind::None
    );
    assert_eq!(
        takes("IMAP4rev1 UIDPLUS", plain_list, false).await.no_junk,
        Some(NoJunk::NoMove)
    );
    let no_junk = "* LIST () \"/\" \"INBOX\"\r\n* LIST (\\Trash) \"/\" \"Trash\"\r\n";
    assert_eq!(
        takes(BRIDGE_CAPABILITY, no_junk, false).await.no_junk,
        Some(NoJunk::NoFolder)
    );
}
