//! Is Bridge ready? Reads the certificate one of Bridge's ports presents, securing the connection
//! as the user answered, and tells a port where nothing listens from a security answer that does
//! not match what the server speaks. It logs in to nothing. It sends a TLS hello on a first
//! connection, then, for STARTTLS, only IMAP's STARTTLS or SMTP's EHLO and STARTTLS on a second
//! one.

use std::io::{self, BufRead, BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use nm_imap::{CertPin, Security};
use rustls::ClientConnection;
use rustls::pki_types::ServerName;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Imap,
    Smtp,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ProbeError {
    /// Nothing listens on the port: the connection was refused.
    Refused,
    /// The server speaks the other security: [`Security::Tls`] when it answered a TLS hello,
    /// [`Security::StartTls`] when it greeted in plain text.
    Speaks(Security),
    /// Anything else, said as a sentence.
    Other(String),
}

/// How long a connect, a read or a write may take. A server on this computer answers at once, so
/// only a server that hangs ever waits this long.
const TIMEOUT: Duration = Duration::from_secs(10);

/// The SHA-256 of the certificate the server at `host:port` presents when the connection is
/// secured with `security`. The certificate is trusted for nothing: it is only read.
pub fn certificate(
    host: &str,
    port: u16,
    protocol: Protocol,
    security: Security,
) -> Result<CertPin, ProbeError> {
    // A TLS hello first: a server that speaks TLS from the first byte answers it with a TLS
    // record, and one that speaks plain text has sent its greeting already. So neither answer
    // waits for a greeting that never comes.
    let mut tcp = connect(host, port)?;
    let mut tls = client(host)?;
    while tls.wants_write() {
        tls.write_tls(&mut tcp).map_err(failed("the TLS hello"))?;
    }
    let mut first = [0u8; 1];
    if tcp.peek(&mut first).map_err(failed("the first answer"))? == 0 {
        return Err(ProbeError::Other(
            "the server closed the connection without answering".to_owned(),
        ));
    }
    // A TLS record starts with its content type: 0x16 a handshake, 0x15 an alert (RFC 8446
    // section 5.1). A greeting starts with "* OK" (IMAP) or "220" (SMTP).
    let speaks_tls = matches!(first[0], 0x15 | 0x16);
    match (speaks_tls, security) {
        (true, Security::Tls) => handshake(tls, tcp),
        (true, Security::StartTls) => Err(ProbeError::Speaks(Security::Tls)),
        (false, Security::Tls) => Err(ProbeError::Speaks(Security::StartTls)),
        (false, Security::StartTls) => {
            drop(tcp);
            let mut tcp = connect(host, port)?;
            start_tls(&mut tcp, protocol)?;
            handshake(client(host)?, tcp)
        }
    }
}

fn connect(host: &str, port: u16) -> Result<TcpStream, ProbeError> {
    let cannot = |e: io::Error| ProbeError::Other(format!("cannot connect to {host}:{port}: {e}"));
    let addr = (host, port)
        .to_socket_addrs()
        .map_err(cannot)?
        .next()
        .ok_or_else(|| ProbeError::Other(format!("{host} has no address")))?;
    let tcp = TcpStream::connect_timeout(&addr, TIMEOUT).map_err(|e| {
        if e.kind() == io::ErrorKind::ConnectionRefused {
            ProbeError::Refused
        } else {
            cannot(e)
        }
    })?;
    tcp.set_read_timeout(Some(TIMEOUT)).map_err(cannot)?;
    tcp.set_write_timeout(Some(TIMEOUT)).map_err(cannot)?;
    Ok(tcp)
}

fn client(host: &str) -> Result<ClientConnection, ProbeError> {
    let other = |e: rustls::Error| ProbeError::Other(format!("cannot set up TLS: {e}"));
    let name = ServerName::try_from(host.to_owned())
        .map_err(|_| ProbeError::Other(format!("{host:?} is not a valid server name")))?;
    ClientConnection::new(nm_imap::capture_config().map_err(other)?, name).map_err(other)
}

fn failed(during: &'static str) -> impl Fn(io::Error) -> ProbeError {
    move |e| ProbeError::Other(format!("the connection failed during {during}: {e}"))
}

fn handshake(mut tls: ClientConnection, mut tcp: TcpStream) -> Result<CertPin, ProbeError> {
    while tls.is_handshaking() {
        tls.complete_io(&mut tcp)
            .map_err(|e| ProbeError::Other(format!("the TLS handshake failed: {e}")))?;
    }
    let pin = tls
        .peer_certificates()
        .and_then(|certs| certs.first())
        .map(|cert| CertPin::of(cert.as_ref()))
        .ok_or_else(|| ProbeError::Other("the server presented no certificate".to_owned()))?;
    tls.send_close_notify();
    while tls.wants_write() && tls.write_tls(&mut tcp).is_ok() {}
    Ok(pin)
}

/// On a plain connection: the greeting, then STARTTLS (after EHLO, for SMTP).
fn start_tls(tcp: &mut TcpStream, protocol: Protocol) -> Result<(), ProbeError> {
    let mut reader = BufReader::new(tcp.try_clone().map_err(failed("STARTTLS"))?);
    let mut send = |command: &str| {
        tcp.write_all(command.as_bytes())
            .map_err(failed("STARTTLS"))
    };
    match protocol {
        Protocol::Imap => {
            let greeting = line(&mut reader, "the greeting")?;
            if !greeting.starts_with("* OK") {
                return Err(not_a_greeting(&greeting, "an IMAP", "SMTP"));
            }
            send("n1 STARTTLS\r\n")?;
            loop {
                let answer = line(&mut reader, "STARTTLS")?;
                if let Some(status) = answer.strip_prefix("n1 ") {
                    if !status.to_ascii_uppercase().starts_with("OK") {
                        return Err(refused_starttls(&answer));
                    }
                    break;
                }
            }
        }
        Protocol::Smtp => {
            let greeting = reply(&mut reader, "the greeting")?;
            if !greeting[0].starts_with("220") {
                return Err(not_a_greeting(&greeting[0], "an SMTP", "IMAP"));
            }
            send("EHLO [127.0.0.1]\r\n")?;
            let ehlo = reply(&mut reader, "EHLO")?;
            let offers = ehlo.iter().any(|l| {
                l.get(4..)
                    .is_some_and(|e| e.eq_ignore_ascii_case("STARTTLS"))
            });
            if !ehlo[0].starts_with("250") || !offers {
                return Err(refused_starttls(&ehlo.join(" ")));
            }
            send("STARTTLS\r\n")?;
            let answer = reply(&mut reader, "STARTTLS")?;
            if !answer[0].starts_with("220") {
                return Err(refused_starttls(&answer.join(" ")));
            }
        }
    }
    if !reader.buffer().is_empty() {
        return Err(ProbeError::Other(
            "the server sent data before the TLS handshake".to_owned(),
        ));
    }
    Ok(())
}

fn not_a_greeting(greeting: &str, expected: &str, other: &str) -> ProbeError {
    ProbeError::Other(format!(
        "the server greeted with {greeting:?}, which is not {expected} greeting (is this \
         Bridge's {other} port?)"
    ))
}

fn refused_starttls(answer: &str) -> ProbeError {
    ProbeError::Other(format!("the server did not take STARTTLS: {answer:?}"))
}

/// One line, without its line end.
fn line(reader: &mut impl BufRead, during: &'static str) -> Result<String, ProbeError> {
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(0) => Err(ProbeError::Other(format!(
            "the server closed the connection during {during}"
        ))),
        Ok(_) => Ok(line.trim_end().to_owned()),
        Err(e) => Err(failed(during)(e)),
    }
}

/// An SMTP reply's lines: each but the last has a `-` after its code (RFC 5321 section 4.2.1).
fn reply(reader: &mut impl BufRead, during: &'static str) -> Result<Vec<String>, ProbeError> {
    let mut lines = Vec::new();
    loop {
        let l = line(reader, during)?;
        let last = l.as_bytes().get(3) != Some(&b'-');
        lines.push(l);
        if last || lines.len() > 100 {
            return Ok(lines);
        }
    }
}
