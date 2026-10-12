//! neomail's core: domain types, the [`MailSource`] and [`IdSource`] seams that providers
//! implement, the SQLite [`cache`] and the read-only sync engines ([`sync`] for UIDs,
//! [`id_source`] for opaque ids). Nothing here touches the network.

pub mod cache;
#[cfg(feature = "conformance")]
pub mod conformance;
pub mod id_source;
pub mod parse;
pub mod source;
pub mod sync;
pub mod types;

pub use cache::{Cache, CacheError};
pub use id_source::{IdSource, sync_inbox_by_id, sync_inbox_by_id_with_progress};
pub use source::MailSource;
pub use sync::{Progress, SyncError, SyncReport, sync_inbox, sync_inbox_with_progress};
pub use types::{
    FetchedMessage, Flags, IdFetched, IdListed, ListedMessage, MailboxStatus, MessageId, RemoteId,
    Uid, UidFlags, UidValidity,
};
