//! Microsoft Graph wiring: `nm_graph`'s [`Transport`] over neomail's [`Https`], the read-only
//! check `account add outlook` runs before anything is saved, and the sync of a Graph account.

use std::io;
use std::num::NonZeroU32;

use anyhow::{Result, anyhow};
use nm_core::{Cache, SyncError, SyncReport, sync_inbox_by_id_with_progress};
use nm_graph::{GraphError, GraphSource, Reply, Transport, TransportError};

use crate::http::Https;
use crate::oauth::Secret;
use crate::output;

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
/// shown it grants Mail.Read, so a 403 here is the mailbox, not the grant.
fn check_failed(e: GraphError) -> anyhow::Error {
    const CHECK: &str = "the read-only check of the INBOX through Microsoft Graph failed";
    match e {
        GraphError::Unauthorized => {
            anyhow!("{CHECK} ({e}); nothing was saved; run `neomail account add outlook` again")
        }
        GraphError::Forbidden => anyhow!(
            "{CHECK} ({e}) although the sign-in granted Mail.Read; nothing was saved. The \
             account may have no Outlook mailbox or licence, or a policy of its organisation \
             blocks the app"
        ),
        other => anyhow::Error::new(other).context(format!("{CHECK}; nothing was saved")),
    }
}

/// Syncs a Graph account's INBOX with `access_token`.
pub async fn sync(
    account: &str,
    access_token: &Secret,
    cache: &mut Cache,
    limit: NonZeroU32,
    progress: &mut output::ProgressLine,
) -> Result<SyncReport> {
    let mut source = GraphSource::new(Https::new()?, access_token.expose());
    sync_inbox_by_id_with_progress(&mut source, cache, account, limit, |p| {
        progress.update(&mut io::stderr(), account, p);
    })
    .await
    .map_err(|e| match e {
        SyncError::Source(e) => explain(e, account),
        other => anyhow::Error::new(other),
    })
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
    fn a_failed_check_says_to_add_the_account_again() {
        assert_eq!(
            check_failed(GraphError::Unauthorized).to_string(),
            "the read-only check of the INBOX through Microsoft Graph failed (access token \
             refused); nothing was saved; run `neomail account add outlook` again"
        );
        assert_eq!(
            check_failed(GraphError::Forbidden).to_string(),
            "the read-only check of the INBOX through Microsoft Graph failed (Microsoft Graph \
             refused access to the mailbox (HTTP 403)) although the sign-in granted Mail.Read; \
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
