//! The IMAP [`MailSource`]: TLS or STARTTLS over rustls, then read-only IMAP on INBOX; and the
//! one write, [`store_seen`], which sets or clears `\Seen` and never expunges.

mod tls;
mod watch;
#[cfg(test)]
mod wire_tests;

use std::future::Future;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use async_imap::types::{Fetch, Flag};
use async_imap::{Client, Session};
use futures_util::TryStreamExt;
use nm_core::{
    FetchedMessage, Flags, ListedMessage, MailSource, MailboxStatus, Uid, UidFlags, UidValidity,
    Written,
};
use rustls::ClientConfig;
use rustls::pki_types::ServerName;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

pub use tls::{CertPin, InvalidPin};
pub use watch::{IDLE_RENEW, WatchEvent, Watcher};

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

/// Where and how to connect. The credential is passed separately as a [`Login`] and never stored
/// here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImapConfig {
    pub host: String,
    pub port: u16,
    pub security: Security,
    pub username: String,
    pub cert_pin: Option<CertPin>,
}

/// How to log in: `LOGIN` with a password, or `AUTHENTICATE XOAUTH2` with an OAuth access token
/// (Gmail). The secret is borrowed for the login only; `Debug` never shows it.
#[derive(Clone, Copy)]
pub enum Login<'a> {
    Password(&'a str),
    XOAuth2 { access_token: &'a str },
}

impl std::fmt::Debug for Login<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Login::Password(_) => "Login::Password(<redacted>)",
            Login::XOAuth2 { .. } => "Login::XOAuth2(<redacted>)",
        })
    }
}

/// The SASL XOAUTH2 initial response before base64 (async-imap encodes it):
/// `user=ADDR^Aauth=Bearer TOKEN^A^A`, per
/// <https://developers.google.com/workspace/gmail/imap/xoauth2-protocol>.
pub fn xoauth2_response(user: &str, access_token: &str) -> String {
    format!("user={user}\x01auth=Bearer {access_token}\x01\x01")
}

/// Answers the server's first (empty) challenge with the XOAUTH2 string and any later one with an
/// empty response. A failed XOAUTH2 login sends a base64 JSON challenge, and SASL requires an
/// empty answer to it, after which the server ends the exchange with `NO`.
struct XOAuth2(Option<String>);

impl async_imap::Authenticator for XOAuth2 {
    type Response = String;

    fn process(&mut self, _challenge: &[u8]) -> String {
        self.0.take().unwrap_or_default()
    }
}

/// Logs in on a greeted connection with `login`. Shared by [`ImapSource::connect`] and the wire
/// tests.
async fn log_in<T: Transport>(
    client: Client<T>,
    user: &str,
    login: Login<'_>,
) -> Result<Session<T>, ImapError> {
    let result = match login {
        Login::Password(password) => step("login", client.login(user, password)).await?,
        Login::XOAuth2 { access_token } => {
            let auth = XOAuth2(Some(xoauth2_response(user, access_token)));
            // One bound for the whole exchange, so a server that keeps challenging cannot hang.
            step("login", client.authenticate("XOAUTH2", auth)).await?
        }
    };
    result.map_err(|(source, _)| ImapError::Login {
        user: user.to_owned(),
        source,
    })
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
    #[error("the server presented no certificate")]
    NoCertificate,
    #[error("the server did not report a valid UIDVALIDITY for INBOX")]
    NoUidValidity,
    #[error(
        "the server's INBOX is not the cached one (its UIDVALIDITY changed); nothing was changed, \
         run `neomail sync` first"
    )]
    UidValidityChanged,
    #[error("timed out during {0}")]
    Timeout(&'static str),
    #[error("the server closed the connection")]
    Closed,
    #[error("the server ended the session: {0}")]
    Bye(String),
    #[error("the server ended IDLE")]
    IdleEnded,
}

type Stream = TlsStream<TcpStream>;

/// What an IMAP session can run over: TLS in production, an in-memory pipe in the tests.
pub trait Transport:
    tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + std::fmt::Debug + Send
{
}

impl<T> Transport for T where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + std::fmt::Debug + Send
{
}

/// A logged-in IMAP session that implements [`MailSource`].
pub struct ImapSource<T: Transport = Stream> {
    session: Session<T>,
    /// Message count from the last EXAMINE; zero means there is nothing to list.
    exists: u32,
}

impl ImapSource {
    /// Connects, secures the connection, and logs in with `login`.
    pub async fn connect(config: &ImapConfig, login: Login<'_>) -> Result<Self, ImapError> {
        let tls = tls::client_config(config.cert_pin).map_err(ImapError::TlsConfig)?;
        let client = open(config, tls).await?;
        let session = log_in(client, &config.username, login).await?;
        Ok(Self { session, exists: 0 })
    }
}

/// Logs in, opens INBOX read-only with `EXAMINE` and logs out: the check `neomail account add`
/// runs before it writes the config. Returns the number of messages in INBOX.
pub async fn check_login(config: &ImapConfig, login: Login<'_>) -> Result<u32, ImapError> {
    let tls = tls::client_config(config.cert_pin).map_err(ImapError::TlsConfig)?;
    let client = open(config, tls).await?;
    check_on(client, &config.username, login).await
}

/// [`check_login`] on a greeted connection. Shared with the wire tests.
async fn check_on<T: Transport>(
    client: Client<T>,
    user: &str,
    login: Login<'_>,
) -> Result<u32, ImapError> {
    let session = log_in(client, user, login).await?;
    let mut source = ImapSource { session, exists: 0 };
    let status = source.examine_inbox().await;
    source.logout().await;
    Ok(status?.exists)
}

/// Sets (`seen`) or clears `\Seen` on `uids` in INBOX with `UID STORE +FLAGS` or `-FLAGS`
/// (RFC 3501 section 6.4.6; RFC 9051 section 6.4.6 is the same), after checking that INBOX's
/// UIDVALIDITY is still `uid_validity`: when it is not, nothing is stored. Each UID's flags are
/// the server's, from the FETCH responses STORE sends or, for a UID it sent none for, from a
/// `UID FETCH`; a UID in neither no longer exists ("a non-existent unique identifier is ignored
/// without any error message", RFC 3501 section 6.4.8) and is reported as gone.
///
/// Never sends EXPUNGE or CLOSE, which expunges `\Deleted` messages (RFC 3501 section 6.4.2):
/// the session ends with LOGOUT.
pub async fn store_seen(
    config: &ImapConfig,
    login: Login<'_>,
    uid_validity: UidValidity,
    uids: &[Uid],
    seen: bool,
) -> Result<Written<Uid>, ImapError> {
    let tls = tls::client_config(config.cert_pin).map_err(ImapError::TlsConfig)?;
    let client = open(config, tls).await?;
    let session = log_in(client, &config.username, login).await?;
    store_seen_on(session, uid_validity, uids, seen).await
}

/// [`store_seen`] on a logged-in session, which it logs out. Shared with the wire tests.
async fn store_seen_on<T: Transport>(
    session: Session<T>,
    uid_validity: UidValidity,
    uids: &[Uid],
    seen: bool,
) -> Result<Written<Uid>, ImapError> {
    let mut source = ImapSource { session, exists: 0 };
    let result = source.store_seen(uid_validity, uids, seen).await;
    source.logout().await;
    result
}

/// The SHA-256 of the certificate the server at `config` presents, for the user to compare and
/// pin (Proton Mail Bridge uses a self-signed one). The certificate is not trusted for anything:
/// the connection is closed after the handshake (and the greeting), without logging in.
/// `config.cert_pin` is ignored.
pub async fn server_certificate(config: &ImapConfig) -> Result<CertPin, ImapError> {
    let tls = tls::capture_config().map_err(ImapError::TlsConfig)?;
    let client = open(config, tls).await?;
    let stream = client.into_inner();
    stream
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|certs| certs.first())
        .map(|cert| CertPin::of(cert.as_ref()))
        .ok_or(ImapError::NoCertificate)
}

/// Connects to `config`'s server, secures the connection with `tls` (TLS from the first byte or
/// STARTTLS) and reads the greeting.
async fn open(config: &ImapConfig, tls: Arc<ClientConfig>) -> Result<Client<Stream>, ImapError> {
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
    Ok(match config.security {
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
    })
}

impl<T: Transport> ImapSource<T> {
    /// Ends the session politely. Errors are ignored: the work is already done.
    pub async fn logout(mut self) {
        let _ = step("logout", self.session.logout()).await;
    }

    /// SELECT INBOX read-write, check its UIDVALIDITY, then store. See [`store_seen`].
    async fn store_seen(
        &mut self,
        uid_validity: UidValidity,
        uids: &[Uid],
        seen: bool,
    ) -> Result<Written<Uid>, ImapError> {
        let op = "SELECT INBOX";
        let mailbox = step(op, self.session.select("INBOX"))
            .await?
            .map_err(|source| ImapError::Command { op, source })?;
        let found = mailbox
            .uid_validity
            .and_then(UidValidity::new)
            .ok_or(ImapError::NoUidValidity)?;
        if found != uid_validity {
            return Err(ImapError::UidValidityChanged);
        }
        if uids.is_empty() {
            return Ok(Written::default());
        }
        let op = "UID STORE";
        let query = if seen {
            "+FLAGS (\\Seen)"
        } else {
            "-FLAGS (\\Seen)"
        };
        let stream = step(op, self.session.uid_store(uid_set(uids), query))
            .await?
            .map_err(|source| ImapError::Command { op, source })?;
        let mut answered: Vec<UidFlags> = collect(op, stream)
            .await?
            .iter()
            .filter_map(|f| {
                let uid = f.uid.and_then(Uid::new).filter(|u| uids.contains(u))?;
                Some(UidFlags {
                    uid,
                    flags: flags(f),
                })
            })
            .collect();
        let unanswered: Vec<Uid> = uids
            .iter()
            .copied()
            .filter(|u| !answered.iter().any(|f| f.uid == *u))
            .collect();
        if !unanswered.is_empty() {
            let set = uid_set(&unanswered);
            let fetched = self.uid_flags("UID FETCH", Cmd::Uid(&set)).await?;
            answered.extend(fetched.into_iter().filter(|f| unanswered.contains(&f.uid)));
        }
        let mut written = Written::default();
        for &uid in uids {
            match answered.iter().find(|f| f.uid == uid) {
                Some(f) => written.done.push((uid, Some(f.flags.clone()))),
                None => written.gone.push(uid),
            }
        }
        Ok(written)
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

impl<T: Transport> MailSource for ImapSource<T> {
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
        let set = uid_set(uids);
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

/// `uids` as an IMAP sequence set: `1,5,9`.
fn uid_set(uids: &[Uid]) -> String {
    uids.iter()
        .map(Uid::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

/// Reads the server greeting, which must come before any command.
async fn greeting<T: Transport>(client: &mut Client<T>) -> Result<(), ImapError> {
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

/// Bounds one network step so a silent server cannot hang neomail.
async fn step<F: Future>(what: &'static str, fut: F) -> Result<F::Output, ImapError> {
    tokio::time::timeout(STEP_TIMEOUT, fut)
        .await
        .map_err(|_| ImapError::Timeout(what))
}
