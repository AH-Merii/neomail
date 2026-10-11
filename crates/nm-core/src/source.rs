//! The seam between the sync engine and a mail provider.

use std::future::Future;

use crate::types::{FetchedMessage, ListedMessage, MailboxStatus, Uid, UidFlags};

/// A read-only view of one account's INBOX. `nm-imap` implements it over IMAP; the tests use a
/// fake. Later providers (Microsoft Graph, the Gmail API) implement the same trait.
///
/// Contract:
/// - [`examine_inbox`](Self::examine_inbox) is called first and opens the inbox read-only.
/// - Nothing a source does may change the server: no flags (fetches must not set `\Seen`), no
///   moves, no deletes.
/// - Results may come in any order and may contain UIDs outside the requested range; the sync
///   engine filters and sorts.
///
/// neomail drives a source from one task, so the futures carry no `Send` bound yet.
pub trait MailSource {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Opens INBOX read-only and reports its UIDVALIDITY and message count.
    fn examine_inbox(&mut self) -> impl Future<Output = Result<MailboxStatus, Self::Error>>;

    /// The UID, flags and arrival time of every message, for the first sync to choose the newest
    /// from. UID order is not arrival order on every server (Proton Bridge numbers imported mail
    /// newest first), so the sync engine picks by arrival time.
    fn list_all(&mut self) -> impl Future<Output = Result<Vec<ListedMessage>, Self::Error>>;

    /// The UIDs and flags of every message whose UID is `from` or higher.
    fn flags_from(&mut self, from: Uid)
    -> impl Future<Output = Result<Vec<UidFlags>, Self::Error>>;

    /// The full messages for `uids`, without marking them read. UIDs that no longer exist are
    /// left out of the result.
    fn fetch(
        &mut self,
        uids: &[Uid],
    ) -> impl Future<Output = Result<Vec<FetchedMessage>, Self::Error>>;
}
