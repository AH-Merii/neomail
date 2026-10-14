//! Groups as Gmail and Proton labels, and confident spam moved to Junk
//! (DEC-2026-10-14-labels-junk): [`probe`] says what a server takes, and [`write_labels`] writes
//! one batch of INBOX messages in one session.
//!
//! What it sends, and why none of it can lose mail:
//! - Gmail (`X-GM-EXT-1`): `UID STORE` with `-X-GM-LABELS` and `+X-GM-LABELS` and one name each,
//!   so a message's set of labels is never replaced, and the names are the ones [`label::plan`]
//!   picks, so a label of the user's is never named. A missing label is created first: Google's
//!   page documents adding a label with STORE, not that STORE creates one
//!   (<https://developers.google.com/workspace/gmail/imap/imap-extensions>).
//! - Proton, through Bridge: `CREATE Labels/<name>` when missing, then `UID COPY` from INBOX into
//!   it, which adds the label and leaves the message in INBOX. neomail's earlier label is removed
//!   by marking and expunging exactly that message's UIDs in that label's mailbox, and only on a
//!   server that advertises UIDPLUS, whose `UID EXPUNGE` touches no other message (RFC 4315
//!   section 2.1). Bridge applies an expunge from a `Labels/` mailbox as removing the label, and
//!   an IMAP copy into one as adding it (docs/research/proton.md, from Bridge's IMAP connector).
//! - Spam: `UID MOVE` from INBOX to the mailbox LIST marks `\Junk`, as [`move_to`](super::move_to)
//!   moves.
//!
//! It never sends EXPUNGE, CLOSE, DELETE or RENAME, and never stores `\Deleted` outside a label
//! mailbox.

use std::collections::BTreeMap;

use async_imap::Session;
use async_imap::types::Fetch;
use nm_core::label::{self, LabelPrefix};
use nm_core::{Destination, Folder, Role, Uid, UidValidity};

use super::{
    Cmd, ImapConfig, ImapError, ImapSource, Login, Mailbox, Transport, collect, log_in, open, step,
    tls, uid_set, utf7,
};

/// Whose labels a server takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelKind {
    /// Gmail's, through `X-GM-LABELS`.
    Gmail,
    /// Proton's, as Bridge's `Labels/` mailboxes.
    Proton,
    /// None: other servers get no labels.
    None,
}

/// Why spam cannot move to Junk on a server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoJunk {
    /// The server lacks the MOVE capability (RFC 6851).
    NoMove,
    /// No mailbox carries the `\Junk` attribute (RFC 6154).
    NoFolder,
}

/// What a server takes from the label pass: [`probe`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Takes {
    pub labels: LabelKind,
    /// Why spam cannot move to Junk; `None` when it can.
    pub no_junk: Option<NoJunk>,
    /// UIDPLUS (RFC 4315), without which neomail's earlier Proton label stays.
    pub uidplus: bool,
}

/// One INBOX message's work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LabelEdit<'a> {
    pub uid: Uid,
    /// The label to write, with its prefix, when one is due.
    pub want: Option<&'a str>,
    /// What neomail wrote last, which it may remove.
    pub last: Option<&'a str>,
    /// The message's Message-ID without its angle brackets: how its copy in a Proton label's
    /// mailbox is found. `None` for a message without one, whose earlier Proton label stays.
    pub message_id: Option<&'a str>,
    /// Whether it moves to Junk, after its label when one is due.
    pub junk: bool,
}

/// What [`write_labels`] did, by the edits' indexes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LabelsWritten {
    /// The server, as this session found it.
    pub takes: Option<Takes>,
    /// The edits whose message now carries its label, and whether a command changed it.
    pub labelled: Vec<(usize, bool)>,
    /// Labelled, but neomail's earlier Proton label stays: no UIDPLUS, or no Message-ID.
    pub kept: Vec<usize>,
    /// Moved to Junk, as INBOX confirmed.
    pub moved: Vec<usize>,
    /// Asked to move and still in INBOX: the move was sent and not confirmed.
    pub unmoved: Vec<usize>,
    /// Not in INBOX any more: nothing was written for them.
    pub gone: Vec<usize>,
}

/// Whether `config` has Proton Mail Bridge's shape: a server on this machine whose certificate is
/// pinned. Labels are written as Bridge's only when its LIST agrees ([`ImapSource::takes`]).
fn bridge_shaped(config: &ImapConfig) -> bool {
    config.cert_pin.is_some()
        && matches!(
            config.host.as_str(),
            "127.0.0.1" | "localhost" | "::1" | "[::1]"
        )
}

/// Whether `config`'s server may take labels, so the label pass signs in to ask it ([`probe`]):
/// Gmail's IMAP host, `imap.gmail.com`
/// (<https://developers.google.com/workspace/gmail/imap/imap-smtp>), or Bridge's shape. Any
/// other server never takes them, so it is signed in to only for a move to Junk.
pub fn may_take_labels(config: &ImapConfig) -> bool {
    config.host.eq_ignore_ascii_case("imap.gmail.com") || bridge_shaped(config)
}

/// What `config`'s server takes from the label pass: Gmail's or Proton's labels, a move to Junk,
/// UIDPLUS. Read-only: CAPABILITY and LIST.
pub async fn probe(config: &ImapConfig, login: Login<'_>) -> Result<Takes, ImapError> {
    let tls = tls::client_config(config.cert_pin).map_err(ImapError::TlsConfig)?;
    let client = open(config, tls).await?;
    let session = log_in(client, &config.username, login).await?;
    probe_on(session, bridge_shaped(config)).await
}

/// [`probe`] on a logged-in session, which it logs out. Shared with the wire tests.
pub(crate) async fn probe_on<T: Transport>(
    session: Session<T>,
    bridge: bool,
) -> Result<Takes, ImapError> {
    let mut source = ImapSource { session, exists: 0 };
    let result = source.takes(bridge).await.map(|(takes, _)| takes);
    source.logout().await;
    result
}

/// Writes the labels of `edits`, INBOX messages of the mailbox with `uid_validity`, then moves
/// the ones marked `junk` to Junk: those whose label is written or was not due. When INBOX's
/// UIDVALIDITY is not `uid_validity`, nothing is written. See the module's rules.
pub async fn write_labels(
    config: &ImapConfig,
    login: Login<'_>,
    uid_validity: UidValidity,
    prefix: &LabelPrefix,
    edits: &[LabelEdit<'_>],
) -> Result<LabelsWritten, ImapError> {
    let tls = tls::client_config(config.cert_pin).map_err(ImapError::TlsConfig)?;
    let client = open(config, tls).await?;
    let session = log_in(client, &config.username, login).await?;
    write_labels_on(session, bridge_shaped(config), uid_validity, prefix, edits).await
}

/// [`write_labels`] on a logged-in session, which it logs out. Shared with the wire tests.
pub(crate) async fn write_labels_on<T: Transport>(
    session: Session<T>,
    bridge: bool,
    uid_validity: UidValidity,
    prefix: &LabelPrefix,
    edits: &[LabelEdit<'_>],
) -> Result<LabelsWritten, ImapError> {
    let mut source = ImapSource { session, exists: 0 };
    let result = source
        .write_labels(bridge, uid_validity, prefix, edits)
        .await;
    source.logout().await;
    result
}

/// `s` as an IMAP quoted string (RFC 3501 section 4.3); `None` when it is not printable US-ASCII,
/// which a quoted string cannot carry.
fn quoted(s: &str) -> Option<String> {
    s.chars()
        .all(|c| (' '..='~').contains(&c))
        .then(|| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")))
}

/// A label's name as Gmail takes it: modified UTF-7, which is printable US-ASCII, quoted.
fn label_name(name: &str) -> String {
    let encoded = utf7::encode(name);
    format!("\"{}\"", encoded.replace('\\', "\\\\").replace('"', "\\\""))
}

impl<T: Transport> ImapSource<T> {
    /// What the server takes, from CAPABILITY and LIST, and LIST's mailboxes. Gmail by its
    /// `X-GM-EXT-1` capability ("indicates support for the extensions", Google's page); Proton
    /// when `bridge` (the config's shape) and LIST shows Bridge's top-level `Labels` and
    /// `Folders`, as Bridge names them (docs/research/proton.md).
    pub(crate) async fn takes(&mut self, bridge: bool) -> Result<(Takes, Vec<Mailbox>), ImapError> {
        let op = "CAPABILITY";
        let capabilities = step(op, self.session.capabilities())
            .await?
            .map_err(|source| ImapError::Command { op, source })?;
        let mailboxes = self.mailboxes().await?;
        let top = |name: &str| mailboxes.iter().any(|m| m.wire == name);
        let labels = if capabilities.has_str("X-GM-EXT-1") {
            LabelKind::Gmail
        } else if bridge && top("Labels") && top("Folders") {
            LabelKind::Proton
        } else {
            LabelKind::None
        };
        let junk = mailboxes
            .iter()
            .any(|m| m.selectable && !m.inbox && m.roles.contains(&Role::Junk));
        let no_junk = if !capabilities.has_str("MOVE") {
            Some(NoJunk::NoMove)
        } else if !junk {
            Some(NoJunk::NoFolder)
        } else {
            None
        };
        let takes = Takes {
            labels,
            no_junk,
            uidplus: capabilities.has_str("UIDPLUS"),
        };
        Ok((takes, mailboxes))
    }

    /// See [`write_labels`].
    async fn write_labels(
        &mut self,
        bridge: bool,
        uid_validity: UidValidity,
        prefix: &LabelPrefix,
        edits: &[LabelEdit<'_>],
    ) -> Result<LabelsWritten, ImapError> {
        let (takes, mailboxes) = self.takes(bridge).await?;
        let mut out = LabelsWritten {
            takes: Some(takes),
            ..LabelsWritten::default()
        };
        self.select_inbox(uid_validity).await?;
        if edits.is_empty() {
            return Ok(out);
        }
        let uids: Vec<Uid> = edits.iter().map(|e| e.uid).collect();
        let present = self.present("UID FETCH", &uids).await?;
        let mut labelling = Vec::new();
        for (i, e) in edits.iter().enumerate() {
            if !present.contains(&e.uid) {
                out.gone.push(i);
            } else if e.want.is_some() {
                labelling.push(i);
            }
        }
        if !labelling.is_empty() {
            match takes.labels {
                LabelKind::Gmail => {
                    self.gmail_labels(prefix, edits, &labelling, &mailboxes, &mut out)
                        .await?;
                }
                LabelKind::Proton => {
                    self.proton_labels(takes, edits, &labelling, &mailboxes, &mut out)
                        .await?;
                }
                // Not asked for by the pass, which asks only what the server takes.
                LabelKind::None => {}
            }
        }
        let junk: Vec<usize> = edits
            .iter()
            .enumerate()
            .filter(|&(i, e)| {
                e.junk
                    && present.contains(&e.uid)
                    && (e.want.is_none() || out.labelled.iter().any(|&(l, _)| l == i))
            })
            .map(|(i, _)| i)
            .collect();
        if junk.is_empty() || takes.no_junk.is_some() {
            return Ok(out);
        }
        let uids: Vec<Uid> = junk.iter().map(|&i| edits[i].uid).collect();
        let moved = self
            .move_uids(uid_validity, &uids, &Destination::Role(Folder::Junk))
            .await?;
        for i in junk {
            if moved.moved.contains(&edits[i].uid) {
                out.moved.push(i);
            } else if moved.gone.contains(&edits[i].uid) {
                out.gone.push(i);
            } else {
                out.unmoved.push(i);
            }
        }
        Ok(out)
    }

    /// Gmail: each message's labels from `X-GM-LABELS`, then [`label::plan`]'s removals and
    /// additions, one name per STORE. Removals go first: a name neomail owns that differs from
    /// the wanted one only in case may be the same label to Gmail (unverified), and adding first
    /// would then let the removal take the wanted label off.
    async fn gmail_labels(
        &mut self,
        prefix: &LabelPrefix,
        edits: &[LabelEdit<'_>],
        which: &[usize],
        mailboxes: &[Mailbox],
        out: &mut LabelsWritten,
    ) -> Result<(), ImapError> {
        let uids: Vec<Uid> = which.iter().map(|&i| edits[i].uid).collect();
        let set = uid_set(&uids);
        let fetches = self
            .run_fetch("UID FETCH", Cmd::Uid(&set), "(UID X-GM-LABELS)")
            .await?;
        let mut add: BTreeMap<String, Vec<Uid>> = BTreeMap::new();
        let mut remove: BTreeMap<String, Vec<Uid>> = BTreeMap::new();
        for &i in which {
            let e = &edits[i];
            let Some(want) = e.want else { continue };
            let Some(fetch) = fetches
                .iter()
                .find(|f| f.uid.and_then(Uid::new) == Some(e.uid))
            else {
                out.gone.push(i);
                continue;
            };
            let current: Vec<String> = fetch
                .gmail_labels()
                .map(|names| names.iter().map(|n| utf7::display(n)).collect())
                .unwrap_or_default();
            let plan = label::plan(&current, prefix, e.last, want);
            out.labelled.push((i, !plan.is_empty()));
            for name in plan.remove {
                remove.entry(name).or_default().push(e.uid);
            }
            if let Some(name) = plan.add {
                add.entry(name).or_default().push(e.uid);
            }
        }
        for name in add.keys() {
            if !mailboxes.iter().any(|m| m.name.eq_ignore_ascii_case(name)) {
                self.create(&utf7::encode(name)).await?;
            }
        }
        for (name, uids) in &remove {
            self.store_label(uids, '-', name).await?;
        }
        for (name, uids) in &add {
            self.store_label(uids, '+', name).await?;
        }
        Ok(())
    }

    /// `UID STORE uids ±X-GM-LABELS (name)`: adds or removes the one name.
    async fn store_label(&mut self, uids: &[Uid], sign: char, name: &str) -> Result<(), ImapError> {
        let op = "UID STORE";
        let query = format!("{sign}X-GM-LABELS ({})", label_name(name));
        let stream = step(op, self.session.uid_store(uid_set(uids), &query))
            .await?
            .map_err(|source| ImapError::Command { op, source })?;
        let _: Vec<Fetch> = collect(op, stream).await?;
        Ok(())
    }

    /// Creates the mailbox `wire` (a label's). A `NO`, as for a name that exists in another case,
    /// is left for the write that follows to settle.
    async fn create(&mut self, wire: &str) -> Result<(), ImapError> {
        let op = "CREATE";
        match step(op, self.session.create(wire)).await? {
            Ok(()) | Err(async_imap::error::Error::No(_)) => Ok(()),
            Err(source) => Err(ImapError::Command { op, source }),
        }
    }

    /// Proton: `UID COPY` from INBOX into each wanted label's mailbox, created when missing; then
    /// neomail's earlier label is removed where UIDPLUS and a Message-ID allow it.
    async fn proton_labels(
        &mut self,
        takes: Takes,
        edits: &[LabelEdit<'_>],
        which: &[usize],
        mailboxes: &[Mailbox],
        out: &mut LabelsWritten,
    ) -> Result<(), ImapError> {
        let mut by_want: BTreeMap<&str, Vec<Uid>> = BTreeMap::new();
        for &i in which {
            if let Some(want) = edits[i].want {
                by_want.entry(want).or_default().push(edits[i].uid);
            }
        }
        for (want, uids) in &by_want {
            let path = format!("Labels/{want}");
            let wire = match mailboxes.iter().find(|m| m.name == path) {
                Some(m) => m.wire.clone(),
                None => {
                    let wire = format!("Labels/{}", utf7::encode(want));
                    self.create(&wire).await?;
                    wire
                }
            };
            let op = "UID COPY";
            step(op, self.session.uid_copy(uid_set(uids), &wire))
                .await?
                .map_err(|source| ImapError::Command { op, source })?;
        }
        let mut by_last: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for &i in which {
            let e = &edits[i];
            out.labelled.push((i, true));
            let Some(last) = e.last.filter(|&last| Some(last) != e.want) else {
                continue;
            };
            let path = format!("Labels/{last}");
            // No such label: no message carries it.
            let Some(mailbox) = mailboxes.iter().find(|m| m.name == path && m.selectable) else {
                continue;
            };
            if takes.uidplus && e.message_id.and_then(quoted).is_some() {
                by_last.entry(mailbox.wire.clone()).or_default().push(i);
            } else {
                out.kept.push(i);
            }
        }
        for (wire, group) in by_last {
            self.unlabel(&wire, edits, &group).await?;
        }
        Ok(())
    }

    /// Removes the label whose mailbox is `wire` (a `Labels/` one) from the messages of `group`:
    /// their copies there, found by Message-ID and checked against it, are marked `\Deleted` and
    /// expunged by UID, so no other message in the mailbox is touched.
    async fn unlabel(
        &mut self,
        wire: &str,
        edits: &[LabelEdit<'_>],
        group: &[usize],
    ) -> Result<(), ImapError> {
        debug_assert!(
            wire.starts_with("Labels/"),
            "only a label's mailbox: {wire}"
        );
        let op = "SELECT";
        step(op, self.session.select(wire))
            .await?
            .map_err(|source| ImapError::Command { op, source })?;
        let mut found = Vec::new();
        for &i in group {
            if let Some(id) = edits[i].message_id {
                found.extend(self.with_message_id(id).await?);
            }
        }
        if found.is_empty() {
            return Ok(());
        }
        found.sort_unstable();
        found.dedup();
        let set = uid_set(&found);
        let op = "UID STORE";
        let stream = step(
            op,
            self.session.uid_store(&set, "+FLAGS.SILENT (\\Deleted)"),
        )
        .await?
        .map_err(|source| ImapError::Command { op, source })?;
        let _: Vec<Fetch> = collect(op, stream).await?;
        let op = "UID EXPUNGE";
        let stream = step(op, self.session.uid_expunge(&set))
            .await?
            .map_err(|source| ImapError::Command { op, source })?;
        let _: Vec<u32> = collect(op, stream).await?;
        Ok(())
    }

    /// The UIDs in the selected mailbox of the messages whose Message-ID is exactly `id`: SEARCH
    /// matches a substring of the header (RFC 3501 section 6.4.4), so each hit's header is read
    /// back and compared.
    async fn with_message_id(&mut self, id: &str) -> Result<Vec<Uid>, ImapError> {
        let Some(query) = quoted(id) else {
            return Ok(Vec::new());
        };
        let op = "UID SEARCH";
        let hits = step(
            op,
            self.session
                .uid_search(format!("HEADER Message-ID {query}")),
        )
        .await?
        .map_err(|source| ImapError::Command { op, source })?;
        let mut hits: Vec<Uid> = hits.into_iter().filter_map(Uid::new).collect();
        if hits.is_empty() {
            return Ok(hits);
        }
        hits.sort_unstable();
        let set = uid_set(&hits);
        let fetches = self
            .run_fetch(
                "UID FETCH",
                Cmd::Uid(&set),
                "(UID BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)])",
            )
            .await?;
        Ok(fetches
            .iter()
            .filter_map(|f| {
                let uid = f.uid.and_then(Uid::new)?;
                let found = nm_core::parse::summarize(f.header()?).message_id?;
                (found.trim() == id).then_some(uid)
            })
            .collect())
    }
}

#[cfg(test)]
mod tests;
