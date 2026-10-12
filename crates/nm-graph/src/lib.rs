//! Microsoft Graph as an [`IdSource`]: the INBOX of an Outlook, Hotmail or Microsoft 365 mailbox,
//! read-only, through Graph's REST API (<https://learn.microsoft.com/en-us/graph/api/user-list-messages>).
//!
//! [`Transport`] is the seam: one HTTPS GET. neomail implements it with its own HTTP client; the
//! tests give a scripted fake, so nothing here opens a connection.
//!
//! - Every request carries the access token and `Prefer: IdType="ImmutableId"`, which applies only
//!   to the request that carries it, so ids stay the same when a message moves
//!   (<https://learn.microsoft.com/en-us/graph/outlook-immutable-id>).
//! - A listing follows `@odata.nextLink` verbatim, and only to `https://graph.microsoft.com/`, so
//!   the token never leaves Graph. A delta round follows `@odata.nextLink` and keeps
//!   `@odata.deltaLink` under the same rule.
//! - Changes come from the delta query on the INBOX
//!   (<https://learn.microsoft.com/en-us/graph/api/message-delta>). A round's query options go
//!   only in its first request; Graph encodes them into its next and delta links
//!   (<https://learn.microsoft.com/en-us/graph/delta-query-messages>). The first request filters on
//!   `receivedDateTime ge`, the only filter delta supports, from the start of the cached window,
//!   so every cached message is inside the filter. Microsoft says a filtered round "can" also send
//!   read changes for messages outside it (<https://learn.microsoft.com/en-us/graph/api/message-delta>),
//!   but a live check found it did not, so nothing relies on that. With `$filter` a round holds at
//!   most [`START_CAP`] messages (<https://learn.microsoft.com/en-us/graph/delta-query-messages>):
//!   a round stops at the page that reaches it and is not used. A starting one leaves the engine
//!   listing; a stored link carries the same filter, so a later one starts over. A listing that
//!   alone holds that many asks for no round.
//! - 410 Gone, or a 4xx with `syncStateNotFound`, means the round must start over
//!   (<https://learn.microsoft.com/en-us/graph/delta-query-overview>). A round from a stored link
//!   also starts over on 400: a live check found Graph answers a link it cannot use with 400
//!   `BadRequest`. A 400 on a new round's first request stays an error, so a bad filter cannot
//!   loop.
//! - 429 and 503 are retried after `Retry-After`, else after 1, 2, 4 and 8 s, for at most
//!   [`MAX_TRIES`] tries (<https://learn.microsoft.com/en-us/graph/throttling>).
//! - At most [`CONCURRENT_FETCHES`] message downloads run at once: Outlook allows four concurrent
//!   requests per app and mailbox (<https://learn.microsoft.com/en-us/graph/throttling-limits>).
//! - Errors never quote a response body or the token.

use std::error::Error;
use std::fmt;
use std::future::Future;
use std::num::NonZeroU32;
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use futures_util::stream::{self, StreamExt, TryStreamExt};
use nm_core::{Cursor, Flags, IdChange, IdChanges, IdFetched, IdListed, IdSource, RemoteId};
use serde::Deserialize;

/// Where every request goes, and the only place a next page link may point.
pub const GRAPH_ORIGIN: &str = "https://graph.microsoft.com/";
const API: &str = "https://graph.microsoft.com/v1.0";
/// The most a JSON answer may hold.
pub const MAX_JSON: usize = 8 * 1024 * 1024;
/// The most a message's MIME source may hold.
pub const MAX_MIME: usize = 160 * 1024 * 1024;
/// Graph's largest page for messages: `$top` is 1 to 1000.
const MAX_PAGE: u32 = 1000;
/// Outlook's limit of concurrent requests per app and mailbox.
pub const CONCURRENT_FETCHES: usize = 4;
/// How many times a throttled request is sent before giving up.
pub const MAX_TRIES: u32 = 5;
/// The first wait without `Retry-After`; it doubles up to [`MAX_BACKOFF`].
const FIRST_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(16);
/// The longest `Retry-After` honoured, so a wrong value cannot stall a sync for hours.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(300);
const PREFER: (&str, &str) = ("Prefer", "IdType=\"ImmutableId\"");
const ACCEPT_JSON: (&str, &str) = ("Accept", "application/json");
const SELECT: &str = "$select=id,isRead,receivedDateTime";
const LIST_FIELDS: &str = "$select=id,isRead,receivedDateTime&$orderby=receivedDateTime%20desc";
/// The error code Graph answers when a delta link expired
/// (<https://learn.microsoft.com/en-us/graph/delta-query-overview>).
const SYNC_STATE_NOT_FOUND: &str = "syncStateNotFound";
/// The most messages a delta round with `$filter` returns
/// (<https://learn.microsoft.com/en-us/graph/delta-query-messages>). A starting round that holds
/// this many may have left cached messages out, so it is not used.
pub const START_CAP: usize = 5_000;

/// A transport's error: whatever went wrong below HTTP (DNS, TLS, a cap, a timeout).
pub type TransportError = Box<dyn Error + Send + Sync>;

/// An HTTP answer. The body can hold mail, so `Debug` leaves it out.
pub struct Reply {
    pub status: u16,
    /// `Retry-After`, when the server sent one in seconds.
    pub retry_after: Option<Duration>,
    pub body: Vec<u8>,
}

impl fmt::Debug for Reply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Reply({}, <body redacted>)", self.status)
    }
}

/// One HTTPS GET of `url` with `headers`, failing when the body is larger than `max_body`.
/// Any status is a [`Reply`]; only a failure to get one is an error.
pub trait Transport {
    fn get(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        max_body: usize,
    ) -> impl Future<Output = Result<Reply, TransportError>>;
}

#[derive(Debug, thiserror::Error)]
pub enum GraphError {
    #[error("cannot reach Microsoft Graph")]
    Transport(#[source] TransportError),
    /// 401: the access token expired or was revoked since it was fetched.
    #[error("access token refused")]
    Unauthorized,
    /// 403: Graph refuses the mailbox. The sign-in may lack Mail.Read, the account may have no
    /// mailbox or licence, or a Conditional Access policy may block the app
    /// (<https://learn.microsoft.com/en-us/graph/errors>).
    #[error("Microsoft Graph refused access to the mailbox (HTTP 403)")]
    Forbidden,
    #[error(
        "Microsoft Graph is throttling requests (HTTP {status}) after {MAX_TRIES} tries; try again later"
    )]
    Throttled { status: u16 },
    /// Any other status, with Graph's `error.code` cleaned to letters, digits, `_`, `-` and `.`.
    #[error("Microsoft Graph answered HTTP {status}{}", code_suffix(.code))]
    Status { status: u16, code: Option<String> },
    #[error("Microsoft Graph's answer is not the expected JSON")]
    Malformed,
    #[error("Microsoft Graph's next page or delta link points outside {GRAPH_ORIGIN}; refused")]
    ForeignLink,
}

fn code_suffix(code: &Option<String>) -> String {
    code.as_deref()
        .map(|c| format!(" ({c})"))
        .unwrap_or_default()
}

/// The INBOX of the signed-in user's mailbox (`/me`), read through `T`.
pub struct GraphSource<T> {
    transport: T,
    /// `Bearer <access token>`.
    authorization: String,
}

impl<T> fmt::Debug for GraphSource<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("GraphSource(<token redacted>)")
    }
}

#[derive(Deserialize)]
struct Page {
    #[serde(default)]
    value: Vec<Item>,
    #[serde(rename = "@odata.nextLink")]
    next_link: Option<String>,
    #[serde(rename = "@odata.deltaLink")]
    delta_link: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    id: Option<String>,
    /// A boolean, though Graph's own delta example shows `"true"` as a string
    /// (<https://learn.microsoft.com/en-us/graph/delta-query-messages>); both are read.
    is_read: Option<serde_json::Value>,
    received_date_time: Option<String>,
    /// Present when the message left the folder: deleted or moved.
    #[serde(rename = "@removed")]
    removed: Option<serde::de::IgnoredAny>,
}

impl Item {
    fn read(&self) -> Option<bool> {
        match self.is_read.as_ref()? {
            serde_json::Value::Bool(b) => Some(*b),
            serde_json::Value::String(s) if s == "true" => Some(true),
            serde_json::Value::String(s) if s == "false" => Some(false),
            _ => None,
        }
    }

    fn received(&self) -> Option<DateTime<Utc>> {
        Some(
            DateTime::parse_from_rfc3339(self.received_date_time.as_deref()?)
                .ok()?
                .with_timezone(&Utc),
        )
    }

    /// The listing entry; `None` without an id or a valid `receivedDateTime`.
    fn listed(self) -> Option<IdListed> {
        let received = self.received()?;
        let flags = seen_flags(self.read() == Some(true));
        Some(IdListed {
            id: RemoteId::new(self.id?)?,
            flags,
            received,
        })
    }

    /// The delta entry; `None` without an id. An update may leave out what did not change.
    fn change(self) -> Option<IdChange> {
        let read = self.read();
        let received = self.received();
        let id = RemoteId::new(self.id?)?;
        Some(match self.removed {
            Some(_) => IdChange::Removed(id),
            None => IdChange::Present {
                id,
                flags: read.map(seen_flags),
                received,
            },
        })
    }
}

fn seen_flags(read: bool) -> Flags {
    if read {
        Flags::new([Flags::SEEN])
    } else {
        Flags::default()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Folder {
    total_item_count: Option<u64>,
}

#[derive(Deserialize)]
struct ErrorBody {
    error: Option<ErrorDetail>,
}

#[derive(Deserialize)]
struct ErrorDetail {
    code: Option<String>,
}

/// Graph's `error.code` from a body, cleaned so it cannot carry escape sequences or text.
fn error_code(body: &[u8]) -> Option<String> {
    let code = serde_json::from_slice::<ErrorBody>(body)
        .ok()?
        .error?
        .code?;
    let clean: String = code
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        .take(64)
        .collect();
    (!clean.is_empty()).then_some(clean)
}

/// Percent-encodes everything but RFC 3986's unreserved characters, for a path segment.
fn encode_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The first request of a delta round over the INBOX's messages that arrived at or after `from`.
fn delta_url(from: DateTime<Utc>) -> String {
    format!(
        "{API}/me/mailFolders/inbox/messages/delta?$filter=receivedDateTime%20ge%20{}&{SELECT}",
        from.to_rfc3339_opts(SecondsFormat::Secs, true)
    )
}

/// Whether Graph refused a delta link: the round must start over.
fn is_reset(reply: &Reply) -> bool {
    reply.status == 410
        || ((400..500).contains(&reply.status)
            && error_code(&reply.body)
                .is_some_and(|c| c.eq_ignore_ascii_case(SYNC_STATE_NOT_FOUND)))
}

/// The first page of the INBOX listing, newest first, with an optional filter.
fn list_url(filter: Option<&str>, top: u32) -> String {
    let filter = filter.map(|f| format!("$filter={f}&")).unwrap_or_default();
    format!("{API}/me/mailFolders/inbox/messages?{filter}{LIST_FIELDS}&$top={top}")
}

impl<T: Transport> GraphSource<T> {
    pub fn new(transport: T, access_token: &str) -> Self {
        Self {
            transport,
            authorization: format!("Bearer {access_token}"),
        }
    }

    /// The transport, for tests that inspect what was sent.
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// The INBOX's message count: a read-only request that shows the token works and the
    /// mailbox is there (<https://learn.microsoft.com/en-us/graph/api/mailfolder-get>).
    pub async fn inbox_count(&self) -> Result<u64, GraphError> {
        let url = format!("{API}/me/mailFolders/inbox?$select=id,totalItemCount");
        let folder: Folder = self.get_json(&url).await?;
        Ok(folder.total_item_count.unwrap_or(0))
    }

    /// GETs `url`, retrying a throttled request; 401 and 403 are errors, any other status is the
    /// caller's.
    async fn get(&self, url: &str, json: bool, max_body: usize) -> Result<Reply, GraphError> {
        let auth = ("Authorization", self.authorization.as_str());
        let headers: &[(&str, &str)] = if json {
            &[auth, PREFER, ACCEPT_JSON]
        } else {
            &[auth, PREFER]
        };
        let mut backoff = FIRST_BACKOFF;
        let mut tries = 1;
        loop {
            let reply = self
                .transport
                .get(url, headers, max_body)
                .await
                .map_err(GraphError::Transport)?;
            match reply.status {
                429 | 503 if tries < MAX_TRIES => {
                    let wait = reply.retry_after.unwrap_or(backoff).min(MAX_RETRY_AFTER);
                    tokio::time::sleep(wait).await;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                    tries += 1;
                }
                status @ (429 | 503) => return Err(GraphError::Throttled { status }),
                401 => return Err(GraphError::Unauthorized),
                403 => return Err(GraphError::Forbidden),
                _ => return Ok(reply),
            }
        }
    }

    async fn get_json<D: serde::de::DeserializeOwned>(&self, url: &str) -> Result<D, GraphError> {
        let reply = self.get(url, true, MAX_JSON).await?;
        if reply.status != 200 {
            return Err(status_error(&reply));
        }
        // serde_json's errors can quote the input: never pass them on.
        serde_json::from_slice(&reply.body).map_err(|_| GraphError::Malformed)
    }

    /// Follows the listing from `url` through every next page link, or until `limit` entries.
    async fn list(
        &self,
        mut url: String,
        limit: Option<usize>,
    ) -> Result<Vec<IdListed>, GraphError> {
        let mut listed = Vec::new();
        loop {
            let page: Page = self.get_json(&url).await?;
            listed.extend(page.value.into_iter().filter_map(Item::listed));
            if let Some(limit) = limit
                && listed.len() >= limit
            {
                listed.truncate(limit);
                return Ok(listed);
            }
            match page.next_link {
                Some(next) if next.starts_with(GRAPH_ORIGIN) => url = next,
                Some(_) => return Err(GraphError::ForeignLink),
                None => return Ok(listed),
            }
        }
    }

    /// A whole delta round from `url`, through every next link to the delta link; `None` when
    /// Graph refuses the round's state. Nothing of a round is returned until its delta link is in.
    ///
    /// `stored`: `url` is a delta link from an earlier round. Graph answers one it cannot use with
    /// 400 `BadRequest` (seen live), so a 400 on such a round starts over too.
    ///
    /// `cap`: the round is not wanted once it holds this many changes, so it stops at the page
    /// that reaches it and answers `None` without asking for the rest.
    async fn round(
        &self,
        mut url: String,
        stored: bool,
        cap: Option<usize>,
    ) -> Result<Option<IdChanges>, GraphError> {
        let mut changes = Vec::new();
        loop {
            let reply = self.get(&url, true, MAX_JSON).await?;
            if is_reset(&reply) || (stored && reply.status == 400) {
                return Ok(None);
            }
            if reply.status != 200 {
                return Err(status_error(&reply));
            }
            let page: Page =
                serde_json::from_slice(&reply.body).map_err(|_| GraphError::Malformed)?;
            changes.extend(page.value.into_iter().filter_map(Item::change));
            if cap.is_some_and(|cap| changes.len() >= cap) {
                return Ok(None);
            }
            match (page.next_link, page.delta_link) {
                (Some(next), _) if next.starts_with(GRAPH_ORIGIN) => url = next,
                (None, Some(delta)) if delta.starts_with(GRAPH_ORIGIN) => {
                    let cursor = Cursor::new(delta).ok_or(GraphError::Malformed)?;
                    return Ok(Some(IdChanges { changes, cursor }));
                }
                (Some(_), _) | (None, Some(_)) => return Err(GraphError::ForeignLink),
                (None, None) => return Err(GraphError::Malformed),
            }
        }
    }

    /// One message's MIME source (<https://learn.microsoft.com/en-us/graph/api/message-get>);
    /// `None` when it is gone (404).
    async fn fetch_one(&self, id: &RemoteId) -> Result<Option<IdFetched>, GraphError> {
        let url = format!("{API}/me/messages/{}/$value", encode_segment(id.as_str()));
        let reply = self.get(&url, false, MAX_MIME).await?;
        match reply.status {
            200 => Ok(Some(IdFetched {
                id: id.clone(),
                raw: reply.body,
            })),
            404 => Ok(None),
            _ => Err(status_error(&reply)),
        }
    }
}

fn status_error(reply: &Reply) -> GraphError {
    GraphError::Status {
        status: reply.status,
        code: error_code(&reply.body),
    }
}

impl<T: Transport> IdSource for GraphSource<T> {
    type Error = GraphError;

    const START_LIMIT: Option<usize> = Some(START_CAP);

    async fn newest(&mut self, limit: NonZeroU32) -> Result<Vec<IdListed>, GraphError> {
        let url = list_url(None, limit.get().min(MAX_PAGE));
        self.list(
            url,
            Some(usize::try_from(limit.get()).unwrap_or(usize::MAX)),
        )
        .await
    }

    async fn arrived_since(&mut self, since: DateTime<Utc>) -> Result<Vec<IdListed>, GraphError> {
        // The $orderby property must come first in $filter, or Graph answers InefficientFilter
        // (<https://learn.microsoft.com/en-us/graph/api/user-list-messages>).
        let filter = format!(
            "receivedDateTime%20ge%20{}",
            since.to_rfc3339_opts(SecondsFormat::Secs, true)
        );
        self.list(list_url(Some(&filter), MAX_PAGE), None).await
    }

    async fn fetch(&mut self, ids: &[RemoteId]) -> Result<Vec<IdFetched>, GraphError> {
        let this = &*self;
        let fetched: Vec<Option<IdFetched>> = stream::iter(ids)
            .map(|id| this.fetch_one(id))
            .buffer_unordered(CONCURRENT_FETCHES)
            .try_collect()
            .await?;
        Ok(fetched.into_iter().flatten().collect())
    }

    async fn start_changes(
        &mut self,
        from: DateTime<Utc>,
    ) -> Result<Option<IdChanges>, GraphError> {
        // A round that reaches the cap may have left cached messages out, and their changes
        // would never come: list instead, without paging through the rest of it.
        self.round(delta_url(from), false, Some(START_CAP)).await
    }

    async fn changes_since(&mut self, cursor: &Cursor) -> Result<Option<IdChanges>, GraphError> {
        // Never stored by this source; a cache holding one starts over rather than send the
        // token elsewhere.
        if !cursor.as_str().starts_with(GRAPH_ORIGIN) {
            return Ok(None);
        }
        // A stored link carries the round's `$filter`, so the cap may cut it too, and the
        // changes it left out would never come: start over instead.
        self.round(cursor.as_str().to_owned(), true, Some(START_CAP))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_encoded_for_a_path_segment() {
        assert_eq!(encode_segment("AAMk+a/b=c-_.~"), "AAMk%2Ba%2Fb%3Dc-_.~");
    }

    #[test]
    fn error_codes_are_cleaned_and_bodies_never_quoted() {
        let body = br#"{"error":{"code":"Bad\u001b[31mCode Here","message":"s3cret text"}}"#;
        let e = status_error(&Reply {
            status: 400,
            retry_after: None,
            body: body.to_vec(),
        });
        assert_eq!(
            e.to_string(),
            "Microsoft Graph answered HTTP 400 (Bad31mCodeHere)"
        );
        let e = status_error(&Reply {
            status: 500,
            retry_after: None,
            body: b"<html>s3cret</html>".to_vec(),
        });
        assert_eq!(e.to_string(), "Microsoft Graph answered HTTP 500");
    }
}
