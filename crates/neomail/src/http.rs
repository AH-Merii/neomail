//! The HTTP neomail needs: OAuth's form POSTs, Microsoft Graph's GETs and the classifier's JSON
//! POSTs, over TLS. Plain `http://` is allowed only to this machine (a local classifier, the
//! tests' fake servers), and any port may be named. [`Https`] speaks HTTP/1.1 over rustls (ring and the system's trust store through
//! rustls-platform-verifier, as for IMAP), so no HTTP client crate, OpenSSL or second TLS stack is
//! pulled in. One connection per request (`Connection: close`); keep-alive is left for later.
//!
//! [`Http`] is the seam OAuth's tests replace with a scripted fake; Graph's seam is
//! `nm_graph::Transport`, which [`Https`] implements in `graph.rs`.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use rustls::ClientConfig;
use rustls::pki_types::ServerName;
use rustls_platform_verifier::BuilderVerifierExt;
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt,
};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

use crate::oauth::form_encode;

/// The most an OAuth answer may hold: token responses are a few kilobytes.
pub const MAX_FORM_RESPONSE: usize = 256 * 1024;
/// The most a response's status line and headers may hold.
const MAX_HEAD: usize = 64 * 1024;
/// Every network step's bound: connecting, the TLS handshake, sending, and each read. A whole
/// response is not bounded, so a slow download of large mail is not cut off.
const TIMEOUT: Duration = Duration::from_secs(60);
/// How much of the body is read per step.
const READ_CHUNK: usize = 64 * 1024;

/// A response's status, `Retry-After` and body. The body can hold tokens or mail, so `Debug`
/// leaves it out.
pub struct Response {
    pub status: u16,
    /// `retry-after-ms` in milliseconds, else `Retry-After` in seconds (RFC 9110 section
    /// 10.2.3); an HTTP date is ignored.
    pub retry_after: Option<Duration>,
    pub body: Vec<u8>,
}

impl fmt::Debug for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Response({}, <body redacted>)", self.status)
    }
}

/// POSTs `form` as `application/x-www-form-urlencoded` to an `https://` URL.
pub trait Http {
    async fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<Response>;
}

/// What to send.
struct Request<'a> {
    method: &'static str,
    url: &'a str,
    headers: &'a [(&'a str, &'a str)],
    /// The content type and the body.
    body: Option<(&'static str, &'a [u8])>,
    max_body: usize,
}

/// HTTP/1.1 over TLS, one connection per request.
pub struct Https {
    tls: Arc<ClientConfig>,
}

impl Https {
    pub fn new() -> Result<Self> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .and_then(|b| b.with_platform_verifier())
            .context("cannot set up TLS")?
            .with_no_client_auth();
        Ok(Self {
            tls: Arc::new(config),
        })
    }

    /// GETs `url` with `headers`, refusing a body larger than `max_body`.
    pub async fn get(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        max_body: usize,
    ) -> Result<Response> {
        self.send(Request {
            method: "GET",
            url,
            headers,
            body: None,
            max_body,
        })
        .await
    }

    /// PATCHes `url` with the JSON `body` and `headers`, refusing an answer larger than
    /// `max_body`.
    pub async fn patch_json(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        body: &[u8],
        max_body: usize,
    ) -> Result<Response> {
        self.send(Request {
            method: "PATCH",
            url,
            headers,
            body: Some(("application/json", body)),
            max_body,
        })
        .await
    }

    /// POSTs the JSON `body` to `url` with `headers`, refusing a 2xx answer larger than
    /// `max_body` and cutting any other status's body to it.
    pub async fn post_json(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        body: &[u8],
        max_body: usize,
    ) -> Result<Response> {
        self.send(Request {
            method: "POST",
            url,
            headers,
            body: Some(("application/json", body)),
            max_body,
        })
        .await
    }

    /// POSTs the text `body` (`text/plain`) to `url` with `headers`, as [`Self::post_json`].
    pub async fn post_text(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        body: &[u8],
        max_body: usize,
    ) -> Result<Response> {
        self.send(Request {
            method: "POST",
            url,
            headers,
            body: Some(("text/plain", body)),
            max_body,
        })
        .await
    }

    async fn send(&self, request: Request<'_>) -> Result<Response> {
        let target = split_url(request.url)?;
        let host = target.host;
        let tcp = step(host, async {
            TcpStream::connect((host, target.port))
                .await
                .with_context(|| format!("cannot connect to {host}"))
        })
        .await?;
        if !target.tls {
            return exchange(tcp, &request).await;
        }
        let handshake = async {
            let name = ServerName::try_from(host.to_owned())
                .map_err(|_| anyhow!("{host:?} is not a valid server name"))?;
            TlsConnector::from(self.tls.clone())
                .connect(name, tcp)
                .await
                .with_context(|| format!("TLS handshake with {host} failed"))
        };
        let tls = step(host, handshake).await?;
        exchange(tls, &request).await
    }
}

impl Http for Https {
    async fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<Response> {
        let body = form_encode(form);
        self.send(Request {
            method: "POST",
            url,
            headers: &[("Accept", "application/json")],
            body: Some(("application/x-www-form-urlencoded", body.as_bytes())),
            max_body: MAX_FORM_RESPONSE,
        })
        .await
    }
}

/// `work`, bounded by [`TIMEOUT`].
async fn step<T>(host: &str, work: impl Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(TIMEOUT, work)
        .await
        .map_err(|_| anyhow!("{host} did not answer within {} s", TIMEOUT.as_secs()))?
}

/// The host a URL names, lower-cased, if [`Https`] would send to it: an `https://` URL, or an
/// `http://` one to this machine.
pub fn url_host(url: &str) -> Result<String> {
    Ok(split_url(url)?.host.to_ascii_lowercase())
}

/// Where a URL points, as [`split_url`] reads it.
#[derive(Debug, PartialEq, Eq)]
struct Target<'a> {
    /// `https://`; plain `http://` only reaches a loopback host.
    tls: bool,
    /// The host to connect to, without an IPv6 literal's brackets.
    host: &'a str,
    port: u16,
    /// The host and port as written, for the `Host` header.
    authority: &'a str,
    path: &'a str,
}

/// The hosts plain `http://` may reach: this machine only, so nothing sent in the clear leaves
/// it (a local classifier, the tests' fake servers).
const LOOPBACK: [&str; 3] = ["127.0.0.1", "::1", "localhost"];

/// Reads an `https://host[:port]/path?query` URL, or an `http://` one to a [`LOOPBACK`] host. No
/// user or fragment, and the path is sent as it is, so it must be visible ASCII: Graph's next page
/// links come encoded.
fn split_url(url: &str) -> Result<Target<'_>> {
    // Never quote the URL: a next page link is the server's text.
    let (tls, rest) = match (url.strip_prefix("https://"), url.strip_prefix("http://")) {
        (Some(rest), _) => (true, rest),
        (None, Some(rest)) => (false, rest),
        (None, None) => bail!("not an https URL"),
    };
    let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    if authority.contains(['@', '?', '#']) {
        bail!("unsupported URL: the host must be a plain name");
    }
    let (host, port) = match authority.strip_prefix('[') {
        Some(inner) => {
            let (host, after) = inner
                .split_once(']')
                .context("unsupported URL: an IPv6 address must end with ]")?;
            if host.parse::<std::net::Ipv6Addr>().is_err() {
                bail!("unsupported URL: the host must be a plain name");
            }
            (host, after)
        }
        None => match authority.find(':') {
            Some(colon) => authority.split_at(colon),
            None => (authority, ""),
        },
    };
    let port = match port {
        "" => {
            if tls {
                443
            } else {
                80
            }
        }
        port => port
            .strip_prefix(':')
            .and_then(|p| p.parse::<u16>().ok())
            .filter(|&p| p != 0)
            .context("unsupported URL: the port must be a number from 1 to 65535")?,
    };
    if host.is_empty() || (!authority.starts_with('[') && host.contains(':')) {
        bail!("unsupported URL: the host must be a plain name");
    }
    if !tls && !LOOPBACK.iter().any(|l| host.eq_ignore_ascii_case(l)) {
        bail!(
            "plain http:// is allowed only to this machine (127.0.0.1, [::1], localhost); use https://"
        );
    }
    if !path.bytes().all(|b| b.is_ascii_graphic()) || path.contains('#') {
        bail!("unsupported URL: the path must be encoded, without a fragment");
    }
    Ok(Target {
        tls,
        host,
        port,
        authority,
        path: if path.is_empty() { "/" } else { path },
    })
}

/// A header name or value that could end the header early (CR, LF) or is not text.
fn valid_header(name: &str, value: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|b| b.is_ascii_graphic() && b != b':')
        && value
            .bytes()
            .all(|b| b == b'\t' || (b' '..=b'~').contains(&b))
}

/// Sends the request on `stream` and reads the response.
async fn exchange<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: S,
    request: &Request<'_>,
) -> Result<Response> {
    let Target {
        host,
        authority,
        path,
        ..
    } = split_url(request.url)?;
    let mut head = format!(
        "{} {path} HTTP/1.1\r\nHost: {authority}\r\nUser-Agent: neomail/{}\r\n",
        request.method,
        env!("CARGO_PKG_VERSION"),
    );
    for (name, value) in request.headers {
        if !valid_header(name, value) {
            // Never quote the value: it can be a token.
            bail!("the {name:?} header is not valid");
        }
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    if let Some((content_type, body)) = request.body {
        head.push_str(&format!(
            "Content-Type: {content_type}\r\nContent-Length: {}\r\n",
            body.len()
        ));
    }
    head.push_str("Connection: close\r\n\r\n");
    step(host, async {
        stream.write_all(head.as_bytes()).await?;
        if let Some((_, body)) = request.body {
            stream.write_all(body).await?;
        }
        stream.flush().await?;
        Ok(())
    })
    .await
    .context("sending the request failed")?;
    let mut reader = tokio::io::BufReader::new(stream);
    read_response(&mut reader, host, request.max_body).await
}

/// How a body ends (RFC 9112 section 6.3).
#[derive(Debug, PartialEq, Eq)]
enum Framing {
    Length(usize),
    Chunked,
    Close,
}

/// The parts of a response head neomail uses.
#[derive(Debug, PartialEq, Eq)]
struct Head {
    status: u16,
    retry_after: Option<Duration>,
    framing: Framing,
}

/// Parses a response's status line and headers (without the blank line).
fn parse_head(head: &[u8]) -> Result<Head> {
    let head = std::str::from_utf8(head).map_err(|_| anyhow!("malformed HTTP header"))?;
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|l| l.strip_prefix("HTTP/1."))
        .and_then(|s| s.get(2..5))
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| anyhow!("malformed HTTP status line"))?;
    let mut length = None;
    let mut chunked = false;
    let mut retry_after = None;
    let mut retry_after_ms = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            length = Some(
                value
                    .parse::<usize>()
                    .map_err(|_| anyhow!("malformed Content-Length"))?,
            );
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            chunked = value.to_ascii_lowercase().ends_with("chunked");
        } else if name.eq_ignore_ascii_case("retry-after") {
            retry_after = value.parse::<u64>().ok().map(Duration::from_secs);
        } else if name.eq_ignore_ascii_case("retry-after-ms") {
            retry_after_ms = value
                .parse::<f64>()
                .ok()
                .filter(|ms| ms.is_finite() && *ms >= 0.0)
                .map(|ms| Duration::from_secs_f64(ms.min(1e9) / 1000.0));
        }
    }
    // A 204 or 304 never has a body (RFC 9112 section 6.3, rule 1).
    let framing = if matches!(status, 204 | 304) {
        Framing::Length(0)
    } else if chunked {
        Framing::Chunked
    } else {
        length.map_or(Framing::Close, Framing::Length)
    };
    Ok(Head {
        status,
        // The finer of the two wins, wherever each comes in the head.
        retry_after: retry_after_ms.or(retry_after),
        framing,
    })
}

/// Reads one response: the head, then the body as its framing says, refusing a 2xx body larger
/// than `max_body`. Any other status's body is cut to `max_body` instead, so that a large error
/// page (a proxy's 502, say) still reports its status. Each read is bounded by [`TIMEOUT`].
async fn read_response<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    host: &str,
    max_body: usize,
) -> Result<Response> {
    let mut head = Vec::new();
    loop {
        let line = read_line(reader, host, MAX_HEAD - head.len().min(MAX_HEAD)).await?;
        if line.is_empty() {
            bail!("the connection closed before the response was complete");
        }
        if line == b"\r\n" {
            break;
        }
        head.extend_from_slice(&line);
    }
    let parsed = parse_head(head.strip_suffix(b"\r\n").unwrap_or(&head))?;
    let too_large = || anyhow!("the response is larger than {max_body} bytes");
    // The connection closes after each response, so a cut body leaves nothing behind.
    let cut = !(200..300).contains(&parsed.status);
    let mut body = Vec::new();
    match parsed.framing {
        Framing::Length(length) => {
            if length > max_body && !cut {
                return Err(too_large());
            }
            read_body(reader, host, &mut body, length.min(max_body), false).await?;
        }
        Framing::Close => {
            read_body(reader, host, &mut body, max_body.saturating_add(1), true).await?;
            if body.len() > max_body {
                if !cut {
                    return Err(too_large());
                }
                body.truncate(max_body);
            }
        }
        Framing::Chunked => loop {
            let line = read_line(reader, host, 1024).await?;
            let line = std::str::from_utf8(&line).map_err(|_| anyhow!("malformed chunk"))?;
            if line.is_empty() {
                bail!("the chunked response is truncated");
            }
            let size = line.split(';').next().unwrap_or("").trim();
            let size =
                usize::from_str_radix(size, 16).map_err(|_| anyhow!("malformed chunk size"))?;
            if size == 0 {
                // Trailers, up to the blank line; ignored.
                loop {
                    let line = read_line(reader, host, MAX_HEAD).await?;
                    if line.is_empty() || line == b"\r\n" {
                        break;
                    }
                }
                break;
            }
            if size > max_body - body.len() {
                if !cut {
                    return Err(too_large());
                }
                let room = max_body - body.len();
                read_body(reader, host, &mut body, room, false).await?;
                break;
            }
            read_body(reader, host, &mut body, size, false).await?;
            if read_line(reader, host, 2).await? != b"\r\n" {
                bail!("malformed chunk");
            }
        },
    }
    Ok(Response {
        status: parsed.status,
        retry_after: parsed.retry_after,
        body,
    })
}

/// One line with its CRLF, at most `max` bytes; empty at the end of the stream.
async fn read_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    host: &str,
    max: usize,
) -> Result<Vec<u8>> {
    let mut line = Vec::new();
    let mut limited = (&mut *reader).take(u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1));
    step(host, async {
        match limited.read_until(b'\n', &mut line).await {
            Ok(_) => Ok(()),
            // Servers often close without TLS close_notify once the response is complete.
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(()),
            Err(e) => Err(anyhow::Error::new(e).context("reading the response failed")),
        }
    })
    .await?;
    if line.len() > max {
        bail!("the response's header is too long");
    }
    Ok(line)
}

/// Appends `length` bytes to `body`, or with `to_end` everything up to the end of the stream
/// but at most `length` bytes.
async fn read_body<R: AsyncRead + Unpin>(
    reader: &mut R,
    host: &str,
    body: &mut Vec<u8>,
    length: usize,
    to_end: bool,
) -> Result<()> {
    let mut left = length;
    let mut chunk = vec![0u8; READ_CHUNK.min(length.max(1))];
    while left > 0 {
        let want = left.min(chunk.len());
        let n = step(host, async {
            match reader.read(&mut chunk[..want]).await {
                Ok(n) => Ok(n),
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(0),
                Err(e) => Err(anyhow::Error::new(e).context("reading the response failed")),
            }
        })
        .await?;
        if n == 0 {
            if to_end {
                return Ok(());
            }
            bail!("the response is shorter than its framing says");
        }
        body.extend_from_slice(&chunk[..n]);
        left -= n;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn parse(bytes: &[u8], max_body: usize) -> Result<Response> {
        let mut reader = bytes;
        read_response(&mut reader, "h.example", max_body).await
    }

    #[tokio::test]
    async fn parses_length_chunked_and_close_delimited_bodies() {
        let r = parse(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}", 10)
            .await
            .unwrap();
        assert_eq!((r.status, r.body.as_slice()), (200, &b"{}"[..]));
        let chunked = b"HTTP/1.1 400 Bad Request\r\ntransfer-encoding: chunked\r\n\r\n\
                        3\r\n{\"a\r\n4;x=y\r\n\":1}\r\n0\r\nX-Trailer: 1\r\n\r\n";
        let r = parse(chunked, 10).await.unwrap();
        assert_eq!((r.status, r.body.as_slice()), (400, &b"{\"a\":1}"[..]));
        assert!(parse(&chunked[..chunked.len() - 20], 10).await.is_err());
        assert!(
            parse(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n{}", 10)
                .await
                .is_err()
        );
        let r = parse(b"HTTP/1.0 200 OK\r\n\r\ns3cret", 10).await.unwrap();
        assert_eq!(r.body, b"s3cret");
        assert!(!format!("{r:?}").contains("s3cret"));
        let r = parse(b"HTTP/1.1 204 No Content\r\n\r\n", 10).await.unwrap();
        assert!(r.body.is_empty());
        assert!(parse(b"garbage\r\n\r\n", 10).await.is_err());
    }

    #[tokio::test]
    async fn the_body_cap_holds_for_every_framing() {
        for response in [
            &b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n12345"[..],
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\n123\r\n2\r\n45\r\n0\r\n\r\n",
            b"HTTP/1.1 200 OK\r\n\r\n12345",
        ] {
            let e = parse(response, 4).await.unwrap_err();
            assert_eq!(e.to_string(), "the response is larger than 4 bytes");
            assert_eq!(parse(response, 5).await.unwrap().body, b"12345");
        }
    }

    #[tokio::test]
    async fn an_error_body_over_the_cap_is_cut_and_keeps_its_status() {
        for response in [
            &b"HTTP/1.1 503 Busy\r\nContent-Length: 5\r\n\r\n12345"[..],
            b"HTTP/1.1 503 Busy\r\nTransfer-Encoding: chunked\r\n\r\n3\r\n123\r\n2\r\n45\r\n0\r\n\r\n",
            b"HTTP/1.1 503 Busy\r\n\r\n12345",
        ] {
            let r = parse(response, 4).await.unwrap();
            assert_eq!((r.status, r.body.as_slice()), (503, &b"1234"[..]));
        }
    }

    #[tokio::test]
    async fn retry_after_is_read_in_seconds() {
        let r = parse(
            b"HTTP/1.1 429 Too Many\r\nRetry-After: 7\r\nContent-Length: 0\r\n\r\n",
            0,
        )
        .await
        .unwrap();
        assert_eq!(
            (r.status, r.retry_after),
            (429, Some(Duration::from_secs(7)))
        );
        let r = parse(
            b"HTTP/1.1 503 Busy\r\nRetry-After: Wed, 21 Oct 2026 07:28:00 GMT\r\n\
              Content-Length: 0\r\n\r\n",
            0,
        )
        .await
        .unwrap();
        assert_eq!(r.retry_after, None);
    }

    fn split(url: &str) -> (bool, &str, u16, &str, &str) {
        let t = split_url(url).unwrap();
        (t.tls, t.host, t.port, t.authority, t.path)
    }

    #[test]
    fn urls_are_https_and_encoded() {
        assert_eq!(
            split("https://oauth2.googleapis.com/token"),
            (
                true,
                "oauth2.googleapis.com",
                443,
                "oauth2.googleapis.com",
                "/token"
            )
        );
        assert_eq!(
            split("https://graph.microsoft.com/v1.0/me?$top=5&$x=a%20b"),
            (
                true,
                "graph.microsoft.com",
                443,
                "graph.microsoft.com",
                "/v1.0/me?$top=5&$x=a%20b"
            )
        );
        assert_eq!(
            split("https://jev.example:8443"),
            (true, "jev.example", 8443, "jev.example:8443", "/")
        );
        assert_eq!(
            split("https://[2001:db8::1]:9/x"),
            (true, "2001:db8::1", 9, "[2001:db8::1]:9", "/x")
        );
        for bad in [
            "ftp://h/",
            "https://a@b/",
            "https://h:0/",
            "https://h:65536/",
            "https://h:x/",
            "https://h:/",
            "https://[h]/",
            "https://[::1/",
            "https://a:b:1/",
            "https:///x",
            "https://h/a b",
            "https://h/a\r\nX: y",
            "https://h/a#f",
        ] {
            assert!(split_url(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn plain_http_reaches_only_this_machine() {
        assert_eq!(
            split("http://127.0.0.1:4000/v1/systemone"),
            (false, "127.0.0.1", 4000, "127.0.0.1:4000", "/v1/systemone")
        );
        assert_eq!(
            split("http://[::1]:4000/"),
            (false, "::1", 4000, "[::1]:4000", "/")
        );
        assert_eq!(
            split("http://LocalHost/"),
            (false, "LocalHost", 80, "LocalHost", "/")
        );
        for bad in [
            "http://api.typesafe.ai/v1/systemone",
            "http://127.0.0.2/",
            "http://10.0.0.1:4000/",
            "http://[::2]/",
            "http://localhost.example/",
            "http://0.0.0.0/",
        ] {
            let e = split_url(bad).unwrap_err().to_string();
            assert!(
                e.contains("plain http:// is allowed only to this machine"),
                "{bad}: {e}"
            );
        }
    }

    #[tokio::test]
    async fn retry_after_ms_wins_over_retry_after() {
        for head in [
            &b"HTTP/1.1 429 x\r\nretry-after-ms: 250\r\nRetry-After: 7\r\nContent-Length: 0\r\n\r\n"[..],
            b"HTTP/1.1 429 x\r\nRetry-After: 7\r\nretry-after-ms: 250.0\r\nContent-Length: 0\r\n\r\n",
        ] {
            assert_eq!(parse(head, 0).await.unwrap().retry_after, Some(Duration::from_millis(250)));
        }
        let r = parse(
            b"HTTP/1.1 429 x\r\nretry-after-ms: soon\r\nRetry-After: 7\r\nContent-Length: 0\r\n\r\n",
            0,
        )
        .await
        .unwrap();
        assert_eq!(r.retry_after, Some(Duration::from_secs(7)));
    }

    #[tokio::test]
    async fn a_request_over_plain_http_reaches_a_loopback_server() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = tokio::io::BufReader::new(stream);
            let mut head = String::new();
            loop {
                let mut line = String::new();
                stream.read_line(&mut line).await.unwrap();
                head.push_str(&line);
                if line == "\r\n" {
                    break;
                }
            }
            let mut body = [0u8; 2];
            stream.read_exact(&mut body).await.unwrap();
            stream
                .get_mut()
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\n{\"a\"")
                .await
                .unwrap();
            head
        });
        let https = Https::new().unwrap();
        let url = format!("http://127.0.0.1:{port}/v1/systemone");
        let r = https
            .post_json(&url, &[("Authorization", "Bearer k")], b"{}", 10)
            .await
            .unwrap();
        assert_eq!((r.status, r.body.as_slice()), (200, &b"{\"a\""[..]));
        let head = server.await.unwrap();
        assert!(
            head.starts_with(&format!(
                "POST /v1/systemone HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n"
            )),
            "{head}"
        );
    }

    async fn serve_once(
        request: Request<'_>,
        answer: &'static [u8],
    ) -> (Result<Response>, String, String) {
        let (client, server) = tokio::io::duplex(64 * 1024);
        let server = tokio::spawn(async move {
            let (read, mut write) = tokio::io::split(server);
            let mut read = tokio::io::BufReader::new(read);
            let mut head = String::new();
            loop {
                let mut line = String::new();
                read.read_line(&mut line).await.unwrap();
                head.push_str(&line);
                if line == "\r\n" {
                    break;
                }
            }
            let length = head
                .lines()
                .find_map(|l| l.strip_prefix("Content-Length: "))
                .map_or(0, |n| n.parse().unwrap());
            let mut body = vec![0u8; length];
            read.read_exact(&mut body).await.unwrap();
            write.write_all(answer).await.unwrap();
            (head, String::from_utf8(body).unwrap())
        });
        let r = exchange(client, &request).await;
        let (head, body) = server.await.unwrap();
        (r, head, body)
    }

    #[tokio::test]
    async fn sends_a_form_post_and_reads_the_answer() {
        let body = form_encode(&[("a", "1"), ("b", "x y/")]);
        let (r, head, body) = serve_once(
            Request {
                method: "POST",
                url: "https://h.example/token",
                headers: &[("Accept", "application/json")],
                body: Some(("application/x-www-form-urlencoded", body.as_bytes())),
                max_body: 10,
            },
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}",
        )
        .await;
        assert_eq!(r.unwrap().body, b"{}");
        assert!(head.starts_with("POST /token HTTP/1.1\r\nHost: h.example\r\n"));
        assert!(head.contains("Accept: application/json\r\n"));
        assert!(head.contains("Content-Type: application/x-www-form-urlencoded\r\n"));
        assert_eq!(body, "a=1&b=x%20y%2F");
    }

    #[tokio::test]
    async fn sends_a_get_with_headers_and_reads_retry_after() {
        let (r, head, body) = serve_once(
            Request {
                method: "GET",
                url: "https://graph.example/v1.0/me?$top=1",
                headers: &[("Authorization", "Bearer t0k"), ("Prefer", "x=\"y\"")],
                body: None,
                max_body: 10,
            },
            b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 3\r\nContent-Length: 0\r\n\r\n",
        )
        .await;
        let r = r.unwrap();
        assert_eq!(
            (r.status, r.retry_after),
            (429, Some(Duration::from_secs(3)))
        );
        assert!(head.starts_with("GET /v1.0/me?$top=1 HTTP/1.1\r\nHost: graph.example\r\n"));
        assert!(head.contains("\r\nAuthorization: Bearer t0k\r\nPrefer: x=\"y\"\r\n"));
        assert!(!head.contains("Content-Length"));
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn a_header_that_could_split_the_request_is_refused_unquoted() {
        let (client, _server) = tokio::io::duplex(1024);
        let e = exchange(
            client,
            &Request {
                method: "GET",
                url: "https://h.example/",
                headers: &[("Authorization", "Bearer s3cret\r\nX: y")],
                body: None,
                max_body: 10,
            },
        )
        .await
        .unwrap_err();
        assert!(!format!("{e:#}").contains("s3cret"), "{e:#}");
    }
}
