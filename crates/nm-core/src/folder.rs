//! Folders as the user sees them: a [`FolderName`], which `neomail folder list` prints and
//! `neomail move` takes, and a [`ListedFolder`]'s [`Role`]. Only a provider turns a name into
//! one of its folders, and back: an IMAP mailbox name stays encoded inside nm-imap, and a Graph
//! folder id inside nm-graph.

use std::fmt;

/// A folder's name as the user sees and types it: an IMAP mailbox name decoded from modified
/// UTF-7 with the server's hierarchy delimiter as it is (`Folders/Receipts`, `[Gmail]/Trash`),
/// or an Outlook folder's path of display names (`Inbox/Receipts`). Never empty.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FolderName(String);

impl FolderName {
    /// `None` for an empty name.
    pub fn new(name: impl Into<String>) -> Option<Self> {
        let name = name.into();
        (!name.is_empty()).then_some(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this is the name `INBOX`, which IMAP matches in any case (RFC 3501 section 5.1).
    pub fn is_inbox(&self) -> bool {
        self.0.eq_ignore_ascii_case("INBOX")
    }
}

impl fmt::Display for FolderName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a folder is for, from IMAP's special-use attributes (RFC 6154 section 2) or Graph's
/// well-known folder names. A folder has at most one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    Inbox,
    Archive,
    Trash,
    Junk,
    Sent,
    Drafts,
    /// Every message in the mailbox (IMAP `\All`: Gmail's All Mail).
    All,
    /// The flagged or starred messages (IMAP `\Flagged`).
    Flagged,
}

impl Role {
    pub const ALL: [Self; 8] = [
        Self::Inbox,
        Self::Archive,
        Self::Trash,
        Self::Junk,
        Self::Sent,
        Self::Drafts,
        Self::All,
        Self::Flagged,
    ];

    /// The role's stable name, as `folder list` prints it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Inbox => "inbox",
            Self::Archive => "archive",
            Self::Trash => "trash",
            Self::Junk => "junk",
            Self::Sent => "sent",
            Self::Drafts => "drafts",
            Self::All => "all",
            Self::Flagged => "flagged",
        }
    }
}

/// A folder a message can be in: its name as `move` takes it, and its role, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedFolder {
    pub name: FolderName,
    pub role: Option<Role>,
}

#[cfg(test)]
mod tests {
    use super::Role;

    /// docs/json.md documents `role` as one of these eight words, in this order; every role
    /// prints as its own word.
    #[test]
    fn every_role_prints_the_word_the_docs_give_it() {
        assert_eq!(
            Role::ALL.map(Role::name),
            [
                "inbox", "archive", "trash", "junk", "sent", "drafts", "all", "flagged"
            ]
        );
    }
}
