//! Domain types shared by the cache, the sync engine and every [`MailSource`](crate::MailSource).

use std::collections::BTreeSet;
use std::fmt;
use std::num::NonZeroU32;

use chrono::{DateTime, Utc};
use serde::Serialize;

/// An IMAP UID: unique and ascending within one mailbox for one [`UidValidity`]. Never zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct Uid(NonZeroU32);

impl Uid {
    /// Returns `None` for zero, which IMAP never uses as a UID.
    pub fn new(value: u32) -> Option<Self> {
        NonZeroU32::new(value).map(Self)
    }

    pub fn get(self) -> u32 {
        self.0.get()
    }
}

impl fmt::Display for Uid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A mailbox's UIDVALIDITY. When it changes, every cached UID of that mailbox is meaningless.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct UidValidity(NonZeroU32);

impl UidValidity {
    /// Returns `None` for zero, which RFC 3501 does not allow.
    pub fn new(value: u32) -> Option<Self> {
        NonZeroU32::new(value).map(Self)
    }

    pub fn get(self) -> u32 {
        self.0.get()
    }
}

impl fmt::Display for UidValidity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// The cache's own id for a message: what `neomail list` prints and `neomail show` takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct MessageId(i64);

impl MessageId {
    pub fn new(value: i64) -> Self {
        Self(value)
    }

    pub fn get(self) -> i64 {
        self.0
    }
}

impl fmt::Display for MessageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// The IMAP flags of a message, kept sorted so two sets compare equal regardless of server order.
/// System flags keep their backslash (`\Seen`); `\Recent` is never stored, because it is
/// per-session and would look like a change on every sync.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Flags(BTreeSet<String>);

impl Flags {
    pub const SEEN: &'static str = "\\Seen";

    pub fn new<I, S>(flags: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self(
            flags
                .into_iter()
                .map(Into::into)
                .filter(|f| !f.is_empty() && !f.eq_ignore_ascii_case("\\Recent"))
                .collect(),
        )
    }

    pub fn is_seen(&self) -> bool {
        self.0.contains(Self::SEEN)
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }

    /// Space-separated, as stored in the cache. IMAP flags never contain spaces.
    pub fn to_storage(&self) -> String {
        self.iter().collect::<Vec<_>>().join(" ")
    }

    pub fn from_storage(s: &str) -> Self {
        Self::new(s.split_ascii_whitespace())
    }
}

/// What the server reports when the inbox is opened read-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MailboxStatus {
    pub uid_validity: UidValidity,
    /// The number of messages in the mailbox.
    pub exists: u32,
}

/// A message's UID and current flags, without its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UidFlags {
    pub uid: Uid,
    pub flags: Flags,
}

/// A message's UID, flags and arrival time, without its content: what a first sync chooses from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedMessage {
    pub uid: Uid,
    pub flags: Flags,
    /// When the server received the message (IMAP INTERNALDATE), if it said.
    pub received: Option<DateTime<Utc>>,
}

/// A whole message as fetched from the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedMessage {
    pub uid: Uid,
    pub flags: Flags,
    /// When the server received the message (IMAP INTERNALDATE), if it said.
    pub received: Option<DateTime<Utc>>,
    /// The RFC 822 source.
    pub raw: Vec<u8>,
}
