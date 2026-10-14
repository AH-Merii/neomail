//! Sending a message whose files are too large for one sendMail request, as Microsoft documents
//! it (<https://learn.microsoft.com/en-us/graph/outlook-large-attachments>):
//!
//! 1. The message without its files is saved as a draft: `POST /me/messages` with its MIME
//!    source, base64-encoded, as `text/plain`, which answers `201 Created` and the draft's id
//!    (<https://learn.microsoft.com/en-us/graph/api/user-post-messages>). Needs Mail.ReadWrite.
//! 2. Each file is attached on its own, "based on its file size": one under 3 MB with a single
//!    `POST /me/messages/{id}/attachments` of a fileAttachment
//!    (<https://learn.microsoft.com/en-us/graph/api/message-post-attachments>); one of 3 MB or
//!    more through an upload session, `POST .../attachments/createUploadSession`, then its bytes
//!    in order, in `PUT`s of a range each to the session's `uploadUrl`, with `Content-Range` and
//!    no `Authorization` header (the URL carries its own token). A range is under 4 MB, as the
//!    page asks. Each `PUT` answers `200 OK` with `nextExpectedRanges`, the last `201 Created`.
//! 3. The draft is sent: `POST /me/messages/{id}/send`, which answers `202 Accepted`
//!    (<https://learn.microsoft.com/en-us/graph/api/message-send>).
//!
//! Only step 3 sends anything, so a failure before it is [`GraphError::Unsent`]. A failed step 1
//! that may still have saved the draft (a lost answer, a 5xx, an answer without an id) says a
//! draft may be left in Drafts; a failed step 2 deletes the draft. A refused step 3 (a 4xx, or
//! throttling) keeps the draft and says so. A failure of step 3 that does not say the message
//! was refused may hide a sent message, so it is [`GraphError::Unconfirmed`], never retried, and
//! the draft is kept. Requests that add something to the draft retry only 429: a 503 may come
//! after Graph did the work, and a retry would attach the same file twice.
//!
//! "MB" is read as 1,000,000 bytes, the smaller reading, so a file at the edge takes the path
//! that works under either.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use nm_core::compose::{AttachedFile, Outgoing};
use serde::Deserialize;
use serde_json::json;

use super::{
    ACCEPT_JSON, API, DraftLeft, GraphError, GraphSource, MAX_JSON, PREFER, Reply, Transport,
    encode_segment, retrying, status_error,
};

/// A file this size or larger goes through an upload session: "If the file size is between 3
/// MB and 150 MB, create an upload session".
pub const UPLOAD_SESSION_FROM: u64 = 3_000_000;
/// The largest sendMail request neomail sends: 4 MB, the MIME payload limit Microsoft documents
/// for Create message, which a file of 3 MB reaches once base64-encoded.
pub const MAX_SENDMAIL_REQUEST: usize = 4_000_000;
/// How many bytes one `PUT` carries: 10 × 320 KiB, under the 4 MB the page asks for.
pub const RANGE_SIZE: usize = 10 * 320 * 1024;
/// Where an upload session's URL may point: "the `https://outlook.office.com` domain", and the
/// Microsoft 365 name of the same service.
pub(crate) const UPLOAD_ORIGINS: [&str; 2] = [
    "https://outlook.office.com/",
    "https://outlook.office365.com/",
];

/// Whether `message`, built with `files`, goes in one sendMail request: no file needs an upload
/// session, and the request, base64, stays within [`MAX_SENDMAIL_REQUEST`].
pub fn fits_send_mail(message: &Outgoing, files: &[AttachedFile]) -> bool {
    files.iter().all(|f| f.size() < UPLOAD_SESSION_FROM)
        && base64::encoded_len(message.mime.len(), true).unwrap_or(usize::MAX)
            <= MAX_SENDMAIL_REQUEST
}

#[derive(Deserialize)]
struct Created {
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UploadSession {
    upload_url: Option<String>,
    #[serde(default)]
    next_expected_ranges: Vec<String>,
}

impl<T: Transport> GraphSource<T> {
    /// Sends `bare` (the message without its files, [`nm_core::compose::Draft::build_without_files`])
    /// with `files`, through a draft as the module says. Ok is the draft's `202 Accepted`.
    pub async fn send_with_files(
        &self,
        bare: &Outgoing,
        files: &[AttachedFile],
    ) -> Result<(), GraphError> {
        if !bare.bcc.is_empty() {
            return Err(GraphError::Bcc);
        }
        let id = self.create_draft(bare).await.map_err(|e| match e {
            e @ (GraphError::Unauthorized | GraphError::CannotDraft) => e,
            e => {
                let draft_left = if may_have_saved(&e) {
                    DraftLeft::Maybe
                } else {
                    DraftLeft::No
                };
                GraphError::Unsent {
                    source: Box::new(e),
                    draft_left,
                }
            }
        })?;
        let mut attached = Ok(());
        for file in files {
            attached = if file.size() < UPLOAD_SESSION_FROM {
                self.attach_small(&id, file).await
            } else {
                self.upload(&id, file).await
            };
            if attached.is_err() {
                break;
            }
        }
        if let Err(e) = attached {
            let removed = self.delete_draft(&id).await;
            return Err(GraphError::Unsent {
                source: Box::new(e),
                draft_left: if removed {
                    DraftLeft::No
                } else {
                    DraftLeft::Yes
                },
            });
        }
        self.send_draft(&id).await
    }

    fn auth(&self) -> (&str, &str) {
        ("Authorization", self.authorization.as_str())
    }

    /// Saves `bare` as a draft and returns its id. Only 429 is retried: any other answer may
    /// have saved one, and a second would be left behind.
    async fn create_draft(&self, bare: &Outgoing) -> Result<String, GraphError> {
        let body = BASE64.encode(&bare.mime);
        let url = format!("{API}/me/messages");
        let headers = [self.auth(), PREFER, ACCEPT_JSON];
        let reply = retrying(&[429], || {
            self.transport
                .post_text(&url, &headers, body.as_bytes(), MAX_JSON)
        })
        .await
        .map_err(|e| match e {
            GraphError::Forbidden => GraphError::CannotDraft,
            e => e,
        })?;
        if reply.status != 201 {
            return Err(status_error(&reply));
        }
        let created: Created =
            serde_json::from_slice(&reply.body).map_err(|_| GraphError::Malformed)?;
        Ok(created.id)
    }

    /// POSTs the JSON `body` to `url`, retrying only 429, which says Graph did nothing.
    async fn post_once(&self, url: &str, body: &[u8]) -> Result<Reply, GraphError> {
        let headers = [self.auth(), PREFER, ACCEPT_JSON];
        retrying(&[429], || {
            self.transport.post_json(url, &headers, body, MAX_JSON)
        })
        .await
    }

    fn attachments_url(id: &str) -> String {
        format!("{API}/me/messages/{}/attachments", encode_segment(id))
    }

    async fn attach_small(&self, id: &str, file: &AttachedFile) -> Result<(), GraphError> {
        let body = json!({
            "@odata.type": "#microsoft.graph.fileAttachment",
            "name": file.name(),
            "contentType": file.media_type(),
            "contentBytes": BASE64.encode(file.bytes()),
        });
        let body = serde_json::to_vec(&body).map_err(|_| GraphError::Malformed)?;
        let reply = self.post_once(&Self::attachments_url(id), &body).await?;
        expect(&reply, 201)
    }

    /// Opens an upload session for `file` and sends its bytes, a range at a time, from where
    /// the session says it expects the next one.
    async fn upload(&self, id: &str, file: &AttachedFile) -> Result<(), GraphError> {
        let total = file.bytes().len();
        let body = json!({ "AttachmentItem": {
            "attachmentType": "file",
            "name": file.name(),
            "size": total,
            "contentType": file.media_type(),
        }});
        let body = serde_json::to_vec(&body).map_err(|_| GraphError::Malformed)?;
        let url = format!("{}/createUploadSession", Self::attachments_url(id));
        let reply = self.post_once(&url, &body).await?;
        expect(&reply, 201)?;
        let session: UploadSession =
            serde_json::from_slice(&reply.body).map_err(|_| GraphError::Malformed)?;
        let upload_url = session.upload_url.ok_or(GraphError::Malformed)?;
        if !UPLOAD_ORIGINS.iter().any(|o| upload_url.starts_with(o)) {
            return Err(GraphError::ForeignUpload);
        }
        let mut start = next_start(&session.next_expected_ranges)?.unwrap_or(0);
        loop {
            if start >= total {
                return Err(GraphError::Malformed);
            }
            let end = (start + RANGE_SIZE).min(total);
            let range = format!("bytes {start}-{}/{total}", end - 1);
            let headers = [("Content-Range", range.as_str())];
            let chunk = &file.bytes()[start..end];
            let reply = retrying(&[429], || {
                self.transport
                    .put_bytes(&upload_url, &headers, chunk, MAX_JSON)
            })
            .await?;
            match reply.status {
                201 => return Ok(()),
                200 => {
                    let progress: UploadSession =
                        serde_json::from_slice(&reply.body).map_err(|_| GraphError::Malformed)?;
                    match next_start(&progress.next_expected_ranges)? {
                        // Never back to a range already sent, so a confused session cannot
                        // keep neomail sending for ever.
                        Some(next) if next > start => start = next,
                        _ => return Err(GraphError::Malformed),
                    }
                }
                _ => return Err(status_error(&reply)),
            }
        }
    }

    /// Deletes the draft `id`; whether it is gone.
    async fn delete_draft(&self, id: &str) -> bool {
        let url = format!("{API}/me/messages/{}", encode_segment(id));
        let headers = [self.auth()];
        retrying(&[429], || self.transport.delete(&url, &headers, MAX_JSON))
            .await
            .is_ok_and(|reply| matches!(reply.status, 204 | 404))
    }

    /// Sends the draft `id`. As for sendMail, only 429 is retried; a 5xx or a transport error may
    /// come after Graph sent it. Any other refusal or lasting throttling sent nothing and keeps
    /// the draft.
    async fn send_draft(&self, id: &str) -> Result<(), GraphError> {
        let url = format!("{API}/me/messages/{}/send", encode_segment(id));
        let headers = [self.auth(), ACCEPT_JSON];
        let reply = retrying(&[429], || {
            self.transport.post_json(&url, &headers, b"", MAX_JSON)
        })
        .await
        .map_err(|e| match e {
            GraphError::Forbidden => GraphError::CannotSend,
            e @ GraphError::Transport(_) => GraphError::Unconfirmed(Box::new(e)),
            e @ GraphError::Throttled { .. } => kept(e),
            e => e,
        })?;
        match reply.status {
            202 => Ok(()),
            500.. => Err(GraphError::Unconfirmed(Box::new(status_error(&reply)))),
            _ => Err(kept(status_error(&reply))),
        }
    }
}

/// Whether a failed save of the draft may still have saved it: the answer was lost, was a 5xx,
/// or said 201 without an id neomail could read.
fn may_have_saved(e: &GraphError) -> bool {
    matches!(
        e,
        GraphError::Transport(_) | GraphError::Malformed | GraphError::Status { status: 500.., .. }
    )
}

/// `e`, from a send of the draft that sent nothing, which keeps the draft.
fn kept(e: GraphError) -> GraphError {
    GraphError::Unsent {
        source: Box::new(e),
        draft_left: DraftLeft::Yes,
    }
}

fn expect(reply: &Reply, status: u16) -> Result<(), GraphError> {
    if reply.status == status {
        Ok(())
    } else {
        Err(status_error(reply))
    }
}

/// The first byte of the first range in `nextExpectedRanges` (`"2097152"` or `"0-"` or
/// `"10-20"`), if any.
fn next_start(ranges: &[String]) -> Result<Option<usize>, GraphError> {
    let Some(first) = ranges.first() else {
        return Ok(None);
    };
    let start = first.split('-').next().unwrap_or_default();
    start
        .trim()
        .parse()
        .map(Some)
        .map_err(|_| GraphError::Malformed)
}
