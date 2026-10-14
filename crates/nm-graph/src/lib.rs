//! Microsoft Graph as an [`IdSource`]: the INBOX of an Outlook, Hotmail or Microsoft 365 mailbox,
//! read through Graph's REST API (<https://learn.microsoft.com/en-us/graph/api/user-list-messages>).
//! It writes a message's read state ([`GraphSource::set_read`]) and its categories
//! ([`GraphSource::set_categories`]), and moves a message to the archive
//! or the trash ([`GraphSource::move_to`]).
//!
//! It also sends mail ([`GraphSource::send`]).
//!
//! [`Transport`] is the seam: one HTTPS GET, or one PATCH or POST with a JSON body, or one POST
//! with a text body. neomail
//! implements it with its own HTTP client; the tests give a scripted fake, so nothing here opens a
//! connection.
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
//! - A read state is written with `PATCH /me/messages/{id}` and `{"isRead": ...}`, which needs
//!   Mail.ReadWrite (<https://learn.microsoft.com/en-us/graph/api/message-update>).
//! - A message's categories are set by name with a GET of `categories`, then a `PATCH
//!   /me/messages/{id}` of the merged list, which needs Mail.ReadWrite too; nothing here touches
//!   the master category list (<https://learn.microsoft.com/en-us/graph/api/resources/outlookcategory>).
//! - A move is `POST /me/messages/{id}/move` with a well-known folder name as `destinationId`,
//!   which also needs Mail.ReadWrite and answers `201 Created`
//!   (<https://learn.microsoft.com/en-us/graph/api/message-move>). Nothing here deletes a message
//!   for good, or creates, renames or deletes a folder.
//! - At most [`CONCURRENT_FETCHES`] message downloads or writes run at once: Outlook allows four concurrent
//!   requests per app and mailbox (<https://learn.microsoft.com/en-us/graph/throttling-limits>).
//! - A message is sent with `POST /me/sendMail` and its MIME source, base64-encoded, as
//!   `Content-Type: text/plain`; Graph answers `202 Accepted` and saves the message in Sent Items
//!   itself. That needs Mail.Send (<https://learn.microsoft.com/en-us/graph/api/user-sendmail>).
//! - Errors never quote a response body or the token.

use std::error::Error;
use std::fmt;
use std::future::Future;
use std::num::NonZeroU32;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use chrono::{DateTime, SecondsFormat, Utc};
use futures_util::stream::{self, StreamExt, TryStreamExt};
use nm_core::compose::Outgoing;
use nm_core::label::{LabelPrefix, merge};
use nm_core::{
    Cursor, Destination, Flags, FolderName, IdChange, IdChanges, IdFetched, IdListed, IdSource,
    ListedFolder, Moved, RemoteId, Role, Written,
};
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

/// One HTTPS request to `url` with `headers`, failing when the answer's body is larger than
/// `max_body`. Any status is a [`Reply`]; only a failure to get one is an error.
pub trait Transport {
    /// A GET.
    fn get(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        max_body: usize,
    ) -> impl Future<Output = Result<Reply, TransportError>>;

    /// A PATCH with `body`, sent as `Content-Type: application/json`: the transport sets that
    /// header, so `headers` never holds it.
    fn patch_json(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        body: &[u8],
        max_body: usize,
    ) -> impl Future<Output = Result<Reply, TransportError>>;

    /// A POST with `body`, sent as `Content-Type: application/json`, as for
    /// [`Self::patch_json`].
    fn post_json(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        body: &[u8],
        max_body: usize,
    ) -> impl Future<Output = Result<Reply, TransportError>>;

    /// A POST with `body`, sent as `Content-Type: text/plain`, as for [`Self::patch_json`].
    fn post_text(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        body: &[u8],
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
    /// 403: Graph refuses the mailbox. The sign-in may lack Mail.Read (or Mail.ReadWrite for a
    /// write), the account may have no mailbox or licence, or a Conditional Access policy may
    /// block the app (<https://learn.microsoft.com/en-us/graph/errors>).
    #[error("Microsoft Graph refused access to the mailbox (HTTP 403)")]
    Forbidden,
    #[error(
        "Microsoft Graph is throttling requests (HTTP {status}) after {MAX_TRIES} tries; try again later"
    )]
    Throttled { status: u16 },
    /// Any other status, with Graph's `error.code` cleaned to letters, digits, `_`, `-` and `.`.
    #[error("Microsoft Graph answered HTTP {status}{}", code_suffix(.code))]
    Status { status: u16, code: Option<String> },
    /// 403 on sendMail: the sign-in may lack Mail.Send, or the account may not send.
    #[error(
        "Microsoft Graph refused to send the message (HTTP 403): the sign-in may not include \
         Mail.Send; nothing was sent"
    )]
    CannotSend,
    /// sendMail failed in a way that does not say the message was refused: a transport error,
    /// which may come after Graph got the request, or a 5xx. It may have been sent.
    #[error(
        "Microsoft Graph did not confirm the message, so it may have been sent; check Sent Items \
         before sending it again"
    )]
    Unconfirmed(#[source] Box<GraphError>),
    /// sendMail takes its recipients from the message's headers, and Bcc is in none.
    #[error(
        "Outlook accounts cannot send Bcc yet: Microsoft Graph takes the recipients from the \
         message's headers, which never hold Bcc; nothing was sent"
    )]
    Bcc,
    #[error("Microsoft Graph's answer is not the expected JSON")]
    Malformed,
    #[error("Microsoft Graph's next page or delta link points outside {GRAPH_ORIGIN}; refused")]
    ForeignLink,
    /// 404 for a well-known folder: the mailbox has no folder with that name, so nothing moved.
    #[error("the mailbox has no {0:?} folder in Microsoft Graph")]
    NoFolder(&'static str),
    /// No folder has this path of display names, so nothing moved.
    #[error("the mailbox has no folder named \"{0}\"; nothing was moved")]
    NoSuchFolder(FolderName),
    /// The path names the inbox, found by its id.
    #[error("the messages are in the Inbox already; nothing was moved")]
    IntoInbox,
    /// The path names a search folder, whose messages are a query's results.
    #[error("\"{0}\" is a search folder, which cannot hold messages; nothing was moved")]
    SearchFolder(FolderName),
    /// Several folders have this path, because a display name holds a `/`.
    #[error(
        "several folders are named \"{0}\" (a folder's name holds a \"/\"); nothing was moved, \
         so use Outlook on the web to move to it"
    )]
    SharedPath(FolderName),
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

/// A next or delta link Graph answered with, refused unless it is on [`GRAPH_ORIGIN`]: every
/// paging read follows links only through here, so the bearer token never leaves Graph.
fn on_graph(link: String) -> Result<String, GraphError> {
    if link.starts_with(GRAPH_ORIGIN) {
        Ok(link)
    } else {
        Err(GraphError::ForeignLink)
    }
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
        retrying(THROTTLED, || self.transport.get(url, headers, max_body)).await
    }

    /// PATCHes `url` with the JSON `body`, under the same rules as [`Self::get`].
    async fn patch(&self, url: &str, body: &[u8], max_body: usize) -> Result<Reply, GraphError> {
        let auth = ("Authorization", self.authorization.as_str());
        let headers = [auth, PREFER, ACCEPT_JSON];
        retrying(THROTTLED, || {
            self.transport.patch_json(url, &headers, body, max_body)
        })
        .await
    }

    /// POSTs the JSON `body` to `url`, under the same rules as [`Self::get`].
    async fn post(&self, url: &str, body: &[u8], max_body: usize) -> Result<Reply, GraphError> {
        let auth = ("Authorization", self.authorization.as_str());
        let headers = [auth, PREFER, ACCEPT_JSON];
        retrying(THROTTLED, || {
            self.transport.post_json(url, &headers, body, max_body)
        })
        .await
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
                Some(next) => url = on_graph(next)?,
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
                (Some(next), _) => url = on_graph(next)?,
                (None, Some(delta)) => {
                    let cursor = Cursor::new(on_graph(delta)?).ok_or(GraphError::Malformed)?;
                    return Ok(Some(IdChanges { changes, cursor }));
                }
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

/// Graph's well-known name for the folder with role `to`
/// (<https://learn.microsoft.com/en-us/graph/api/resources/mailfolder>): `archive` is the folder
/// Outlook's one-click Archive moves to, `deleteditems` the one a deleted message goes to, and
/// `junkemail` the junk email folder.
pub fn well_known(to: nm_core::Folder) -> &'static str {
    match to {
        nm_core::Folder::Archive => "archive",
        nm_core::Folder::Trash => "deleteditems",
        nm_core::Folder::Junk => "junkemail",
    }
}

impl<T: Transport> GraphSource<T> {
    /// Moves the messages `ids` to the folder `to`, at most [`CONCURRENT_FETCHES`] at once.
    ///
    /// First the folder is found, read-only, so that nothing moves without it and a 404 on a
    /// move means the message is gone, not the folder:
    /// - a role is its well-known name, which a `GET` shows the mailbox has (a 404 there is
    ///   [`GraphError::NoFolder`]), and the moves send that name;
    /// - a name is a path of display names, found by [`Self::find`]; the moves send the
    ///   folder's id. The inbox is refused by its id ([`GraphError::IntoInbox`]), since its
    ///   display name follows the mailbox's language.
    ///
    /// Then each message gets `POST /me/messages/{id}/move`
    /// (<https://learn.microsoft.com/en-us/graph/api/message-move>): 201 is moved, 404 is gone.
    /// Needs Mail.ReadWrite: without it Graph answers 403, [`GraphError::Forbidden`]. On an error
    /// some messages may have moved already; the next sync finds them gone from the INBOX.
    /// `Moved::folder` is the well-known name for a role and the path for a name.
    pub async fn move_to(
        &self,
        ids: &[RemoteId],
        to: &Destination,
    ) -> Result<Moved<RemoteId>, GraphError> {
        let folder = match to {
            Destination::Role(role) => well_known(*role).to_owned(),
            Destination::Name(path) => path.as_str().to_owned(),
        };
        let mut result = Moved {
            folder,
            moved: Vec::new(),
            gone: Vec::new(),
        };
        if ids.is_empty() {
            return Ok(result);
        }
        let destination = match to {
            Destination::Role(role) => {
                let name = well_known(*role);
                if self.well_known_id(name).await?.is_none() {
                    return Err(GraphError::NoFolder(name));
                }
                name.to_owned()
            }
            Destination::Name(path) => {
                let id = self.find(path).await?;
                if self.well_known_id("inbox").await?.as_ref() == Some(&id) {
                    return Err(GraphError::IntoInbox);
                }
                id
            }
        };
        let body = serde_json::json!({ "destinationId": destination }).to_string();
        let answers: Vec<(RemoteId, bool)> = stream::iter(ids)
            .map(|id| async {
                let moved = self.move_one(id, body.as_bytes()).await?;
                Ok::<_, GraphError>((id.clone(), moved))
            })
            .buffer_unordered(CONCURRENT_FETCHES)
            .try_collect()
            .await?;
        for id in ids {
            match answers.iter().find(|(a, _)| a == id) {
                Some((_, true)) => result.moved.push(id.clone()),
                Some((_, false)) => result.gone.push(id.clone()),
                None => {}
            }
        }
        Ok(result)
    }

    /// One move: `true` when Graph moved the message (201), `false` when it is gone (404). The
    /// answer is the whole message, its body included, so it gets [`MAX_MIME`].
    async fn move_one(&self, id: &RemoteId, body: &[u8]) -> Result<bool, GraphError> {
        let url = format!("{API}/me/messages/{}/move", encode_segment(id.as_str()));
        let reply = self.post(&url, body, MAX_MIME).await?;
        match reply.status {
            201 => Ok(true),
            404 => Ok(false),
            _ => Err(status_error(&reply)),
        }
    }
}

/// A listing of mail folders: their id, display name and child count, 100 to a page (the
/// default is 10: <https://learn.microsoft.com/en-us/graph/api/user-list-mailfolders>).
const FOLDER_FIELDS: &str = "$select=id,displayName,childFolderCount&$top=100";

/// A search folder's `@odata.type`: its messages are a query's results, so it cannot hold a moved
/// message, and it is left out
/// (<https://learn.microsoft.com/en-us/graph/api/user-list-mailfolders>).
const SEARCH_FOLDER: &str = "#microsoft.graph.mailSearchFolder";

/// The roles `folder list` shows and Graph's well-known name for each
/// (<https://learn.microsoft.com/en-us/graph/api/resources/mailfolder>). Graph has no folder for
/// all mail or for flagged mail.
const ROLES: [(Role, &str); 6] = [
    (Role::Inbox, "inbox"),
    (Role::Archive, "archive"),
    (Role::Trash, "deleteditems"),
    (Role::Junk, "junkemail"),
    (Role::Sent, "sentitems"),
    (Role::Drafts, "drafts"),
];

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MailFolder {
    id: Option<String>,
    display_name: Option<String>,
    child_folder_count: Option<u64>,
    #[serde(rename = "@odata.type")]
    odata_type: Option<String>,
}

impl MailFolder {
    /// Its id and display name; `None` for a search folder or one without either.
    fn usable(&self) -> Option<(&str, &str)> {
        if self.odata_type.as_deref() == Some(SEARCH_FOLDER) {
            return None;
        }
        let name = self.display_name.as_deref().filter(|n| !n.is_empty())?;
        Some((self.id.as_deref()?, name))
    }

    fn has_children(&self) -> bool {
        self.child_folder_count.is_some_and(|n| n > 0)
    }
}

#[derive(Deserialize)]
struct FolderPage {
    #[serde(default)]
    value: Vec<MailFolder>,
    #[serde(rename = "@odata.nextLink")]
    next_link: Option<String>,
}

#[derive(Deserialize)]
struct FolderId {
    id: Option<String>,
}

/// A folder found while listing, waiting for its children.
struct Found {
    id: String,
    path: String,
    has_children: bool,
}

impl<T: Transport> GraphSource<T> {
    /// The folders of the mailbox a message can be in, each a [`ListedFolder`] named by its
    /// path of display names from the top of the mailbox (`Inbox/Receipts`), depth first in
    /// Graph's order. `GET /me/mailFolders`, then `GET /me/mailFolders/{id}/childFolders` for
    /// each folder with children, each through every next link
    /// (<https://learn.microsoft.com/en-us/graph/api/mailfolder-list-childfolders>); hidden and
    /// search folders are left out. Roles come from one `GET` per well-known name in [`ROLES`].
    /// Needs Mail.Read or Mail.ReadBasic; changes nothing.
    pub async fn folders(&self) -> Result<Vec<ListedFolder>, GraphError> {
        let roles: Vec<(Role, Option<String>)> = stream::iter(ROLES)
            .map(|(role, name)| async move { Ok((role, self.well_known_id(name).await?)) })
            .buffered(CONCURRENT_FETCHES)
            .try_collect()
            .await?;
        let mut listed = Vec::new();
        let mut pending = Vec::new();
        push_found(&mut pending, self.child_folders(None).await?, "");
        while let Some(found) = pending.pop() {
            if found.has_children {
                let children = self.child_folders(Some(&found.id)).await?;
                push_found(&mut pending, children, &found.path);
            }
            let role = roles
                .iter()
                .find(|(_, id)| id.as_deref() == Some(found.id.as_str()))
                .map(|(role, _)| *role);
            if let Some(name) = FolderName::new(found.path) {
                listed.push(ListedFolder { name, role });
            }
        }
        Ok(listed)
    }

    /// The id of the folder at `path`, a path of display names joined with `/` as
    /// [`Self::folders`] prints it. Only the levels that lead to `path` are listed: the top, then
    /// the children of each folder whose name starts what is left of the path, so a move never
    /// walks the whole tree. A display name may hold a `/`, so a path can lead to several
    /// folders: that is [`GraphError::SharedPath`]. None is [`GraphError::NoSuchFolder`]; a
    /// search folder is [`GraphError::SearchFolder`].
    async fn find(&self, path: &FolderName) -> Result<String, GraphError> {
        let mut found: Vec<(String, bool)> = Vec::new();
        let mut pending: Vec<(Option<String>, &str)> = vec![(None, path.as_str())];
        while let Some((parent, rest)) = pending.pop() {
            for folder in self.child_folders(parent.as_deref()).await? {
                let search = folder.odata_type.as_deref() == Some(SEARCH_FOLDER);
                let (Some(id), Some(name)) = (&folder.id, &folder.display_name) else {
                    continue;
                };
                if name.is_empty() {
                    continue;
                }
                if name == rest {
                    found.push((id.clone(), search));
                } else if let Some(deeper) = rest.strip_prefix(name.as_str())
                    && let Some(deeper) = deeper.strip_prefix('/')
                    && folder.has_children()
                    && !search
                {
                    pending.push((Some(id.clone()), deeper));
                }
            }
        }
        match found.as_slice() {
            [] => Err(GraphError::NoSuchFolder(path.clone())),
            [(_, true)] => Err(GraphError::SearchFolder(path.clone())),
            [(id, false)] => Ok(id.clone()),
            _ => Err(GraphError::SharedPath(path.clone())),
        }
    }

    /// The folders directly under the folder `parent`, or at the top of the mailbox, through
    /// every next link.
    async fn child_folders(&self, parent: Option<&str>) -> Result<Vec<MailFolder>, GraphError> {
        let mut url = match parent {
            None => format!("{API}/me/mailFolders?{FOLDER_FIELDS}"),
            Some(id) => format!(
                "{API}/me/mailFolders/{}/childFolders?{FOLDER_FIELDS}",
                encode_segment(id)
            ),
        };
        let mut folders = Vec::new();
        loop {
            let page: FolderPage = self.get_json(&url).await?;
            folders.extend(page.value);
            match page.next_link {
                Some(next) => url = on_graph(next)?,
                None => return Ok(folders),
            }
        }
    }

    /// The id of the folder with the well-known name `name`; `None` when the mailbox has no
    /// such folder (404).
    async fn well_known_id(&self, name: &str) -> Result<Option<String>, GraphError> {
        let url = format!("{API}/me/mailFolders/{name}?$select=id");
        let reply = self.get(&url, true, MAX_JSON).await?;
        match reply.status {
            200 => serde_json::from_slice::<FolderId>(&reply.body)
                .ok()
                .and_then(|f| f.id)
                .map(Some)
                .ok_or(GraphError::Malformed),
            404 => Ok(None),
            _ => Err(status_error(&reply)),
        }
    }
}

/// Pushes the usable `folders` under `prefix` onto `pending`, last first, so that they come off
/// it in Graph's order.
fn push_found(pending: &mut Vec<Found>, folders: Vec<MailFolder>, prefix: &str) {
    for folder in folders.iter().rev() {
        let Some((id, name)) = folder.usable() else {
            continue;
        };
        let path = if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}/{name}")
        };
        pending.push(Found {
            id: id.to_owned(),
            path,
            has_children: folder.has_children(),
        });
    }
}

/// What one PATCH answered: the message's read state when the answer holds it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Patched {
    is_read: Option<bool>,
}

impl<T: Transport> GraphSource<T> {
    /// Sets the read state of the messages `ids` with `PATCH /me/messages/{id}` and
    /// `{"isRead": read}` (<https://learn.microsoft.com/en-us/graph/api/message-update>), at most
    /// [`CONCURRENT_FETCHES`] at once. A message Graph no longer has (404) is gone; the rest are
    /// done, with the flags Graph's answer shows. Needs Mail.ReadWrite: without it Graph answers
    /// 403, [`GraphError::Forbidden`]. On an error some messages may have changed already; the
    /// next sync reads their state.
    pub async fn set_read(
        &self,
        ids: &[RemoteId],
        read: bool,
    ) -> Result<Written<RemoteId>, GraphError> {
        let body = serde_json::json!({ "isRead": read }).to_string();
        let answers: Vec<(RemoteId, Option<Option<Flags>>)> = stream::iter(ids)
            .map(|id| async {
                let answer = self.set_read_one(id, body.as_bytes()).await?;
                Ok::<_, GraphError>((id.clone(), answer))
            })
            .buffer_unordered(CONCURRENT_FETCHES)
            .try_collect()
            .await?;
        let mut written = Written::default();
        for id in ids {
            match answers.iter().find(|(a, _)| a == id) {
                Some((_, Some(flags))) => written.done.push((id.clone(), flags.clone())),
                Some((_, None)) => written.gone.push(id.clone()),
                None => {}
            }
        }
        Ok(written)
    }

    /// One PATCH: `None` when the message is gone (404), else its flags when the answer holds
    /// `isRead`. The answer is the whole message, its body included, so it gets
    /// [`MAX_MIME`] rather than [`MAX_JSON`].
    async fn set_read_one(
        &self,
        id: &RemoteId,
        body: &[u8],
    ) -> Result<Option<Option<Flags>>, GraphError> {
        let url = format!("{API}/me/messages/{}", encode_segment(id.as_str()));
        let reply = self.patch(&url, body, MAX_MIME).await?;
        match reply.status {
            200 | 204 => Ok(Some(
                serde_json::from_slice::<Patched>(&reply.body)
                    .ok()
                    .and_then(|p| p.is_read)
                    .map(seen_flags),
            )),
            404 => Ok(None),
            _ => Err(status_error(&reply)),
        }
    }
}

/// A group to set on a message as an Outlook category: [`GraphSource::set_categories`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategoryEdit {
    pub id: RemoteId,
    /// The category the message should carry: the prefix and its group's name.
    pub want: String,
    /// The category neomail wrote on it last, if any: neomail's even under another prefix.
    pub last: Option<String>,
}

/// What became of one [`CategoryEdit`].
#[derive(Debug)]
pub enum CategoryResult {
    /// The categories were changed with a PATCH.
    Set,
    /// The message carried the category already, and nothing of neomail's to remove: no PATCH.
    Already,
    /// Graph no longer has the message (404).
    Gone,
    /// The request failed. [`GraphError::Unauthorized`] and [`GraphError::Forbidden`] end the
    /// pass: the edits after them are not tried, and are left out of the answer.
    Failed(GraphError),
}

#[derive(Deserialize)]
struct Categorized {
    categories: Option<Vec<String>>,
}

impl<T: Transport> GraphSource<T> {
    /// Sets each message's Outlook category: `GET /me/messages/{id}?$select=categories`, then
    /// [`nm_core::label::merge`] keeps every category that is not neomail's, and only a list that
    /// changed is sent back with `PATCH /me/messages/{id}` and `{"categories": [...]}`
    /// (<https://learn.microsoft.com/en-us/graph/api/message-update>), which needs
    /// Mail.ReadWrite. A category is applied by its name alone; the master category list is not
    /// touched (<https://learn.microsoft.com/en-us/graph/api/resources/outlookcategory>). At most
    /// [`CONCURRENT_FETCHES`] messages at once, each with Graph's retry rules.
    ///
    /// One message's failure never stops the others, except 401 and 403, which stop the pass:
    /// the token or the grant is wrong for every message. The answer is in `edits`' order.
    pub async fn set_categories(
        &self,
        prefix: &LabelPrefix,
        edits: &[CategoryEdit],
    ) -> Vec<(RemoteId, CategoryResult)> {
        let mut answers = stream::iter(edits.iter().enumerate())
            .map(|(i, edit)| async move { (i, self.set_categories_one(prefix, edit).await) })
            .buffer_unordered(CONCURRENT_FETCHES);
        let mut results: Vec<Option<CategoryResult>> = edits.iter().map(|_| None).collect();
        while let Some((i, result)) = answers.next().await {
            let refused = matches!(
                result,
                CategoryResult::Failed(GraphError::Unauthorized | GraphError::Forbidden)
            );
            results[i] = Some(result);
            if refused {
                break;
            }
        }
        edits
            .iter()
            .zip(results)
            .filter_map(|(edit, result)| result.map(|r| (edit.id.clone(), r)))
            .collect()
    }

    async fn set_categories_one(
        &self,
        prefix: &LabelPrefix,
        edit: &CategoryEdit,
    ) -> CategoryResult {
        let url = format!("{API}/me/messages/{}", encode_segment(edit.id.as_str()));
        let reply = match self
            .get(&format!("{url}?$select=categories"), true, MAX_JSON)
            .await
        {
            Ok(reply) => reply,
            Err(e) => return CategoryResult::Failed(e),
        };
        let current = match reply.status {
            200 => match serde_json::from_slice::<Categorized>(&reply.body) {
                Ok(c) => c.categories.unwrap_or_default(),
                Err(_) => return CategoryResult::Failed(GraphError::Malformed),
            },
            404 => return CategoryResult::Gone,
            _ => return CategoryResult::Failed(status_error(&reply)),
        };
        let Some(merged) = merge(&current, prefix, edit.last.as_deref(), &edit.want) else {
            return CategoryResult::Already;
        };
        let body = serde_json::json!({ "categories": merged }).to_string();
        // The answer is the whole message, its body included, as for set_read.
        match self.patch(&url, body.as_bytes(), MAX_MIME).await {
            Ok(reply) => match reply.status {
                200 | 204 => CategoryResult::Set,
                404 => CategoryResult::Gone,
                _ => CategoryResult::Failed(status_error(&reply)),
            },
            Err(e) => CategoryResult::Failed(e),
        }
    }
}

/// The statuses a request is sent again after: Graph's throttling answers
/// (<https://learn.microsoft.com/en-us/graph/throttling>).
const THROTTLED: &[u16] = &[429, 503];

/// Sends with `send` until the answer is not throttled: a status in `retry` is sent again after
/// `Retry-After`, else after a doubling backoff, for at most [`MAX_TRIES`] tries. 401 and 403
/// are errors; any other status is the caller's.
async fn retrying<F, Fut>(retry: &[u16], mut send: F) -> Result<Reply, GraphError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Reply, TransportError>>,
{
    let mut backoff = FIRST_BACKOFF;
    let mut tries = 1;
    loop {
        let reply = send().await.map_err(GraphError::Transport)?;
        match reply.status {
            status if retry.contains(&status) && tries < MAX_TRIES => {
                let wait = reply.retry_after.unwrap_or(backoff).min(MAX_RETRY_AFTER);
                tokio::time::sleep(wait).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
                tries += 1;
            }
            status if retry.contains(&status) => return Err(GraphError::Throttled { status }),
            401 => return Err(GraphError::Unauthorized),
            403 => return Err(GraphError::Forbidden),
            _ => return Ok(reply),
        }
    }
}

impl<T: Transport> GraphSource<T> {
    /// Sends `message` with `POST /me/sendMail`: its MIME source, base64-encoded, as
    /// `Content-Type: text/plain` (<https://learn.microsoft.com/en-us/graph/api/user-sendmail>).
    /// Ok is Graph's `202 Accepted`, which means accepted, not delivered. Graph saves the copy in
    /// Sent Items itself.
    ///
    /// Graph takes no envelope and reads the recipients from the headers, so a message with Bcc
    /// recipients is refused before any request: [`GraphError::Bcc`]. 403 is
    /// [`GraphError::CannotSend`]. Only 429 is sent again: it says the request was not
    /// processed, and a 503 might come after Graph accepted the message, which a retry would send
    /// twice. So a 5xx or a transport error is [`GraphError::Unconfirmed`]: it may have been sent.
    pub async fn send(&self, message: &Outgoing) -> Result<(), GraphError> {
        if !message.bcc.is_empty() {
            return Err(GraphError::Bcc);
        }
        let body = BASE64.encode(&message.mime);
        let url = format!("{API}/me/sendMail");
        let auth = ("Authorization", self.authorization.as_str());
        let headers = [auth, ACCEPT_JSON];
        let reply = retrying(&[429], || {
            self.transport
                .post_text(&url, &headers, body.as_bytes(), MAX_JSON)
        })
        .await
        .map_err(|e| match e {
            GraphError::Forbidden => GraphError::CannotSend,
            e @ GraphError::Transport(_) => GraphError::Unconfirmed(Box::new(e)),
            e => e,
        })?;
        match reply.status {
            202 => Ok(()),
            500.. => Err(GraphError::Unconfirmed(Box::new(status_error(&reply)))),
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
