//! The push watcher: one extra IMAP connection per account whose only job is to say "this account
//! changed". It logs in, opens INBOX read-only with EXAMINE and runs IDLE (RFC 2177,
//! <https://www.rfc-editor.org/rfc/rfc2177>). Like the sync, it never sends a command that can
//! change the server: its commands are LOGIN or AUTHENTICATE, CAPABILITY, EXAMINE, IDLE, DONE and
//! LOGOUT.

use std::time::Duration;

use async_imap::imap_proto::{MailboxDatum, Response, Status};
use async_imap::types::UnsolicitedResponse;
use async_imap::{Client, Session};
use futures_util::StreamExt;

use super::{ImapConfig, ImapError, Login, Stream, Transport, log_in, open, step, tls};

/// How long one IDLE runs before the watcher ends it with DONE and issues it again on the same
/// connection. RFC 2177, section 3: "clients using IDLE are advised to terminate the IDLE and
/// re-issue it at least every 29 minutes". It also bounds how long a connection that died without
/// a word (a suspend, a lost network) can hold a watcher: DONE's reply has the same one-minute
/// deadline as every other reply, so such a watcher ends at most 29 minutes and one step later.
pub const IDLE_RENEW: Duration = Duration::from_secs(29 * 60);

/// What a running [`Watcher`] reports through its callback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchEvent {
    /// The first IDLE was accepted. Changes from here on arrive as [`WatchEvent::Changed`]; mail
    /// that arrived between the account's last sync and this moment does not, so the daemon syncs
    /// the account once on this event.
    Idling,
    /// INBOX changed: the server sent an untagged EXISTS, EXPUNGE or FETCH.
    Changed,
}

/// A logged-in connection with INBOX examined, on a server that has IDLE. It holds no secret: the
/// [`Login`] is borrowed for the login only.
pub struct Watcher<T: Transport = Stream> {
    session: Session<T>,
}

impl Watcher {
    /// Connects as the sync does (TLS or STARTTLS, the certificate pin), logs in with `login`, and
    /// opens INBOX with EXAMINE. `Ok(None)` means the server has no IDLE in its CAPABILITY: it was
    /// logged out, and the account keeps polling.
    pub async fn connect(config: &ImapConfig, login: Login<'_>) -> Result<Option<Self>, ImapError> {
        let tls = tls::client_config(config.cert_pin).map_err(ImapError::TlsConfig)?;
        let client = open(config, tls).await?;
        Watcher::start(client, &config.username, login).await
    }
}

impl<T: Transport> Watcher<T> {
    /// [`Watcher::connect`] on a greeted connection. Shared with the wire tests.
    pub(crate) async fn start(
        client: Client<T>,
        user: &str,
        login: Login<'_>,
    ) -> Result<Option<Self>, ImapError> {
        let mut session = log_in(client, user, login).await?;
        // Asked after the login, since a server may announce more capabilities then.
        let capabilities = step("CAPABILITY", session.capabilities())
            .await?
            .map_err(|source| ImapError::Command {
                op: "CAPABILITY",
                source,
            })?;
        if !capabilities.has_str("IDLE") {
            let _ = step("logout", session.logout()).await;
            return Ok(None);
        }
        step("EXAMINE INBOX", session.examine("INBOX"))
            .await?
            .map_err(|source| ImapError::Command {
                op: "EXAMINE INBOX",
                source,
            })?;
        // What came before the first IDLE is covered by `WatchEvent::Idling`.
        while session.unsolicited_responses.try_recv().is_ok() {}
        Ok(Some(Self { session }))
    }

    /// Idles on INBOX until the connection ends, calling `on` with [`WatchEvent::Idling`] once the
    /// first IDLE is accepted and with [`WatchEvent::Changed`] for every untagged EXISTS, EXPUNGE
    /// or FETCH. Every [`IDLE_RENEW`] it sends DONE and IDLE again on the same connection.
    ///
    /// It never returns while the connection works; the error says why it ended: the server's BYE
    /// or end of stream, a refused or ended IDLE, or a deadline. A stop drops the future, which
    /// closes the connection at once.
    pub async fn run(self, mut on: impl FnMut(WatchEvent)) -> ImapError {
        let mut session = self.session;
        let unsolicited = session.unsolicited_responses.clone();
        let changes = || drain_changes(|| unsolicited.try_recv().ok());
        let mut first = true;
        loop {
            let mut idle = session.idle();
            match step("IDLE", idle.init()).await {
                Ok(Ok(())) => {}
                Ok(Err(source)) => return ImapError::Command { op: "IDLE", source },
                Err(timeout) => return timeout,
            }
            if std::mem::take(&mut first) {
                on(WatchEvent::Idling);
            }
            // Sent outside IDLE, from the last DONE to this IDLE's continuation: async-imap queues
            // such untagged responses (`Handle::done`, `Handle::init`).
            if changes() {
                on(WatchEvent::Changed);
            }
            let renew = tokio::time::Instant::now() + IDLE_RENEW;
            loop {
                let Ok(next) = tokio::time::timeout_at(renew, idle.next()).await else {
                    break;
                };
                let response = match next {
                    None => return ImapError::Closed,
                    Some(Err(source)) => {
                        return ImapError::Command {
                            op: "IDLE",
                            source: source.into(),
                        };
                    }
                    Some(Ok(response)) => response,
                };
                match response.parsed() {
                    Response::Data {
                        status: Status::Bye,
                        outcome,
                        ..
                    } => {
                        let text = outcome
                            .information
                            .as_deref()
                            .unwrap_or_default()
                            .to_owned();
                        return ImapError::Bye(text);
                    }
                    // A tagged reply: the server ended the IDLE itself.
                    Response::Done { .. } => return ImapError::IdleEnded,
                    parsed if is_change(parsed) => on(WatchEvent::Changed),
                    _ => {}
                }
            }
            session = match step("DONE", idle.done()).await {
                Ok(Ok(session)) => session,
                Ok(Err(source)) => return ImapError::Command { op: "DONE", source },
                Err(timeout) => return timeout,
            };
        }
    }
}

/// Whether one of the responses `next` yields says that INBOX changed. Reads until `next` returns
/// `None`.
fn drain_changes(mut next: impl FnMut() -> Option<UnsolicitedResponse>) -> bool {
    let mut changed = false;
    while let Some(response) = next() {
        changed |= match response {
            UnsolicitedResponse::Exists(_) | UnsolicitedResponse::Expunge(_) => true,
            UnsolicitedResponse::Other(data) => is_change(data.parsed()),
            _ => false,
        };
    }
    changed
}

/// An untagged EXISTS, EXPUNGE or FETCH: RFC 2177, section 3, lets the server send "untagged
/// EXISTS, EXPUNGE, and other messages at any time" while idling.
fn is_change(response: &Response<'_>) -> bool {
    matches!(
        response,
        Response::MailboxData(MailboxDatum::Exists(_)) | Response::Expunge(_) | Response::Fetch(..)
    )
}
