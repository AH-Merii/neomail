//! The one HTTP request OAuth needs: a form POST over TLS, answered with a small JSON body.
//! [`Http`] is the seam the tests replace; [`Https`] speaks HTTP/1.1 over rustls (ring and the
//! system's trust store through rustls-platform-verifier, as for IMAP), so no HTTP client crate,
//! OpenSSL or second TLS stack is pulled in.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use rustls::ClientConfig;
use rustls::pki_types::ServerName;
use rustls_platform_verifier::BuilderVerifierExt;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

use super::form_encode;

/// The most a response may hold: token responses are a few kilobytes.
const MAX_RESPONSE: usize = 256 * 1024;
const TIMEOUT: Duration = Duration::from_secs(60);

/// A response's status and body. The body can hold tokens, so `Debug` leaves it out.
pub struct Response {
    pub status: u16,
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
}

impl Http for Https {
    async fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<Response> {
        let (host, path) = split_https_url(url)?;
        let work = async {
            let tcp = TcpStream::connect((host, 443))
                .await
                .with_context(|| format!("cannot connect to {host}"))?;
            let name = ServerName::try_from(host.to_owned())
                .map_err(|_| anyhow!("{host:?} is not a valid server name"))?;
            let tls = TlsConnector::from(self.tls.clone())
                .connect(name, tcp)
                .await
                .with_context(|| format!("TLS handshake with {host} failed"))?;
            exchange(tls, host, path, form).await
        };
        tokio::time::timeout(TIMEOUT, work)
            .await
            .map_err(|_| anyhow!("{host} did not answer within {} s", TIMEOUT.as_secs()))?
    }
}

/// The host and path of an `https://host/path` URL. No port, user or fragment: the endpoints
/// are fixed in the provider's data.
fn split_https_url(url: &str) -> Result<(&str, &str)> {
    let rest = url
        .strip_prefix("https://")
        .with_context(|| format!("not an https URL: {url}"))?;
    let (host, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    if host.is_empty() || host.contains([':', '@']) {
        bail!("unsupported URL: {url}");
    }
    Ok((host, if path.is_empty() { "/" } else { path }))
}

/// Sends the request on `stream` and reads the response.
async fn exchange<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: S,
    host: &str,
    path: &str,
    form: &[(&str, &str)],
) -> Result<Response> {
    let body = form_encode(form);
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: nmail/{}\r\n\
         Accept: application/json\r\nContent-Type: application/x-www-form-urlencoded\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        env!("CARGO_PKG_VERSION"),
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body.as_bytes()).await?;
    stream.flush().await?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        if let Some(response) = parse_response(&buf, false)? {
            return Ok(response);
        }
        let n = match stream.read(&mut chunk).await {
            Ok(n) => n,
            // Servers often close without TLS close_notify once the response is complete.
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => 0,
            Err(e) => return Err(anyhow::Error::new(e).context("reading the response failed")),
        };
        if n == 0 {
            return parse_response(&buf, true)?
                .context("the connection closed before the response was complete");
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > MAX_RESPONSE {
            bail!("the response is larger than {MAX_RESPONSE} bytes");
        }
    }
}

/// Parses an HTTP/1.1 response from `buf`: `None` while it is incomplete. The body is framed by
/// `Content-Length`, chunked transfer coding, or (with `eof`) the end of the connection.
pub fn parse_response(buf: &[u8], eof: bool) -> Result<Option<Response>> {
    let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
        return if eof {
            Err(anyhow!("the response has no complete header"))
        } else {
            Ok(None)
        };
    };
    let head = std::str::from_utf8(&buf[..end]).map_err(|_| anyhow!("malformed HTTP header"))?;
    let rest = &buf[end + 4..];
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    let status = status_line
        .strip_prefix("HTTP/1.")
        .and_then(|s| s.get(2..5))
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| anyhow!("malformed HTTP status line"))?;
    let mut length = None;
    let mut chunked = false;
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
        }
    }
    let body = if chunked {
        match dechunk(rest)? {
            Some(body) => body,
            None if eof => bail!("the chunked response is truncated"),
            None => return Ok(None),
        }
    } else if let Some(length) = length {
        match rest.get(..length) {
            Some(body) => body.to_vec(),
            None if eof => bail!("the response is shorter than its Content-Length"),
            None => return Ok(None),
        }
    } else if eof {
        rest.to_vec()
    } else {
        return Ok(None);
    };
    Ok(Some(Response { status, body }))
}

/// Decodes a chunked body: `None` until the last (empty) chunk has arrived. Trailers are ignored.
fn dechunk(mut data: &[u8]) -> Result<Option<Vec<u8>>> {
    let mut body = Vec::new();
    loop {
        let Some(eol) = data.windows(2).position(|w| w == b"\r\n") else {
            return Ok(None);
        };
        let line = std::str::from_utf8(&data[..eol]).map_err(|_| anyhow!("malformed chunk"))?;
        let size = line.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size, 16).map_err(|_| anyhow!("malformed chunk size"))?;
        data = &data[eol + 2..];
        if size == 0 {
            return Ok(Some(body));
        }
        let Some(chunk) = data.get(..size) else {
            return Ok(None);
        };
        body.extend_from_slice(chunk);
        match data.get(size..size + 2) {
            Some(b"\r\n") => data = &data[size + 2..],
            Some(_) => bail!("malformed chunk"),
            None => return Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_length_chunked_and_close_delimited_bodies() {
        let r = parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}", false)
            .unwrap()
            .unwrap();
        assert_eq!((r.status, r.body.as_slice()), (200, &b"{}"[..]));
        let chunked = b"HTTP/1.1 400 Bad Request\r\ntransfer-encoding: chunked\r\n\r\n\
                        3\r\n{\"a\r\n4;x=y\r\n\":1}\r\n0\r\n\r\n";
        let r = parse_response(chunked, false).unwrap().unwrap();
        assert_eq!((r.status, r.body.as_slice()), (400, &b"{\"a\":1}"[..]));
        // Incomplete until the last chunk or the full length is there.
        assert!(
            parse_response(&chunked[..chunked.len() - 5], false)
                .unwrap()
                .is_none()
        );
        let short = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n{}";
        assert!(parse_response(short, false).unwrap().is_none());
        assert!(parse_response(short, true).is_err());
        let r = parse_response(b"HTTP/1.0 200 OK\r\n\r\ns3cret", true)
            .unwrap()
            .unwrap();
        assert_eq!(r.body, b"s3cret");
        assert!(parse_response(b"garbage\r\n\r\n", true).is_err());
        assert!(!format!("{r:?}").contains("s3cret"));
    }

    #[test]
    fn urls_are_https_only() {
        assert_eq!(
            split_https_url("https://oauth2.googleapis.com/token").unwrap(),
            ("oauth2.googleapis.com", "/token")
        );
        for bad in [
            "http://oauth2.googleapis.com/token",
            "https://a@b/",
            "https://h:1/",
        ] {
            assert!(split_https_url(bad).is_err(), "{bad}");
        }
    }

    #[tokio::test]
    async fn sends_a_form_post_and_reads_the_answer() {
        use tokio::io::AsyncBufReadExt;
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
            let mut body = vec![0u8; "a=1&b=x%20y%2F".len()];
            read.read_exact(&mut body).await.unwrap();
            write
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
                .await
                .unwrap();
            (head, String::from_utf8(body).unwrap())
        });
        let r = exchange(client, "h.example", "/token", &[("a", "1"), ("b", "x y/")])
            .await
            .unwrap();
        assert_eq!(r.body, b"{}");
        let (head, body) = server.await.unwrap();
        assert!(head.starts_with("POST /token HTTP/1.1\r\nHost: h.example\r\n"));
        assert!(head.contains("Content-Type: application/x-www-form-urlencoded\r\n"));
        assert_eq!(body, "a=1&b=x%20y%2F");
    }
}
