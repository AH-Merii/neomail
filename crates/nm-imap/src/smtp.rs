//! SMTP submission (RFC 6409): sends one built message ([`Outgoing`]) through the account's
//! server, over the same TLS setup and certificate pinning as IMAP. TLS is always on: implicit
//! TLS (RFC 8314 section 3.3), or STARTTLS (RFC 3207) before anything else is said, and a server
//! that does not offer STARTTLS is refused rather than spoken to in plaintext.
//!
//! The session is EHLO, AUTH (PLAIN, RFC 4616, or Google's XOAUTH2), MAIL, one RCPT per
//! recipient, DATA and QUIT (RFC 5321 section 3.3). When the server refuses any recipient, the
//! session sends RSET and QUIT instead of DATA, so nobody gets the message: a script that retries
//! after the error must not deliver it twice to the recipients that went through.
//!
//! The credential is used for AUTH only; no error or `Debug` output holds it.

use std::fmt;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use nm_core::compose::Outgoing;
use rustls::ClientConfig;
use rustls::pki_types::ServerName;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

use crate::tls;
use crate::{CertPin, Login, Security, xoauth2_response};

/// How long one command's reply may take.
const STEP_TIMEOUT: Duration = Duration::from_secs(60);

/// How long the reply to the end of DATA may take: "10 minutes" (RFC 5321 section 4.5.3.2.6).
const DATA_TIMEOUT: Duration = Duration::from_secs(600);

/// The longest reply line read; RFC 5321 section 4.5.3.1.5 allows 512 octets.
const MAX_LINE: u64 = 4096;

/// The most lines one reply may have, so a hostile server cannot make neomail read forever.
const MAX_REPLY_LINES: usize = 100;

/// What EHLO names the client as: an address literal (RFC 5321 section 4.1.1.1), so the
/// machine's host name never leaves it.
const EHLO_NAME: &str = "[127.0.0.1]";

/// Where and how to submit. The credential is passed separately as a [`Login`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub security: Security,
    /// The user name AUTH sends.
    pub username: String,
    pub cert_pin: Option<CertPin>,
}

/// A server reply: its code and the text of each line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub code: u16,
    pub lines: Vec<String>,
}

impl fmt::Display for Reply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.code, self.lines.join(" "))
    }
}

impl Reply {
    fn ok(&self) -> bool {
        (200..300).contains(&self.code)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SmtpError {
    #[error("could not connect to {addr}")]
    Connect {
        addr: String,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid TLS setup")]
    TlsConfig(#[source] rustls::Error),
    #[error("{0:?} is not a valid server name")]
    ServerName(String),
    #[error("TLS handshake with {host} failed")]
    Handshake {
        host: String,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "the server does not offer STARTTLS, and neomail never sends mail without TLS; nothing \
         was sent"
    )]
    NoStartTls,
    #[error("the server sent data before the TLS handshake; nothing was sent")]
    InjectedData,
    #[error("authentication as {user} failed: {reply}; nothing was sent")]
    Auth { user: String, reply: Reply },
    #[error(
        "the server refused {}; nothing was sent to anyone",
        refused.iter().map(|(a, r)| format!("{a} ({r})")).collect::<Vec<_>>().join(", ")
    )]
    Refused { refused: Vec<(String, Reply)> },
    #[error("the server refused {step}: {reply}; nothing was sent")]
    Rejected { step: &'static str, reply: Reply },
    #[error("{0:?} cannot be used in an SMTP command; nothing was sent")]
    BadAddress(String),
    #[error("the server sent a malformed reply during {0}")]
    Malformed(&'static str),
    #[error("timed out during {0}")]
    Timeout(&'static str),
    #[error("the server closed the connection during {0}")]
    Closed(&'static str),
    #[error("the connection failed during {step}")]
    Io {
        step: &'static str,
        #[source]
        source: std::io::Error,
    },
    /// The whole message, its final `.` included, was written, but no reply to it came: the
    /// server may have accepted it (RFC 1047), so sending it again may deliver it twice.
    #[error(
        "the server did not confirm the message, so it may have been sent; check Sent before \
         sending it again"
    )]
    Unconfirmed(#[source] Box<SmtpError>),
}

/// Sends `message` to its envelope recipients through the server at `config`, logged in with
/// `login`. Ok means the server accepted the message for every recipient.
pub async fn submit(
    config: &SmtpConfig,
    login: Login<'_>,
    message: &Outgoing,
) -> Result<(), SmtpError> {
    let tls = tls::client_config(config.cert_pin).map_err(SmtpError::TlsConfig)?;
    let mut conn = open(config, tls).await?;
    let result = send_on(&mut conn, &config.username, login, message).await;
    conn.quit().await;
    result
}

/// Connects to `config`'s server, secures the connection with `tls` (TLS from the first byte or
/// STARTTLS) and greets it with EHLO.
async fn open(
    config: &SmtpConfig,
    tls: Arc<ClientConfig>,
) -> Result<Conn<tokio_rustls::client::TlsStream<TcpStream>>, SmtpError> {
    let connector = TlsConnector::from(tls);
    let server_name = ServerName::try_from(config.host.clone())
        .map_err(|_| SmtpError::ServerName(config.host.clone()))?;
    let addr = format!("{}:{}", config.host, config.port);
    let tcp = timed("connect", STEP_TIMEOUT, TcpStream::connect(&addr))
        .await?
        .map_err(|source| SmtpError::Connect { addr, source })?;
    let handshake = |tcp| async {
        timed(
            "the TLS handshake",
            STEP_TIMEOUT,
            connector.connect(server_name.clone(), tcp),
        )
        .await?
        .map_err(|source| SmtpError::Handshake {
            host: config.host.clone(),
            source,
        })
    };
    match config.security {
        Security::Tls => {
            let mut conn = Conn::new(handshake(tcp).await?);
            hello(&mut conn).await?;
            Ok(conn)
        }
        Security::StartTls => {
            let mut plain = Conn::new(tcp);
            let tcp = start_tls(&mut plain).await.map(|()| plain.into_inner())?;
            let mut conn = Conn::new(handshake(tcp).await?);
            // "The client SHOULD send an EHLO command as the first command after a successful
            // TLS negotiation" (RFC 3207 section 4.2): what the server offers may change.
            ehlo(&mut conn).await?;
            Ok(conn)
        }
    }
}

/// On a plaintext connection: the greeting, EHLO and STARTTLS, which the server must offer.
/// Nothing else is said before TLS, and data the server sent ahead of the handshake (a
/// command-injection attempt) ends the session.
async fn start_tls<S: AsyncRead + AsyncWrite + Unpin>(
    plain: &mut Conn<S>,
) -> Result<(), SmtpError> {
    let extensions = hello(plain).await?;
    if !extensions.iter().any(|e| e == "STARTTLS") {
        return Err(SmtpError::NoStartTls);
    }
    let reply = plain.command("STARTTLS", "STARTTLS").await?;
    if reply.code != 220 {
        return Err(SmtpError::Rejected {
            step: "STARTTLS",
            reply,
        });
    }
    if !plain.stream.buffer().is_empty() {
        return Err(SmtpError::InjectedData);
    }
    Ok(())
}

/// Reads the greeting and sends EHLO. Returns the extensions the server offers.
async fn hello<S: AsyncRead + AsyncWrite + Unpin>(
    conn: &mut Conn<S>,
) -> Result<Vec<String>, SmtpError> {
    let step = "the greeting";
    let reply = conn.reply(step, STEP_TIMEOUT).await?;
    if reply.code != 220 {
        return Err(SmtpError::Rejected { step, reply });
    }
    ehlo(conn).await
}

/// Sends EHLO and returns each extension's keyword, in upper case (RFC 5321 section 4.1.1.1).
async fn ehlo<S: AsyncRead + AsyncWrite + Unpin>(
    conn: &mut Conn<S>,
) -> Result<Vec<String>, SmtpError> {
    let step = "EHLO";
    let reply = conn.command(&format!("EHLO {EHLO_NAME}"), step).await?;
    if reply.code != 250 {
        return Err(SmtpError::Rejected { step, reply });
    }
    Ok(reply
        .lines
        .iter()
        .skip(1)
        .filter_map(|l| l.split_whitespace().next())
        .map(str::to_ascii_uppercase)
        .collect())
}

/// AUTH, then the envelope and the message, on a greeted connection. Shared with the tests.
async fn send_on<S: AsyncRead + AsyncWrite + Unpin>(
    conn: &mut Conn<S>,
    user: &str,
    login: Login<'_>,
    message: &Outgoing,
) -> Result<(), SmtpError> {
    for address in std::iter::once(&message.envelope_from).chain(&message.envelope_to) {
        if address.is_empty()
            || address
                .chars()
                .any(|c| c.is_control() || c.is_whitespace() || c == '<' || c == '>')
        {
            return Err(SmtpError::BadAddress(address.clone()));
        }
    }
    authenticate(conn, user, login).await?;

    let step = "MAIL FROM";
    let from = format!("MAIL FROM:<{}>", message.envelope_from);
    let reply = conn.command(&from, step).await?;
    if !reply.ok() {
        return Err(SmtpError::Rejected { step, reply });
    }
    let mut refused = Vec::new();
    for to in &message.envelope_to {
        let reply = conn.command(&format!("RCPT TO:<{to}>"), "RCPT TO").await?;
        if !reply.ok() {
            refused.push((to.clone(), reply));
        }
    }
    if !refused.is_empty() {
        // Ends the transaction so no DATA can follow (RFC 5321 section 4.1.1.5).
        let _ = conn.command("RSET", "RSET").await;
        return Err(SmtpError::Refused { refused });
    }
    let step = "DATA";
    let reply = conn.command("DATA", step).await?;
    if reply.code != 354 {
        return Err(SmtpError::Rejected { step, reply });
    }
    let step = "the message";
    // A failed write leaves the final "." unsent, so nothing was delivered. Once it is written,
    // a lost reply leaves the message's fate unknown (RFC 1047).
    conn.write(&dot_stuffed(&message.mime), step).await?;
    let reply = conn
        .reply(step, DATA_TIMEOUT)
        .await
        .map_err(|e| SmtpError::Unconfirmed(Box::new(e)))?;
    if !reply.ok() {
        return Err(SmtpError::Rejected { step, reply });
    }
    Ok(())
}

/// AUTH with the initial response (RFC 4954 section 4): PLAIN (RFC 4616) with a password, or
/// XOAUTH2 with an access token
/// (<https://developers.google.com/workspace/gmail/imap/xoauth2-protocol>). A failed XOAUTH2
/// gets a 334 challenge with the error, answered with an empty line, after which the server sends
/// the final reply. Only the server's replies go into an error.
async fn authenticate<S: AsyncRead + AsyncWrite + Unpin>(
    conn: &mut Conn<S>,
    user: &str,
    login: Login<'_>,
) -> Result<(), SmtpError> {
    let step = "AUTH";
    let line = match login {
        Login::Password(password) => {
            format!(
                "AUTH PLAIN {}",
                BASE64.encode(format!("\0{user}\0{password}"))
            )
        }
        Login::XOAuth2 { access_token } => format!(
            "AUTH XOAUTH2 {}",
            BASE64.encode(xoauth2_response(user, access_token))
        ),
    };
    let mut reply = conn.command(&line, step).await?;
    if reply.code == 334 {
        // XOAUTH2's error challenge wants an empty answer; for any other mechanism "*" cancels
        // (RFC 4954 section 4). Either way the next reply is the final one.
        let answer = match login {
            Login::XOAuth2 { .. } => "",
            Login::Password(_) => "*",
        };
        reply = conn.command(answer, step).await?;
    }
    if reply.code != 235 {
        return Err(SmtpError::Auth {
            user: user.to_owned(),
            reply,
        });
    }
    Ok(())
}

/// The message as DATA sends it (RFC 5321 section 4.5.2): every line ends in CRLF, a line that
/// starts with "." gets another in front, and a line holding only "." ends it.
fn dot_stuffed(mime: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(mime.len() + mime.len() / 64 + 5);
    let body = mime.strip_suffix(b"\n").unwrap_or(mime);
    if !mime.is_empty() {
        for line in body.split(|b| *b == b'\n') {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if line.first() == Some(&b'.') {
                out.push(b'.');
            }
            out.extend_from_slice(line);
            out.extend_from_slice(b"\r\n");
        }
    }
    out.extend_from_slice(b".\r\n");
    out
}

/// A connection: commands out, replies in.
struct Conn<S> {
    stream: BufReader<S>,
}

impl<S> fmt::Debug for Conn<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Conn")
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> Conn<S> {
    fn new(stream: S) -> Self {
        Self {
            stream: BufReader::new(stream),
        }
    }

    fn into_inner(self) -> S {
        self.stream.into_inner()
    }

    /// Sends `line` and reads the reply.
    async fn command(&mut self, line: &str, step: &'static str) -> Result<Reply, SmtpError> {
        self.write(format!("{line}\r\n").as_bytes(), step).await?;
        self.reply(step, STEP_TIMEOUT).await
    }

    async fn write(&mut self, bytes: &[u8], step: &'static str) -> Result<(), SmtpError> {
        let stream = self.stream.get_mut();
        let io = |source| SmtpError::Io { step, source };
        timed(step, STEP_TIMEOUT, async {
            stream.write_all(bytes).await.map_err(io)?;
            stream.flush().await.map_err(io)
        })
        .await?
    }

    /// Reads one reply: lines of `CODE-text`, the last `CODE text` (RFC 5321 section 4.2.1).
    async fn reply(&mut self, step: &'static str, wait: Duration) -> Result<Reply, SmtpError> {
        timed(step, wait, self.read_reply(step)).await?
    }

    async fn read_reply(&mut self, step: &'static str) -> Result<Reply, SmtpError> {
        let mut code = None;
        let mut lines = Vec::new();
        for _ in 0..MAX_REPLY_LINES {
            let mut raw = Vec::new();
            let read = (&mut self.stream)
                .take(MAX_LINE)
                .read_until(b'\n', &mut raw)
                .await
                .map_err(|source| SmtpError::Io { step, source })?;
            if read == 0 {
                return Err(SmtpError::Closed(step));
            }
            let Some(line) = raw.strip_suffix(b"\n") else {
                return Err(SmtpError::Malformed(step));
            };
            let line = String::from_utf8_lossy(line.strip_suffix(b"\r").unwrap_or(line));
            let digits = line
                .get(..3)
                .filter(|d| d.bytes().all(|b| b.is_ascii_digit()));
            let this: u16 = digits
                .and_then(|d| d.parse().ok())
                .ok_or(SmtpError::Malformed(step))?;
            if code.is_some_and(|c| c != this) {
                return Err(SmtpError::Malformed(step));
            }
            code = Some(this);
            let more = match line.as_bytes().get(3) {
                Some(b'-') => true,
                None | Some(b' ') => false,
                Some(_) => return Err(SmtpError::Malformed(step)),
            };
            lines.push(line.get(4..).unwrap_or_default().trim().to_owned());
            if !more {
                return Ok(Reply { code: this, lines });
            }
        }
        Err(SmtpError::Malformed(step))
    }

    /// Ends the session politely. Errors are ignored: the work is done or already failed.
    async fn quit(&mut self) {
        let _ = self.command("QUIT", "QUIT").await;
        let _ = self.stream.get_mut().shutdown().await;
    }
}

/// Bounds one network step so a silent server cannot hang neomail.
async fn timed<F: Future>(
    what: &'static str,
    wait: Duration,
    fut: F,
) -> Result<F::Output, SmtpError> {
    tokio::time::timeout(wait, fut)
        .await
        .map_err(|_| SmtpError::Timeout(what))
}

#[cfg(test)]
mod tests;
