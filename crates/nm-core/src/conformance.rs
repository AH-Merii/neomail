//! Contract checks for [`IdSource`] implementations, behind the `conformance` feature, so every
//! source (the tests' reference source, Microsoft Graph) runs the same ones. They only read, as
//! every source must. They need an inbox with at least two messages of different arrival times
//! that does not change while they run; with fewer they fail with
//! [`ConformanceError::Precondition`] instead of passing without checking anything.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;

use crate::id_source::IdSource;
use crate::parse;
use crate::types::{IdListed, RemoteId};

/// How many of the newest messages the checks look at.
const SAMPLE: NonZeroU32 = NonZeroU32::new(10).unwrap();
/// How many of them are fetched.
const FETCHED: usize = 3;

#[derive(Debug, thiserror::Error)]
pub enum ConformanceError<E: std::error::Error + 'static> {
    #[error("the source failed")]
    Source(#[source] E),
    #[error("the source breaks the IdSource contract: {0}")]
    Contract(String),
    #[error("the checks cannot run: {0}")]
    Precondition(String),
}

fn ensure<E: std::error::Error>(
    ok: bool,
    what: impl FnOnce() -> String,
) -> Result<(), ConformanceError<E>> {
    if ok {
        Ok(())
    } else {
        Err(ConformanceError::Contract(what()))
    }
}

/// Runs every check against `source`:
/// - `newest(SAMPLE)` lists at least two messages of different arrival times (the precondition);
/// - `newest(n)` returns at most `n` messages, each id once;
/// - `newest(n)` returns the latest arrivals: every message `arrived_since` lists after the
///   oldest of `newest(SAMPLE)` is in it, and `newest(1)` is one message of the latest arrival;
/// - `arrived_since(t)` holds every message of `newest` that arrived at or after `t`;
/// - `fetch` returns raw mail that parses and has a From or a Date header, and leaves out an
///   unknown id without an error (the unknown id differs from a real one only in letter case,
///   as ids are case-sensitive);
/// - fetching leaves flags unchanged (list, fetch, list again).
pub async fn id_source<S: IdSource>(source: &mut S) -> Result<(), ConformanceError<S::Error>> {
    let mut lists = Vec::new();
    for n in [NonZeroU32::MIN, SAMPLE] {
        let listed = source.newest(n).await.map_err(ConformanceError::Source)?;
        ensure(listed.len() <= n.get() as usize, || {
            format!("newest({n}) returned {} messages", listed.len())
        })?;
        let unique: BTreeSet<&RemoteId> = listed.iter().map(|m| &m.id).collect();
        ensure(unique.len() == listed.len(), || {
            format!("newest({n}) returned an id twice")
        })?;
        lists.push(listed);
    }
    let newest = lists.pop().unwrap_or_default();
    let newest_one = lists.pop().unwrap_or_default();

    let times: BTreeSet<_> = newest.iter().map(|m| m.received).collect();
    let (Some(&oldest), Some(&latest)) = (times.first(), times.last()) else {
        return Err(ConformanceError::Precondition(
            "newest listed no messages; the inbox needs two of different arrival times".into(),
        ));
    };
    if oldest == latest {
        return Err(ConformanceError::Precondition(
            "newest listed no two messages of different arrival times".into(),
        ));
    }
    let mut windows = Vec::new();
    for since in [oldest, latest] {
        let listed = source
            .arrived_since(since)
            .await
            .map_err(ConformanceError::Source)?;
        let ids: BTreeSet<&RemoteId> = listed.iter().map(|m| &m.id).collect();
        for m in newest.iter().filter(|m| m.received >= since) {
            ensure(ids.contains(&m.id), || {
                format!(
                    "arrived_since({since}) left out {}, which arrived {}",
                    m.id, m.received
                )
            })?;
        }
        windows.push(listed);
    }

    // newest(SAMPLE) holds everything that arrived after its oldest message (ties at that
    // arrival may be cut off), and the whole inbox when it holds fewer than SAMPLE.
    let full = newest.len() == SAMPLE.get() as usize;
    let in_newest: BTreeSet<&RemoteId> = newest.iter().map(|m| &m.id).collect();
    let window = windows.swap_remove(0);
    for m in window.iter().filter(|m| m.received >= oldest) {
        ensure(
            in_newest.contains(&m.id) || (full && m.received == oldest),
            || {
                format!(
                    "newest({SAMPLE}) left out {}, which arrived {}, after its oldest at {oldest}",
                    m.id, m.received
                )
            },
        )?;
    }
    // So the latest arrival is in newest(SAMPLE), and newest(1) must be one message of it.
    ensure(
        newest_one.len() == 1 && newest_one.iter().all(|m| m.received == latest),
        || {
            format!(
                "newest(1) returned {} messages, not one that arrived {latest}",
                newest_one.len()
            )
        },
    )?;

    let before = source
        .newest(SAMPLE)
        .await
        .map_err(ConformanceError::Source)?;
    let asked: Vec<RemoteId> = before.iter().take(FETCHED).map(|m| m.id.clone()).collect();
    let unknown = unknown_id(asked.first());
    let mut request = asked.clone();
    request.push(unknown.clone());
    let fetched = source
        .fetch(&request)
        .await
        .map_err(ConformanceError::Source)?;
    for f in &fetched {
        ensure(f.id != unknown, || {
            format!("fetch returned the unknown id {unknown}")
        })?;
        ensure(asked.contains(&f.id), || {
            format!("fetch returned {}, which was not asked for", f.id)
        })?;
        // mail-parser accepts almost any bytes (only empty input fails), so mail must also
        // carry a From or a Date header: a JSON message resource or an HTML page has neither.
        let mail = parse::parse(&f.raw).is_ok_and(|m| !m.from.is_empty() || m.date.is_some());
        ensure(mail, || {
            format!(
                "the source of {} does not parse as mail with a From or a Date header",
                f.id
            )
        })?;
    }
    for id in &asked {
        ensure(fetched.iter().any(|f| f.id == *id), || {
            format!("fetch left out {id}, which newest listed")
        })?;
    }

    let after = source
        .newest(SAMPLE)
        .await
        .map_err(ConformanceError::Source)?;
    let after: BTreeMap<&RemoteId, &IdListed> = after.iter().map(|m| (&m.id, m)).collect();
    for m in before.iter().filter(|m| asked.contains(&m.id)) {
        if let Some(a) = after.get(&m.id) {
            ensure(a.flags == m.flags, || {
                format!("fetching {} changed its flags", m.id)
            })?;
        }
    }
    Ok(())
}

/// An id that no source holds: `known` with the case of its last letter swapped, or a made-up
/// one when there is no message.
fn unknown_id(known: Option<&RemoteId>) -> RemoteId {
    let fallback = || RemoteId::new("neomail-conformance-unknown-id");
    let Some(known) = known else {
        return fallback().unwrap_or_else(|| unreachable!());
    };
    let mut chars: Vec<char> = known.as_str().chars().collect();
    match chars.iter().rposition(char::is_ascii_alphabetic) {
        Some(i) => {
            let c = chars[i];
            chars[i] = if c.is_ascii_lowercase() {
                c.to_ascii_uppercase()
            } else {
                c.to_ascii_lowercase()
            };
        }
        None => chars.push('x'),
    }
    RemoteId::new(chars.into_iter().collect::<String>()).unwrap_or_else(|| unreachable!())
}
