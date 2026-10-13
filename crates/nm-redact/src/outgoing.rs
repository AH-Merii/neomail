//! What classification sends for one message: the [`Excerpt`] and a few [`Signals`] about the
//! sender and the headers, together an [`Outgoing`]. Only this module builds one, so a request
//! that takes an `Outgoing` can hold nothing but redacted text and these fields.
//!
//! The signals identify nobody: the sender's domain (lower-cased), whether the sender is a role
//! address such as `noreply@`, and whether the mail comes from a mailing list, is automated, or
//! is a reply. The address's local part and display names are never kept.

use crate::Excerpt;

/// From local parts that name a role, not a person, compared without case. A short exact list:
/// a local part that merely contains one of these (`info.smith`) is personal.
pub const ROLE_LOCAL_PARTS: [&str; 27] = [
    "noreply",
    "no-reply",
    "donotreply",
    "do-not-reply",
    "info",
    "news",
    "newsletter",
    "support",
    "help",
    "billing",
    "invoice",
    "invoices",
    "receipts",
    "orders",
    "shipping",
    "notifications",
    "notification",
    "alerts",
    "marketing",
    "hello",
    "team",
    "jobs",
    "careers",
    "recruiting",
    "account",
    "accounts",
    "security",
];

/// The longest domain kept (RFC 1035 section 2.3.4); a longer one is not a domain.
const MAX_DOMAIN: usize = 253;

/// What kind of address the mail comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SenderKind {
    /// The local part is on [`ROLE_LOCAL_PARTS`].
    Role,
    /// Any other address.
    Personal,
    /// The message has no From address, or one without an `@`.
    Unknown,
}

impl SenderKind {
    /// The kind's stable lower-case name, as sent and as used in JSON output.
    pub fn name(self) -> &'static str {
        match self {
            SenderKind::Role => "role",
            SenderKind::Personal => "personal",
            SenderKind::Unknown => "unknown",
        }
    }
}

/// Three facts the caller reads from a message's headers. Plain booleans, so anyone may build
/// one: they carry nothing personal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HeaderFlags {
    /// List-Unsubscribe or List-Id is present.
    pub mailing_list: bool,
    /// Auto-Submitted is present and not `no`, or Precedence is bulk, list or junk.
    pub automated: bool,
    /// In-Reply-To is present.
    pub reply: bool,
}

/// What is sent about a message besides its text. Only [`Outgoing::new`] makes one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signals {
    domain: Option<String>,
    kind: SenderKind,
    flags: HeaderFlags,
}

impl Signals {
    fn new(from_address: Option<&str>, flags: HeaderFlags) -> Self {
        let (domain, kind) = match from_address.and_then(|a| a.trim().rsplit_once('@')) {
            Some((local, domain)) if !local.is_empty() => {
                let kind = if ROLE_LOCAL_PARTS
                    .iter()
                    .any(|role| local.eq_ignore_ascii_case(role))
                {
                    SenderKind::Role
                } else {
                    SenderKind::Personal
                };
                (clean_domain(domain), kind)
            }
            _ => (None, SenderKind::Unknown),
        };
        Signals {
            domain,
            kind,
            flags,
        }
    }

    /// The From address's domain, lower-cased; `None` without a From address or when what
    /// follows the `@` is not a domain name (an address literal such as `[192.0.2.1]`, say).
    pub fn domain(&self) -> Option<&str> {
        self.domain.as_deref()
    }

    pub fn kind(&self) -> SenderKind {
        self.kind
    }

    pub fn mailing_list(&self) -> bool {
        self.flags.mailing_list
    }

    pub fn automated(&self) -> bool {
        self.flags.automated
    }

    pub fn reply(&self) -> bool {
        self.flags.reply
    }
}

/// `domain` lower-cased, if it is a domain name: dot-separated labels of letters, digits and
/// hyphens (any script, for internationalised names), at least two of them, the last not all
/// digits. Anything else could be text of the sender's choosing, so it is not sent.
fn clean_domain(domain: &str) -> Option<String> {
    let domain = domain.trim().trim_end_matches('.').to_lowercase();
    let labels: Vec<&str> = domain.split('.').collect();
    let valid = domain.len() <= MAX_DOMAIN
        && labels.len() >= 2
        && labels.iter().all(|l| {
            !l.is_empty()
                && l.chars().all(|c| c.is_alphanumeric() || c == '-')
                && !l.starts_with('-')
                && !l.ends_with('-')
        })
        && labels
            .last()
            .is_some_and(|tld| !tld.chars().all(|c| c.is_ascii_digit()));
    valid.then_some(domain)
}

/// Everything classification may send for one message: its [`Excerpt`] and its [`Signals`].
/// Only [`Outgoing::new`] makes one, and it keeps the sender's domain alone of the address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outgoing {
    excerpt: Excerpt,
    signals: Signals,
}

impl Outgoing {
    /// `from_address` is the bare From address (`orders@acme.example`), without a display name.
    pub fn new(excerpt: Excerpt, from_address: Option<&str>, flags: HeaderFlags) -> Self {
        Outgoing {
            excerpt,
            signals: Signals::new(from_address, flags),
        }
    }

    pub fn excerpt(&self) -> &Excerpt {
        &self.excerpt
    }

    pub fn signals(&self) -> &Signals {
        &self.signals
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Known;

    fn signals(from: Option<&str>) -> Signals {
        Outgoing::new(
            Excerpt::new("s", "b", &Known::default()),
            from,
            HeaderFlags::default(),
        )
        .signals
    }

    #[test]
    fn a_role_local_part_is_matched_whole_and_without_case() {
        for from in [
            "noreply@acme.example",
            "No-Reply@acme.example",
            "ORDERS@shop.example",
            "security@bank.example",
            "do-not-reply@acme.example",
        ] {
            assert_eq!(signals(Some(from)).kind(), SenderKind::Role, "{from}");
        }
        for from in [
            "ada@acme.example",
            "info.smith@acme.example",
            "noreply2@acme.example",
            "my-orders@shop.example",
        ] {
            assert_eq!(signals(Some(from)).kind(), SenderKind::Personal, "{from}");
        }
    }

    #[test]
    fn no_from_address_is_unknown_with_no_domain() {
        for from in [
            None,
            Some(""),
            Some("not an address"),
            Some("@acme.example"),
        ] {
            let s = signals(from);
            assert_eq!(
                (s.kind(), s.domain()),
                (SenderKind::Unknown, None),
                "{from:?}"
            );
        }
    }

    #[test]
    fn the_domain_is_kept_lower_cased_and_only_when_it_is_a_domain_name() {
        assert_eq!(
            signals(Some("Ada@Mail.ACME.example")).domain(),
            Some("mail.acme.example")
        );
        assert_eq!(
            signals(Some("ada@acme.example.")).domain(),
            Some("acme.example")
        );
        assert_eq!(
            signals(Some("ada@bücher.example")).domain(),
            Some("bücher.example")
        );
        for from in [
            "ada@[192.0.2.1]",
            "ada@192.0.2.1",
            "ada@localhost",
            "ada@acme..example",
            "ada@-acme.example",
            "ada@acme example.org",
            "ada@acme.example>, Bob Smith",
        ] {
            let s = signals(Some(from));
            assert_eq!(s.domain(), None, "{from}");
            assert_eq!(s.kind(), SenderKind::Personal, "{from}");
        }
        let long = format!("ada@{}.example", "a".repeat(MAX_DOMAIN));
        assert_eq!(signals(Some(&long)).domain(), None);
    }

    #[test]
    fn the_local_part_is_never_kept() {
        let s = signals(Some("ada.lovelace@acme.example"));
        assert!(!format!("{s:?}").contains("lovelace"), "{s:?}");
    }

    #[test]
    fn the_header_flags_pass_through() {
        let flags = HeaderFlags {
            mailing_list: true,
            automated: false,
            reply: true,
        };
        let out = Outgoing::new(Excerpt::new("s", "b", &Known::default()), None, flags);
        let s = out.signals();
        assert_eq!(
            (s.mailing_list(), s.automated(), s.reply()),
            (true, false, true)
        );
        assert_eq!(out.excerpt().subject().as_str(), "s");
    }
}
