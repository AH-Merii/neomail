//! neomail's core: domain types, the [`MailSource`] seam that providers implement, the SQLite
//! [`cache`] and the read-only [`sync`] engine. Nothing here touches the network.

pub mod cache;
pub mod parse;
pub mod source;
pub mod sync;
pub mod types;

pub use cache::{Cache, CacheError};
pub use source::MailSource;
pub use sync::{Progress, SyncError, SyncReport, sync_inbox, sync_inbox_with_progress};
pub use types::{
    FetchedMessage, Flags, ListedMessage, MailboxStatus, MessageId, Uid, UidFlags, UidValidity,
};
