//! The loopback redirect (RFC 8252 section 7.3): a listener on 127.0.0.1 with a port the OS
//! picks, waiting for the browser to bring back the authorization code.
//!
//! It answers anything that is not the redirect (a favicon, a stray tab) with 404 and keeps
//! waiting, ignores a redirect with the wrong `state` (another page cannot end the sign-in),
//! handles up to [`MAX_CONNECTIONS`] connections concurrently, bounds each request's size and
//! time, gives up after the overall timeout, and never puts the code in a page, a message or an error.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;

use super::Secret;

/// The most a request's head may hold. A redirect is well under 4 KiB.
const MAX_REQUEST: usize = 16 * 1024;
/// How long one connection may take to send its request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// How many connections are handled at once. More wait in the kernel's queue until one ends, so
/// a local process opening many cannot make the listener hold an unbounded number.
pub const MAX_CONNECTIONS: usize = 16;

pub struct Listener {
    listener: TcpListener,
    port: u16,
}

/// What one request was.
#[derive(Debug, PartialEq, Eq)]
pub enum Request {
    /// The redirect with a code and the right state.
    Code(Secret),
    /// The redirect saying the user declined.
    Denied,
    /// The redirect with another error code (RFC 6749 section 4.1.2.1).
    Error(String),
    /// The redirect with the right state but neither a code nor an error.
    NoCode,
    /// A redirect whose state is missing or wrong.
    WrongState,
    /// Anything else: another path, method or a malformed request.
    Stray,
}

impl Listener {
    pub async fn bind() -> Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .context("cannot listen on 127.0.0.1 for the sign-in redirect")?;
        let port = listener.local_addr()?.port();
        Ok(Self { listener, port })
    }

    /// The `redirect_uri`: Google accepts any port on the loopback address for a desktop client.
    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Waits for the redirect carrying `state`, for at most `timeout`.
    pub async fn wait(&self, state: &str, timeout: Duration) -> Result<Secret> {
        tokio::time::timeout(timeout, self.accept_until_done(state))
            .await
            .map_err(|_| {
                anyhow!(
                    "no sign-in came back within {} minutes; run the command again",
                    timeout.as_secs() / 60
                )
            })?
    }

    /// Accepts connections and handles each on its own task, so an idle one (a browser's
    /// speculative preconnect) cannot hold up the redirect. At most [`MAX_CONNECTIONS`] are
    /// handled at once: at the cap it stops accepting until one ends. Dropping the set aborts the
    /// rest.
    async fn accept_until_done(&self, state: &str) -> Result<Secret> {
        let state: Arc<str> = state.into();
        let mut handlers = JoinSet::new();
        loop {
            let request = tokio::select! {
                accepted = self.listener.accept(), if handlers.len() < MAX_CONNECTIONS => {
                    let (stream, _) = accepted.context("the sign-in listener failed")?;
                    handlers.spawn(handle(stream, state.clone()));
                    continue;
                }
                Some(joined) = handlers.join_next() => joined.unwrap_or(Request::Stray),
            };
            match request {
                Request::Code(code) => return Ok(code),
                Request::Denied => bail!("access was declined in the browser; nothing was saved"),
                Request::Error(e) => bail!("the sign-in failed in the browser: {e}"),
                Request::NoCode => bail!("the sign-in redirect carried no authorization code"),
                Request::WrongState | Request::Stray => continue,
            }
        }
    }
}

/// Reads one request, answers it and says what it was.
async fn handle(mut stream: TcpStream, state: Arc<str>) -> Request {
    let request = match read_head(&mut stream).await {
        Ok(head) => parse_request(&head, &state),
        Err(Oversized) => {
            let _ = respond(&mut stream, "431 Request Header Fields Too Large", "").await;
            drain(&mut stream).await;
            return Request::Stray;
        }
    };
    let (status, page) = match &request {
        Request::Code(_) => (
            "200 OK",
            "Signed in. You can close this tab and return to the terminal.",
        ),
        Request::Denied => ("200 OK", "Sign-in cancelled. You can close this tab."),
        Request::Error(_) | Request::NoCode => {
            ("400 Bad Request", "Sign-in failed; see the terminal.")
        }
        Request::WrongState => (
            "400 Bad Request",
            "This sign-in link is not the current one.",
        ),
        Request::Stray => ("404 Not Found", ""),
    };
    let _ = respond(&mut stream, status, page).await;
    request
}

/// The request was larger than [`MAX_REQUEST`].
struct Oversized;

/// Reads a request's head (up to the blank line). A slow or silent client gets what it sent so
/// far after [`REQUEST_TIMEOUT`], which parses as a stray request.
async fn read_head(stream: &mut TcpStream) -> Result<Vec<u8>, Oversized> {
    let mut buf = Vec::new();
    let read = async {
        let mut chunk = [0u8; 2048];
        loop {
            match stream.read(&mut chunk).await {
                Ok(0) | Err(_) => return Ok(()),
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
            if buf.len() > MAX_REQUEST {
                return Err(Oversized);
            }
            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                return Ok(());
            }
        }
    };
    match tokio::time::timeout(REQUEST_TIMEOUT, read).await {
        Ok(Err(Oversized)) => Err(Oversized),
        Ok(Ok(())) | Err(_) => Ok(buf),
    }
}

/// Reads what is left of an oversized request for a moment, so closing the socket sends the
/// response and a FIN rather than a reset that could discard it.
async fn drain(stream: &mut TcpStream) {
    let mut chunk = [0u8; 8192];
    let mut left: usize = 1024 * 1024;
    let read = async {
        while left > 0 {
            match stream.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => left = left.saturating_sub(n),
            }
        }
    };
    let _ = tokio::time::timeout(Duration::from_secs(1), read).await;
}

async fn respond(stream: &mut TcpStream, status: &str, page: &str) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\n\
         Content-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\nneomail: {page}",
        page.len() + "neomail: ".len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}

/// Classifies a request head. Only `GET /?…` is the redirect; its query is checked for `state`
/// before anything else, so a page that guesses the port cannot cancel the sign-in.
pub fn parse_request(head: &[u8], state: &str) -> Request {
    let Some(line) = head.split(|&b| b == b'\n').next() else {
        return Request::Stray;
    };
    let Ok(line) = std::str::from_utf8(line) else {
        return Request::Stray;
    };
    let mut parts = line.trim_end_matches('\r').split(' ');
    let (Some("GET"), Some(target)) = (parts.next(), parts.next()) else {
        return Request::Stray;
    };
    let query = match target.split_once('?') {
        Some(("/", query)) => query,
        _ => return Request::Stray,
    };
    let param = |name: &str| {
        query
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .find(|(k, _)| *k == name)
            .and_then(|(_, v)| percent_decode(v))
    };
    if param("state").as_deref() != Some(state) {
        return Request::WrongState;
    }
    if let Some(error) = param("error") {
        return if error == "access_denied" {
            Request::Denied
        } else {
            Request::Error(
                error
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(100)
                    .collect(),
            )
        };
    }
    match param("code") {
        Some(code) if !code.is_empty() => Request::Code(Secret::new(code)),
        _ => Request::NoCode,
    }
}

/// Decodes `%XX` and `+` in a query value; `None` when it is not valid UTF-8 or a `%` is
/// malformed.
fn percent_decode(s: &str) -> Option<String> {
    let mut out = Vec::with_capacity(s.len());
    let mut bytes = s.bytes();
    while let Some(b) = bytes.next() {
        match b {
            b'+' => out.push(b' '),
            b'%' => {
                let hex = [bytes.next()?, bytes.next()?];
                out.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
            }
            _ => out.push(b),
        }
    }
    String::from_utf8(out).ok()
}
