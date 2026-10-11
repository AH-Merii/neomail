//! The IMAP [`MailSource`]: TLS or STARTTLS over rustls, then read-only IMAP on INBOX.

mod tls;

use std::future::Future;
use std::io;
use std::time::Duration;

use async_imap::types::{Fetch, Flag};
use async_imap::{Client, Session};
use futures_util::TryStreamExt;
use nm_core::{
    FetchedMessage, Flags, ListedMessage, MailSource, MailboxStatus, Uid, UidFlags, UidValidity,
};
use rustls::pki_types::ServerName;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

pub use tls::{CertPin, InvalidPin};

/// How long one network step may take: connecting, the handshake, a command's reply, or the
/// next response of a FETCH. A FETCH as a whole has no bound, so a slow but working transfer of
/// many large messages is not cut off.
const STEP_TIMEOUT: Duration = Duration::from_secs(60);

/// How the connection is secured. There is no plaintext option.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Security {
    /// TLS from the first byte (usually port 993).
    Tls,
    /// Plain connection upgraded with STARTTLS before login (usually port 143).
    StartTls,
}

/// Where and how to connect. The password is passed separately and never stored here.
#[derive(Debug, Clone)]
pub struct ImapConfig {
    pub host: String,
    pub port: u16,
    pub security: Security,
    pub username: String,
    pub cert_pin: Option<CertPin>,
}

#[derive(Debug, thiserror::Error)]
pub enum ImapError {
    #[error("could not connect to {addr}")]
    Connect {
        addr: String,
        #[source]
        source: io::Error,
    },
    #[error("invalid TLS setup")]
    TlsConfig(#[source] rustls::Error),
    #[error("{0:?} is not a valid server name")]
    ServerName(String),
    #[error("TLS handshake with {host} failed")]
    Handshake {
        host: String,
        #[source]
        source: io::Error,
    },
    #[error("the server closed the connection without a greeting")]
    NoGreeting,
    #[error("the server refused STARTTLS")]
    StartTls(#[source] async_imap::error::Error),
    #[error("login as {user} failed")]
    Login {
        user: String,
        #[source]
        source: async_imap::error::Error,
    },
    #[error("IMAP {op} failed")]
    Command {
        op: &'static str,
        #[source]
        source: async_imap::error::Error,
    },
    #[error("the server did not report a valid UIDVALIDITY for INBOX")]
    NoUidValidity,
    #[error("timed out during {0}")]
    Timeout(&'static str),
}

type Stream = TlsStream<TcpStream>;

/// A logged-in IMAP session that implements [`MailSource`].
pub struct ImapSource {
    session: Session<Stream>,
    /// Message count from the last EXAMINE; zero means there is nothing to list.
    exists: u32,
}

impl ImapSource {
    /// Connects, secures the connection, and logs in.
    pub async fn connect(config: &ImapConfig, password: &str) -> Result<Self, ImapError> {
        let tls = tls::client_config(config.cert_pin).map_err(ImapError::TlsConfig)?;
        let connector = TlsConnector::from(tls);
        let server_name = ServerName::try_from(config.host.clone())
            .map_err(|_| ImapError::ServerName(config.host.clone()))?;
        let addr = format!("{}:{}", config.host, config.port);
        let tcp = step("connect", TcpStream::connect(&addr))
            .await?
            .map_err(|source| ImapError::Connect { addr, source })?;

        let handshake = |tcp| async {
            step(
                "the TLS handshake",
                connector.connect(server_name.clone(), tcp),
            )
            .await?
            .map_err(|source| ImapError::Handshake {
                host: config.host.clone(),
                source,
            })
        };
        let client = match config.security {
            Security::Tls => {
                let mut client = Client::new(handshake(tcp).await?);
                greeting(&mut client).await?;
                client
            }
            Security::StartTls => {
                let mut plain = Client::new(tcp);
                greeting(&mut plain).await?;
                step("STARTTLS", plain.run_command_and_check_ok("STARTTLS", None))
                    .await?
                    .map_err(ImapError::StartTls)?;
                Client::new(handshake(plain.into_inner()).await?)
            }
        };

        let session = step("login", client.login(&config.username, password))
            .await?
            .map_err(|(source, _)| ImapError::Login {
                user: config.username.clone(),
                source,
            })?;
        Ok(Self { session, exists: 0 })
    }

    /// Ends the session politely. Errors are ignored: the work is already done.
    pub async fn logout(mut self) {
        let _ = step("logout", self.session.logout()).await;
    }

    async fn uid_flags(
        &mut self,
        op: &'static str,
        cmd: Cmd<'_>,
    ) -> Result<Vec<UidFlags>, ImapError> {
        let fetches = self.run_fetch(op, cmd, "(UID FLAGS)").await?;
        Ok(fetches
            .iter()
            .filter_map(|f| {
                Some(UidFlags {
                    uid: f.uid.and_then(Uid::new)?,
                    flags: flags(f),
                })
            })
            .collect())
    }

    async fn run_fetch(
        &mut self,
        op: &'static str,
        cmd: Cmd<'_>,
        query: &str,
    ) -> Result<Vec<Fetch>, ImapError> {
        let err = |source| ImapError::Command { op, source };
        match cmd {
            Cmd::Seq(set) => {
                let stream = step(op, self.session.fetch(set, query))
                    .await?
                    .map_err(err)?;
                collect(op, stream).await
            }
            Cmd::Uid(set) => {
                let stream = step(op, self.session.uid_fetch(set, query))
                    .await?
                    .map_err(err)?;
                collect(op, stream).await
            }
        }
    }
}

/// Collects a FETCH's responses, bounding the wait for each one rather than the whole transfer.
async fn collect<S>(op: &'static str, stream: S) -> Result<Vec<Fetch>, ImapError>
where
    S: futures_util::Stream<Item = Result<Fetch, async_imap::error::Error>>,
{
    let mut stream = std::pin::pin!(stream);
    let mut out = Vec::new();
    while let Some(fetch) = step(op, stream.try_next())
        .await?
        .map_err(|source| ImapError::Command { op, source })?
    {
        out.push(fetch);
    }
    Ok(out)
}

enum Cmd<'a> {
    Seq(&'a str),
    Uid(&'a str),
}

impl MailSource for ImapSource {
    type Error = ImapError;

    async fn examine_inbox(&mut self) -> Result<MailboxStatus, ImapError> {
        let mailbox = step("EXAMINE INBOX", self.session.examine("INBOX"))
            .await?
            .map_err(|source| ImapError::Command {
                op: "EXAMINE INBOX",
                source,
            })?;
        self.exists = mailbox.exists;
        Ok(MailboxStatus {
            uid_validity: mailbox
                .uid_validity
                .and_then(UidValidity::new)
                .ok_or(ImapError::NoUidValidity)?,
            exists: mailbox.exists,
        })
    }

    async fn list_all(&mut self) -> Result<Vec<ListedMessage>, ImapError> {
        if self.exists == 0 {
            return Ok(Vec::new());
        }
        // Metadata only, no bodies: cheap even for a large inbox.
        let fetches = self
            .run_fetch("FETCH", Cmd::Seq("1:*"), "(UID FLAGS INTERNALDATE)")
            .await?;
        Ok(fetches
            .iter()
            .filter_map(|f| {
                Some(ListedMessage {
                    uid: f.uid.and_then(Uid::new)?,
                    flags: flags(f),
                    received: f.internal_date().map(|d| d.with_timezone(&chrono::Utc)),
                })
            })
            .collect())
    }

    async fn flags_from(&mut self, from: Uid) -> Result<Vec<UidFlags>, ImapError> {
        let set = format!("{from}:*");
        self.uid_flags("UID FETCH", Cmd::Uid(&set)).await
    }

    async fn fetch(&mut self, uids: &[Uid]) -> Result<Vec<FetchedMessage>, ImapError> {
        if uids.is_empty() {
            return Ok(Vec::new());
        }
        let set = uids
            .iter()
            .map(Uid::to_string)
            .collect::<Vec<_>>()
            .join(",");
        // BODY.PEEK[] leaves \Seen alone; a plain BODY[] would mark the message read.
        let fetches = self
            .run_fetch(
                "UID FETCH",
                Cmd::Uid(&set),
                "(UID FLAGS INTERNALDATE BODY.PEEK[])",
            )
            .await?;
        Ok(fetches
            .iter()
            .filter_map(|f| {
                Some(FetchedMessage {
                    uid: f.uid.and_then(Uid::new)?,
                    flags: flags(f),
                    received: f.internal_date().map(|d| d.with_timezone(&chrono::Utc)),
                    raw: f.body()?.to_vec(),
                })
            })
            .collect())
    }
}

/// Reads the server greeting, which must come before any command.
async fn greeting<T>(client: &mut Client<T>) -> Result<(), ImapError>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    match step("the greeting", client.read_response()).await? {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(ImapError::NoGreeting),
        Err(source) => Err(ImapError::Command {
            op: "greeting",
            source: source.into(),
        }),
    }
}

fn flags(fetch: &Fetch) -> Flags {
    Flags::new(fetch.flags().filter_map(|f| match f {
        Flag::Seen => Some("\\Seen".to_owned()),
        Flag::Answered => Some("\\Answered".to_owned()),
        Flag::Flagged => Some("\\Flagged".to_owned()),
        Flag::Deleted => Some("\\Deleted".to_owned()),
        Flag::Draft => Some("\\Draft".to_owned()),
        Flag::Custom(c) => Some(c.into_owned()),
        Flag::Recent | Flag::MayCreate => None,
    }))
}

/// Bounds one network step so a silent server cannot hang nmail.
async fn step<F: Future>(what: &'static str, fut: F) -> Result<F::Output, ImapError> {
    tokio::time::timeout(STEP_TIMEOUT, fut)
        .await
        .map_err(|_| ImapError::Timeout(what))
}
