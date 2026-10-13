//! neomail's PII redaction: the rules that mask personal data, how each mask is spelled, and the
//! [`Excerpt`], the only text that may leave the machine for classification.
//!
//! Pure: no I/O, no network, no configuration. Callers pass the text and what they already know
//! ([`Known`]: display names from the headers, the owner's addresses); this crate decides what is
//! masked and how. Every pattern is compiled once per process.
//!
//! The rules, with the placeholder each writes (`docs/privacy.md` says why each exists):
//!
//! | Kind | Masks | Placeholder |
//! |---|---|---|
//! | [`Kind::Email`] | an address's local part; the owner's own addresses whole | `<email>@acme.example`, `<email>` |
//! | [`Kind::Link`] | a URL, all but its host | `<link host=acme.example>` |
//! | [`Kind::Iban`] | an IBAN passing the ISO 13616 check digits | `<iban>` |
//! | [`Kind::Card`] | 13 to 19 digits passing the Luhn check | `<card>` |
//! | [`Kind::Amount`] | a number with a currency symbol or code | `<amount>` |
//! | [`Kind::Code`] | a one-time code after "code", "OTP" …, or a password after "password", "PIN" … | `<code>` |
//! | [`Kind::Phone`] | a phone number (dates and times excluded) | `<phone>` |
//! | [`Kind::Number`] | any other run of 4 or more digits | `<number>` |
//! | [`Kind::Name`] | a known name, as a whole word | `<name>` |
//!
//! Kept on purpose: domains and link hosts, dates and times, numbers of 1 to 3 digits, and names
//! that appear in no header. Overlapping matches merge into one mask, named by the kind listed
//! first; nothing masks a link host, an address's domain or a date, except a one-time code right
//! after its keyword (`sort code 12-03-45`). A bare domain (one ending in a real top-level domain)
//! is kept from the name rule only, so digits written straight before a full stop (`4111….co`) are
//! still masked.

use std::collections::HashSet;
use std::fmt;
use std::ops::Range;

mod rules;

/// How many characters (Unicode scalar values) of the redacted body an [`Excerpt`] keeps: the
/// excerpt size the PII benchmark measured (DEC-2026-10-11-pii-rules in `docs/decisions.md`).
pub const EXCERPT_CHARS: usize = 1_500;

/// What a mask stands for. The order is the priority when matches overlap: the merged mask takes
/// the earliest kind. Checksum-backed and keyword-backed rules come before the looser phone and
/// digit-run rules, so a valid IBAN or card is never reported as a phone or a number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Email,
    Link,
    Iban,
    Card,
    Amount,
    Code,
    Phone,
    Number,
    Name,
}

impl Kind {
    /// Every kind, in priority order.
    pub const ALL: [Kind; 9] = [
        Kind::Email,
        Kind::Link,
        Kind::Iban,
        Kind::Card,
        Kind::Amount,
        Kind::Code,
        Kind::Phone,
        Kind::Number,
        Kind::Name,
    ];

    /// The kind's stable lower-case name, as used in JSON output.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Email => "email",
            Kind::Link => "link",
            Kind::Iban => "iban",
            Kind::Card => "card",
            Kind::Amount => "amount",
            Kind::Code => "code",
            Kind::Phone => "phone",
            Kind::Number => "number",
            Kind::Name => "name",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// How many masks of each kind a text holds. Every kind has a count, zeros included.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MaskCounts([usize; Kind::ALL.len()]);

impl MaskCounts {
    pub fn get(&self, kind: Kind) -> usize {
        self.0[kind.index()]
    }

    /// Every kind with its count, in [`Kind::ALL`] order.
    pub fn iter(&self) -> impl Iterator<Item = (Kind, usize)> + '_ {
        Kind::ALL.iter().map(|&k| (k, self.get(k)))
    }

    pub fn total(&self) -> usize {
        self.0.iter().sum()
    }

    fn add(&mut self, other: &MaskCounts) {
        for (a, b) in self.0.iter_mut().zip(other.0) {
            *a += b;
        }
    }
}

/// What the caller already knows and wants masked: display names from a message's headers, and
/// the owner's own addresses.
#[derive(Clone, Debug, Default)]
pub struct Known {
    /// Capitalised words of the display names, plus their all-caps forms; matched case-sensitively.
    names: HashSet<String>,
    /// The owner's addresses, lower-cased; matched without case.
    owner_addresses: HashSet<String>,
}

impl Known {
    /// `display_names`: the display names from From, To, Cc and Reply-To (the owner's own name
    /// among them whenever a correspondent writes it). Each word of a name that starts with a
    /// capital letter and has at least two characters is masked as a whole word, as written and
    /// in capitals; lower-case particles such as "de" or "van" are skipped. A word written all in
    /// capitals with three or more letters (`SMITH John`) is masked in title case too (`Smith`).
    /// Matching is case-sensitive, so a person called Will or May never masks "will" or "may";
    /// the cost of the title case is that a sender written `MAY` masks a sentence's first "May".
    /// Apostrophes match in any form: a header's `O'Brien` masks a body's `O’Brien`, and back.
    ///
    /// `owner_addresses`: every account's address; matched case-insensitively and masked whole.
    pub fn new<N, A>(
        display_names: impl IntoIterator<Item = N>,
        owner_addresses: impl IntoIterator<Item = A>,
    ) -> Self
    where
        N: AsRef<str>,
        A: AsRef<str>,
    {
        let mut names = HashSet::new();
        for name in display_names {
            for word in rules::words(name.as_ref()) {
                let folded = rules::fold_apostrophes(word);
                let word: &str = &folded;
                if word.contains('@') || word.chars().count() < 2 {
                    continue;
                }
                let mut chars = word.chars();
                let Some(first) = chars.next().filter(|c| c.is_uppercase()) else {
                    continue;
                };
                names.insert(word.to_uppercase());
                names.insert(word.to_string());
                let letters = word.chars().filter(|c| c.is_alphabetic()).count();
                if letters >= 3 && !word.chars().any(char::is_lowercase) {
                    names.insert(first.to_string() + &chars.as_str().to_lowercase());
                }
            }
        }
        let owner_addresses = owner_addresses
            .into_iter()
            .map(|a| a.as_ref().trim().to_lowercase())
            .filter(|a| a.contains('@'))
            .collect();
        Known {
            names,
            owner_addresses,
        }
    }

    fn is_name(&self, word: &str) -> bool {
        self.names.contains(word)
    }

    fn is_owner_address(&self, address: &str) -> bool {
        !self.owner_addresses.is_empty() && self.owner_addresses.contains(&address.to_lowercase())
    }
}

/// One mask in a [`Redacted`] text: where its placeholder sits, and its kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mask {
    /// Byte range of the placeholder in [`Redacted::as_str`].
    pub range: Range<usize>,
    pub kind: Kind,
}

/// A part of a [`Redacted`] text: kept text, or a placeholder with its kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Piece<'a> {
    Text(&'a str),
    Mask(Kind, &'a str),
}

/// A text with every rule applied. Only this crate makes one, so a `Redacted` always holds masked
/// text, and it knows where each placeholder is (for styling, and so a cut never splits one).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Redacted {
    text: String,
    masks: Vec<Mask>,
}

impl Redacted {
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// The placeholders, in text order; their ranges never overlap.
    pub fn masks(&self) -> &[Mask] {
        &self.masks
    }

    /// The text split into kept text and placeholders, in order.
    pub fn pieces(&self) -> impl Iterator<Item = Piece<'_>> + '_ {
        let mut out = Vec::with_capacity(self.masks.len() * 2 + 1);
        let mut last = 0;
        for m in &self.masks {
            if m.range.start > last {
                out.push(Piece::Text(&self.text[last..m.range.start]));
            }
            out.push(Piece::Mask(m.kind, &self.text[m.range.clone()]));
            last = m.range.end;
        }
        if last < self.text.len() {
            out.push(Piece::Text(&self.text[last..]));
        }
        out.into_iter()
    }

    pub fn counts(&self) -> MaskCounts {
        let mut c = MaskCounts::default();
        for m in &self.masks {
            c.0[m.kind.index()] += 1;
        }
        c
    }

    /// Keeps at most `max_chars` characters, never cutting inside a placeholder (a placeholder
    /// that would cross the edge is dropped whole). Returns whether anything was cut.
    fn cut(&mut self, max_chars: usize) -> bool {
        let Some((edge, _)) = self.text.char_indices().nth(max_chars) else {
            return false;
        };
        let edge = self
            .masks
            .iter()
            .find(|m| m.range.start < edge && edge < m.range.end)
            .map_or(edge, |m| m.range.start);
        self.text.truncate(edge);
        self.masks.retain(|m| m.range.end <= edge);
        true
    }
}

impl fmt::Display for Redacted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

/// Applies every rule to `text`, masking what `known` names as well.
pub fn redact(text: &str, known: &Known) -> Redacted {
    rules::redact(text, known)
}

/// The text classification will send for one message: its subject, redacted whole, and the
/// start of its body, redacted whole and then cut to [`EXCERPT_CHARS`] characters. Redacting
/// before cutting means a value cut in half at the edge cannot slip past its rule.
///
/// Only [`Excerpt::new`] makes one, so whatever takes an `Excerpt` gets redacted text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Excerpt {
    subject: Redacted,
    body: Redacted,
    truncated: bool,
}

impl Excerpt {
    /// `body` is the message body as `neomail show` renders it.
    pub fn new(subject: &str, body: &str, known: &Known) -> Self {
        let subject = redact(subject, known);
        let mut body = redact(body, known);
        let truncated = body.cut(EXCERPT_CHARS);
        Excerpt {
            subject,
            body,
            truncated,
        }
    }

    pub fn subject(&self) -> &Redacted {
        &self.subject
    }

    pub fn body(&self) -> &Redacted {
        &self.body
    }

    /// Whether the body was cut to [`EXCERPT_CHARS`] characters.
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    /// Masks in the subject and the kept body together.
    pub fn masks(&self) -> MaskCounts {
        let mut c = self.subject.counts();
        c.add(&self.body.counts());
        c
    }
}
