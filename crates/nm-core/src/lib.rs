//! neomail's core: domain types, the [`MailSource`] and [`IdSource`] seams that providers
//! implement, the SQLite [`cache`], the read-only sync engines ([`sync`] for UIDs,
//! [`id_source`] for opaque ids) and the [`action`]s that change mail through the
//! [`MailWriter`] seam. Nothing here touches the network.

pub mod action;
pub mod cache;
#[cfg(feature = "conformance")]
pub mod conformance;
pub mod folder;
pub mod group;
pub mod id_source;
pub mod parse;
pub mod source;
pub mod sync;
pub mod types;

pub use action::{Action, Change, Destination, Folder, MailWriter, Moved, Written};
pub use cache::{
    Cache, CacheError, Classification, GroupFilter, Grouped, Located, MessageKey, Pending,
};
pub use folder::{FolderName, ListedFolder, Role};
pub use group::{Choice, Group, GroupRules};
pub use id_source::{IdSource, sync_inbox_by_id, sync_inbox_by_id_with_progress};
pub use source::MailSource;
pub use sync::{Progress, SyncError, SyncReport, sync_inbox, sync_inbox_with_progress};
pub use types::{
    Cursor, FetchedMessage, Flags, IdChange, IdChanges, IdFetched, IdListed, ListedMessage,
    MailboxStatus, MessageId, RemoteId, Uid, UidFlags, UidValidity,
};
