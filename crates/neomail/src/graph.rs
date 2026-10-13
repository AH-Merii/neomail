//! Microsoft Graph wiring: `nm_graph`'s [`Transport`] over neomail's [`Https`], the read-only
//! check `account add outlook` runs before anything is saved, the sync of a Graph account, and
//! the writes: marking its messages read or unread, and moving them to the archive or trash.

use std::io;
use std::num::NonZeroU32;

use anyhow::{Result, anyhow};
use nm_core::{
    Cache, Folder, Moved, RemoteId, SyncError, SyncReport, Written, sync_inbox_by_id_with_progress,
};
use nm_graph::{GraphError, GraphSource, Reply, Transport, TransportError};

use crate::http::Https;
use crate::oauth::{MailGrant, Secret};
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

/// Moves the messages `ids` of a Graph account to the folder with role `to`. Like
/// [`set_read`], a Mail.Read sign-in fails before any request.
pub async fn move_to(
    account: &str,
    token: &AccessToken,
    ids: &[RemoteId],
    to: Folder,
) -> Result<Moved<RemoteId>> {
    may_write(account, token)?;
    GraphSource::new(Https::new()?, token.expose())
        .move_to(ids, to)
        .await
        .map_err(|e| explain_write(e, account))
}

/// Fails when `token`'s sign-in grants only Mail.Read: changing or moving mail needs
/// Mail.ReadWrite, which only a new sign-in can grant.
fn may_write(account: &str, token: &AccessToken) -> Result<()> {
    if token.grant != MailGrant::ReadWrite {
        return Err(anyhow!(
            "this account's sign-in lets neomail read mail but not change it; run `neomail \
             account login {account}` to let neomail change mail"
        ));
    }
    Ok(())
}

/// A Graph error from a write, with what to do next.
fn explain_write(e: GraphError, account: &str) -> anyhow::Error {
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
        // Nor move it, to either folder.
        for to in [Folder::Archive, Folder::Trash] {
            let id = RemoteId::new("AAMk1".to_owned()).unwrap();
            let e = move_to("outlook", &token, &[id], to).await.unwrap_err();
            assert_eq!(
                e.to_string(),
                "this account's sign-in lets neomail read mail but not change it; run `neomail \
                 account login outlook` to let neomail change mail"
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
