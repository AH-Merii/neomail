//! Microsoft Graph wiring: `nm_graph`'s [`Transport`] over neomail's [`Https`], the read-only
//! check `account add outlook` runs before anything is saved, the sync of a Graph account, and
//! the writes: marking its messages read or unread, and moving them to the archive, the trash or
//! a folder the user names; the listing of its folders; and sending mail with sendMail.

use std::io;
use std::num::NonZeroU32;

use anyhow::{Result, anyhow};
use nm_core::compose::Outgoing;
use nm_core::{
    Cache, Destination, ListedFolder, Moved, RemoteId, SyncError, SyncReport, Written,
    sync_inbox_by_id_with_progress,
};
use nm_graph::{GraphError, GraphSource, Reply, Transport, TransportError};

use crate::http::Https;
use crate::oauth::Secret;
use crate::output;
use crate::sync::AccessToken;

impl Transport for Https {
    async fn get(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        max_body: usize,
    ) -> Result<Reply, TransportError> {
        let r = Https::get(self, url, headers, max_body).await?;
        Ok(Reply {
            status: r.status,
            retry_after: r.retry_after,
            body: r.body,
        })
    }

    async fn patch_json(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        body: &[u8],
        max_body: usize,
    ) -> Result<Reply, TransportError> {
        let r = Https::patch_json(self, url, headers, body, max_body).await?;
        Ok(Reply {
            status: r.status,
            retry_after: r.retry_after,
            body: r.body,
        })
    }

    async fn post_json(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        body: &[u8],
        max_body: usize,
    ) -> Result<Reply, TransportError> {
        let r = Https::post_json(self, url, headers, body, max_body).await?;
        Ok(Reply {
            status: r.status,
            retry_after: r.retry_after,
            body: r.body,
        })
    }

    async fn post_text(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        body: &[u8],
        max_body: usize,
    ) -> Result<Reply, TransportError> {
        let r = Https::post_text(self, url, headers, body, max_body).await?;
        Ok(Reply {
            status: r.status,
            retry_after: r.retry_after,
            body: r.body,
        })
    }
}

/// The INBOX's message count, read-only: shows the sign-in's grant works and the mailbox is
/// there. `account add outlook` runs it before anything is saved.
pub async fn check(access_token: &Secret) -> Result<u64> {
    GraphSource::new(Https::new()?, access_token.expose())
        .inbox_count()
        .await
        .map_err(check_failed)
}

/// A failed check with what to do next. Nothing is saved yet, and the sign-in has already
/// shown it grants mail access (Mail.ReadWrite or Mail.Read), so a 403 here is the mailbox, not
/// the grant.
fn check_failed(e: GraphError) -> anyhow::Error {
    const CHECK: &str = "the read-only check of the INBOX through Microsoft Graph failed";
    match e {
        GraphError::Unauthorized => {
            anyhow!("{CHECK} ({e}); nothing was saved; run `neomail account add outlook` again")
        }
        GraphError::Forbidden => anyhow!(
            "{CHECK}: {e}, although the sign-in granted mail access; nothing was saved. \
             The account may have no Outlook mailbox or licence, or a policy of its \
             organisation blocks the app"
        ),
        other => anyhow::Error::new(other).context(format!("{CHECK}; nothing was saved")),
    }
}

/// Syncs a Graph account's INBOX with `access_token`, filling `report` as it goes. Once the
/// cache holds a delta link, the sync asks Graph only for what changed since it
/// (`nm_core::sync_inbox_by_id`).
pub async fn sync(
    account: &str,
    access_token: &Secret,
    cache: &mut Cache,
    limit: NonZeroU32,
    report: &mut SyncReport,
    progress: &mut output::ProgressLine,
) -> Result<()> {
    let mut source = GraphSource::new(Https::new()?, access_token.expose());
    sync_inbox_by_id_with_progress(&mut source, cache, account, limit, report, |p| {
        progress.update(&mut io::stderr(), account, p);
    })
    .await
    .map_err(|e| match e {
        SyncError::Source(e) => explain(e, account),
        other => anyhow::Error::new(other),
    })
}

/// Sets the read state of the messages `ids` of a Graph account. A sign-in that grants only
/// Mail.Read fails before any request: changing mail needs Mail.ReadWrite, which only a new
/// sign-in can grant.
pub async fn set_read(
    account: &str,
    token: &AccessToken,
    ids: &[RemoteId],
    read: bool,
) -> Result<Written<RemoteId>> {
    may_write(account, token)?;
    GraphSource::new(Https::new()?, token.expose())
        .set_read(ids, read)
        .await
        .map_err(|e| explain_write(e, account))
}

/// Moves the messages `ids` of a Graph account to the folder `to`: a role, or a path of display
/// names as [`folders`] prints it. Like [`set_read`], a Mail.Read sign-in fails before any
/// request.
pub async fn move_to(
    account: &str,
    token: &AccessToken,
    ids: &[RemoteId],
    to: &Destination,
) -> Result<Moved<RemoteId>> {
    may_write(account, token)?;
    GraphSource::new(Https::new()?, token.expose())
        .move_to(ids, to)
        .await
        .map_err(|e| explain_write(e, account))
}

/// The folders of a Graph account, read-only: any mail grant can list them.
pub async fn folders(account: &str, token: &AccessToken) -> Result<Vec<ListedFolder>> {
    GraphSource::new(Https::new()?, token.expose())
        .folders()
        .await
        .map_err(|e| match e {
            GraphError::Unauthorized => anyhow!("access token refused; run the command again"),
            e => explain(e, account),
        })
}

/// Sends `message` from a Graph account with sendMail; Graph keeps the copy in Sent Items. The
/// grant is checked first: a sign-in from before neomail asked for Mail.Send is told to sign in
/// again, and nothing is sent.
pub async fn send(account: &str, token: &AccessToken, message: &Outgoing) -> Result<()> {
    if !token.grant.can_send() {
        return Err(anyhow!(
            "this account's sign-in does not let neomail send mail; nothing was sent; run \
             `neomail account login {account}` once to let neomail send"
        ));
    }
    GraphSource::new(Https::new()?, token.expose())
        .send(message)
        .await
        .map_err(|e| match e {
            GraphError::Unauthorized => {
                anyhow!("access token refused; nothing was sent; run the command again")
            }
            GraphError::CannotSend => anyhow!(
                "{e}; run `neomail account login {account}` and accept the permission to send \
                 mail as you; if it persists, a policy of the account's organisation may block it"
            ),
            e => anyhow::Error::new(e),
        })
}

/// Fails when `token`'s sign-in grants only Mail.Read: changing or moving mail needs
/// Mail.ReadWrite, which only a new sign-in can grant.
fn may_write(account: &str, token: &AccessToken) -> Result<()> {
    if !token.grant.can_write() {
        return Err(anyhow!(
            "this account's sign-in lets neomail read mail but not change it; run `neomail \
             account login {account}` to let neomail change mail"
        ));
    }
    Ok(())
}

/// A Graph error from a write, with what to do next.
pub(crate) fn explain_write(e: GraphError, account: &str) -> anyhow::Error {
    match e {
        GraphError::Unauthorized => anyhow!("access token refused; run the command again"),
        GraphError::Forbidden => anyhow!(
            "{e}; run `neomail account login {account}` and accept the permission to read and \
             change your email; if it persists, a policy of the account's organisation may block \
             the app"
        ),
        GraphError::NoFolder(_) => {
            anyhow!("{e}, so nothing was moved; use Outlook on the web for these messages")
        }
        e @ (GraphError::NoSuchFolder(_) | GraphError::SearchFolder(_)) => {
            crate::folders::point_at_folder_list(e, account)
        }
        other => anyhow::Error::new(other),
    }
}

/// A Graph error with what to do next.
fn explain(e: GraphError, account: &str) -> anyhow::Error {
    match e {
        GraphError::Unauthorized => anyhow!("access token refused; run sync again"),
        GraphError::Forbidden => anyhow!(
            "{e}; run `neomail account login {account}` and accept the permission to read your \
             email; if it persists, the account may have no Outlook mailbox or a policy blocks \
             the app"
        ),
        other => anyhow::Error::new(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oauth::MailGrant;

    #[test]
    fn a_failed_check_says_nothing_was_saved_and_what_to_do() {
        assert_eq!(
            check_failed(GraphError::Unauthorized).to_string(),
            "the read-only check of the INBOX through Microsoft Graph failed (access token \
             refused); nothing was saved; run `neomail account add outlook` again"
        );
        assert_eq!(
            check_failed(GraphError::Forbidden).to_string(),
            "the read-only check of the INBOX through Microsoft Graph failed: Microsoft Graph \
             refused access to the mailbox (HTTP 403), although the sign-in granted mail access; \
             nothing was saved. The account may have no Outlook mailbox or licence, or a policy \
             of its organisation blocks the app"
        );
        let e = check_failed(GraphError::Malformed);
        assert_eq!(
            format!("{e:#}"),
            "the read-only check of the INBOX through Microsoft Graph failed; nothing was saved: \
             Microsoft Graph's answer is not the expected JSON"
        );
        // Any other error keeps its cause, such as why Graph could not be reached.
        let cause: nm_graph::TransportError = Box::new(io::Error::other("tls handshake failed"));
        let e = check_failed(GraphError::Transport(cause));
        assert!(
            format!("{e:#}")
                .ends_with("nothing was saved: cannot reach Microsoft Graph: tls handshake failed"),
            "{e:#}"
        );
    }

    #[tokio::test]
    async fn a_read_only_sign_in_cannot_change_mail_and_sends_nothing() {
        // `Https` is never built: the grant is checked first, so no request can be made.
        let token = AccessToken {
            secret: Secret::new("not-a-real-token".into()),
            grant: MailGrant::Read,
        };
        let id = RemoteId::new("AAMk1".to_owned()).unwrap();
        let e = set_read("outlook", &token, &[id], true).await.unwrap_err();
        assert_eq!(
            e.to_string(),
            "this account's sign-in lets neomail read mail but not change it; run `neomail \
             account login outlook` to let neomail change mail"
        );
        // Nor move it, to either role or to a named folder.
        let receipts = Destination::Name(nm_core::FolderName::new("Inbox/Receipts").unwrap());
        for to in [
            nm_core::Folder::Archive.into(),
            nm_core::Folder::Trash.into(),
            receipts,
        ] {
            let id = RemoteId::new("AAMk1".to_owned()).unwrap();
            let e = move_to("outlook", &token, &[id], &to).await.unwrap_err();
            assert_eq!(
                e.to_string(),
                "this account's sign-in lets neomail read mail but not change it; run `neomail \
                 account login outlook` to let neomail change mail"
            );
        }
    }

    #[tokio::test]
    async fn a_sign_in_without_mail_send_is_told_to_log_in_and_sends_nothing() {
        // `Https` is never built: the grant is checked first, so no request can be made.
        let message = Outgoing {
            mime: b"From: me@example.com\r\n\r\nx\r\n".to_vec(),
            envelope_from: "me@example.com".to_owned(),
            envelope_to: vec!["ann@example.org".to_owned()],
            bcc: Vec::new(),
            message_id: "id@example.com".to_owned(),
        };
        for grant in [MailGrant::Read, MailGrant::ReadWrite] {
            let token = AccessToken {
                secret: Secret::new("not-a-real-token".into()),
                grant,
            };
            let e = send("outlook", &token, &message).await.unwrap_err();
            assert_eq!(
                e.to_string(),
                "this account's sign-in does not let neomail send mail; nothing was sent; run \
                 `neomail account login outlook` once to let neomail send"
            );
        }
    }

    #[test]
    fn a_refused_write_says_what_to_do() {
        assert_eq!(
            explain_write(GraphError::Forbidden, "outlook").to_string(),
            "Microsoft Graph refused access to the mailbox (HTTP 403); run `neomail account \
             login outlook` and accept the permission to read and change your email; if it \
             persists, a policy of the account's organisation may block the app"
        );
        assert_eq!(
            explain_write(GraphError::NoFolder("archive"), "outlook").to_string(),
            "the mailbox has no \"archive\" folder in Microsoft Graph, so nothing was moved; use \
             Outlook on the web for these messages"
        );
        assert_eq!(
            explain_write(GraphError::Unauthorized, "outlook").to_string(),
            "access token refused; run the command again"
        );
    }

    #[test]
    fn refused_tokens_and_grants_say_what_to_do() {
        assert_eq!(
            explain(GraphError::Unauthorized, "outlook").to_string(),
            "access token refused; run sync again"
        );
        assert_eq!(
            explain(GraphError::Forbidden, "outlook").to_string(),
            "Microsoft Graph refused access to the mailbox (HTTP 403); run `neomail account \
             login outlook` and accept the permission to read your email; if it persists, the \
             account may have no Outlook mailbox or a policy blocks the app"
        );
        assert_eq!(
            explain(GraphError::Throttled { status: 429 }, "outlook").to_string(),
            "Microsoft Graph is throttling requests (HTTP 429) after 5 tries; try again later"
        );
    }
}
