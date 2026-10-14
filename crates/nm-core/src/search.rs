//! Searching the cache: the query a user types ([`Query`]), where it looks ([`Scope`]), and the
//! text the full-text index holds for each message. [`Cache::search`](crate::Cache::search) runs
//! a query; this module is the one owner of how text is folded and how a query reaches SQLite's
//! FTS5, so user text never reaches FTS5's query syntax unquoted.

use crate::cache::GroupFilter;
use crate::parse::{self, EmailAddress};
use crate::types::MessageId;

/// What the index holds for a message changes with this: a row indexed under another version is
/// indexed again ([`Cache::index_pending`](crate::Cache::index_pending)). Bump it when the
/// folding, the fields or the body cap change.
pub const TEXT_VERSION: i64 = 1;

/// The most body text indexed per message, in bytes: a huge newsletter does not bloat the index.
const BODY_CAP: usize = 64 * 1024;

/// Which of a message's texts a term must match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    /// The subject, the sender, the recipients or the body.
    Any,
    Subject,
    /// The sender's name and address.
    From,
    /// The To and Cc names and addresses.
    To,
}

impl Field {
    const NAMED: [(&'static str, Field); 3] = [
        ("subject", Field::Subject),
        ("from", Field::From),
        ("to", Field::To),
    ];

    /// The index column, or `None` for every column.
    fn column(self) -> Option<&'static str> {
        match self {
            Field::Any => None,
            Field::Subject => Some("subject"),
            Field::From => Some("sender"),
            Field::To => Some("recipients"),
        }
    }
}

/// One thing a message must contain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Term {
    pub field: Field,
    /// As typed, unfolded.
    pub text: String,
    /// `true` for `"quoted words"`, which match whole words in that order; otherwise each word
    /// matches as a prefix (`invo` finds `invoice`).
    pub phrase: bool,
}

/// A parsed search: every term must match. Case, Latin accents and the Arabic marks [`fold`]
/// drops are ignored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    pub terms: Vec<Term>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QueryError {
    #[error("{0}: needs a word after it, such as {0}:invoice")]
    EmptyField(&'static str),
}

/// Where a search looks: every account or one, a group, and a thread. They narrow together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    pub account: Option<String>,
    pub group: GroupFilter,
    /// The thread of this message, in its own account.
    pub thread: Option<MessageId>,
}

impl Scope {
    /// Every cached message.
    pub fn everything() -> Self {
        Scope {
            account: None,
            group: GroupFilter::Any,
            thread: None,
        }
    }
}

impl Query {
    /// Parses what a user typed. Words are separated by whitespace; `"quoted words"` are a
    /// phrase (an unclosed quote runs to the end); `subject:`, `from:` and `to:` (any case) put
    /// the word or phrase after them in that field. Any other `name:` is ordinary text. A word
    /// with no letter or digit, such as `-`, matches nothing and is left out.
    pub fn parse(text: &str) -> Result<Self, QueryError> {
        let mut terms = Vec::new();
        let mut rest = text.trim_start();
        while !rest.is_empty() {
            let mut field = Field::Any;
            for (name, f) in Field::NAMED {
                let named = rest
                    .get(..name.len())
                    .is_some_and(|h| h.eq_ignore_ascii_case(name));
                if named && rest[name.len()..].starts_with(':') {
                    field = f;
                    rest = &rest[name.len() + 1..];
                    if rest.is_empty() || rest.starts_with(char::is_whitespace) {
                        return Err(QueryError::EmptyField(name));
                    }
                    break;
                }
            }
            let (value, phrase, after) = match rest.strip_prefix('"') {
                Some(quoted) => match quoted.split_once('"') {
                    Some((inside, after)) => (inside, true, after),
                    None => (quoted, true, ""),
                },
                None => {
                    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
                    (&rest[..end], false, &rest[end..])
                }
            };
            if fold(value).chars().any(char::is_alphanumeric) {
                terms.push(Term {
                    field,
                    text: value.to_owned(),
                    phrase,
                });
            }
            rest = after.trim_start();
        }
        Ok(Query { terms })
    }

    /// Whether the query matches every message: it has no term.
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// The FTS5 MATCH expression, or `None` for an empty query. Every term is one quoted FTS5
    /// string, so `NEAR`, `*`, `-` and `"` in it are text.
    /// <https://www.sqlite.org/fts5.html#full_text_query_syntax>
    pub(crate) fn fts(&self) -> Option<String> {
        let parts: Vec<String> = self
            .terms
            .iter()
            .map(|t| {
                let quoted = format!("\"{}\"", fold(&t.text).replace('"', "\"\""));
                let star = if t.phrase { "" } else { "*" };
                match t.field.column() {
                    Some(column) => format!("{column} : {quoted}{star}"),
                    None => format!("{quoted}{star}"),
                }
            })
            .collect();
        (!parts.is_empty()).then(|| parts.join(" AND "))
    }
}

/// Text as the index stores it and a query looks for it. The index's tokenizer already ignores
/// case and Latin accents; this drops what it does not: the Arabic short vowels and other marks
/// (U+064B to U+065F, U+0670), which would split a word, and the tatweel (U+0640); and writes the
/// alef with a hamza or wasla (`أ`, `إ`, `ٱ`) as `ا` and the Persian keheh and yeh (`ک`, `ی`) as
/// `ك` and `ي`. The same set as redaction's name folding: `ي` and `ى`, `ة` and `ه`, and `آ` and
/// `ا` stay apart, so `علي` does not find `على`.
pub fn fold(text: &str) -> String {
    text.chars()
        .filter(|&c| !matches!(c, '\u{64b}'..='\u{65f}' | '\u{670}' | '\u{640}'))
        .map(|c| match c {
            'أ' | 'إ' | 'ٱ' => 'ا',
            'ک' => 'ك',
            'ی' => 'ي',
            c => c,
        })
        .collect()
}

/// What the index holds for one message, folded, and the thread it belongs to.
pub(crate) struct IndexText {
    pub subject: String,
    pub sender: String,
    pub recipients: String,
    pub body: String,
    /// The thread's key, as [`thread_key`] picks it.
    pub thread: String,
}

impl IndexText {
    pub(crate) fn of(raw: &[u8]) -> Self {
        let Ok(parsed) = parse::parse(raw) else {
            return IndexText {
                subject: String::new(),
                sender: String::new(),
                recipients: String::new(),
                body: String::new(),
                thread: parse::msg_key(None, raw),
            };
        };
        let thread = thread_key(&parsed.threading, raw);
        let mut body = parsed.body;
        if body.len() > BODY_CAP {
            let mut end = BODY_CAP;
            while !body.is_char_boundary(end) {
                end -= 1;
            }
            body.truncate(end);
        }
        IndexText {
            subject: fold(parsed.subject.as_deref().unwrap_or("")),
            sender: fold(&addresses(&parsed.from)),
            recipients: fold(&addresses(parsed.to.iter().chain(&parsed.cc))),
            body: fold(&body),
            thread,
        }
    }
}

/// The thread's key: the first References id (the thread's root, RFC 5322 section 3.6.4), else
/// the first In-Reply-To id, else the message's own key ([`parse::msg_key`]). Proton Mail Bridge
/// appends its own `<ID@protonmail.internalid>` to every message's References, a thread's first
/// message too (docs/research/proton.md); no other message names it, so it is skipped, or a
/// thread's root would never share its replies' key.
fn thread_key(threading: &parse::Threading, raw: &[u8]) -> String {
    let real = |id: &&String| !id.to_ascii_lowercase().ends_with("@protonmail.internalid");
    threading
        .references
        .iter()
        .find(real)
        .or_else(|| threading.in_reply_to.iter().find(real))
        .cloned()
        .unwrap_or_else(|| parse::msg_key(threading.message_id.as_deref(), raw))
}

fn addresses<'a>(list: impl IntoIterator<Item = &'a EmailAddress>) -> String {
    list.into_iter()
        .flat_map(|a| [a.name.as_deref(), a.address.as_deref()])
        .flatten()
        .collect::<Vec<_>>()
        .join(" ")
}
