//! The SMTP client against a fake server that records what it gets: over an in-memory pipe for
//! the session, and over TLS and STARTTLS on 127.0.0.1 for the connection.
#![allow(clippy::unwrap_used)]

use std::net::SocketAddr;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

use super::*;

const PASSWORD: &str = "hunter2-test-secret";
const TOKEN: &str = "ya29.test-access-token";

/// How the fake server answers.
#[derive(Debug, Clone)]
struct Script {
    /// RCPT TO addresses it refuses with 550.
    refuse: Vec<&'static str>,
    /// The reply to the end of DATA.
    data_code: u16,
    auth_fails: bool,
    offer_starttls: bool,
    /// Sends a line of its own right after agreeing to STARTTLS.
    inject: bool,
    /// Closes the connection after the message's final "." instead of answering it.
    hang_up_after_data: bool,
}

impl Default for Script {
    fn default() -> Self {
        Self {
            refuse: Vec::new(),
            data_code: 250,
            auth_fails: false,
            offer_starttls: true,
            inject: false,
            hang_up_after_data: false,
        }
    }
}

/// What the fake server got: each command line, the lines said before TLS, and the raw DATA
/// (still dot-stuffed, without the final ".").
#[derive(Debug, Default)]
struct Seen {
    commands: Vec<String>,
    before_tls: Vec<String>,
    data: Option<Vec<u8>>,
}

impl Seen {
    fn verbs(&self) -> Vec<String> {
        self.commands
            .iter()
            .map(|c| c.split([' ', ':']).next().unwrap().to_ascii_uppercase())
            .collect()
    }
}

/// One SMTP session after the TLS question is settled.
async fn session<S: AsyncRead + AsyncWrite + Unpin>(stream: S, script: &Script, seen: &mut Seen) {
    let mut s = BufReader::new(stream);
    loop {
        let mut line = String::new();
        if s.read_line(&mut line).await.unwrap_or(0) == 0 {
            return;
        }
        let line = line.trim_end_matches(['\r', '\n']).to_owned();
        let upper = line.to_ascii_uppercase();
        seen.commands.push(line.clone());
        let reply = if upper.starts_with("EHLO") {
            "250-fake.example\r\n250-AUTH PLAIN XOAUTH2\r\n250 8BITMIME\r\n".to_owned()
        } else if upper.starts_with("AUTH XOAUTH2") && script.auth_fails {
            "334 eyJzdGF0dXMiOiI0MDEifQ==\r\n".to_owned()
        } else if upper.starts_with("AUTH") || line.is_empty() || line == "*" {
            if script.auth_fails {
                "535 5.7.8 bad credentials\r\n"
            } else {
                "235 2.7.0 ok\r\n"
            }
            .to_owned()
        } else if upper.starts_with("MAIL FROM:") || upper == "RSET" {
            "250 ok\r\n".to_owned()
        } else if let Some(to) = line
            .strip_prefix("RCPT TO:<")
            .and_then(|t| t.strip_suffix('>'))
        {
            if script.refuse.contains(&to) {
                "550 5.1.1 no such user\r\n"
            } else {
                "250 ok\r\n"
            }
            .to_owned()
        } else if upper == "DATA" {
            s.write_all(b"354 go on\r\n").await.unwrap();
            let mut data = Vec::new();
            loop {
                let mut raw = Vec::new();
                if s.read_until(b'\n', &mut raw).await.unwrap() == 0 || raw == b".\r\n" {
                    break;
                }
                data.extend_from_slice(&raw);
            }
            seen.data = Some(data);
            if script.hang_up_after_data {
                return;
            }
            format!("{} done\r\n", script.data_code)
        } else if upper == "QUIT" {
            s.write_all(b"221 bye\r\n").await.unwrap();
            return;
        } else {
            "502 not scripted\r\n".to_owned()
        };
        s.write_all(reply.as_bytes()).await.unwrap();
    }
}

/// A message whose body has lines that start with ".".
fn message(to: &[&str]) -> Outgoing {
    Outgoing {
        mime: b"From: me@example.com\r\nSubject: s\r\n\r\n.hidden\r\n..two\r\n.\r\nend\r\n"
            .to_vec(),
        envelope_from: "me@example.com".to_owned(),
        envelope_to: to.iter().map(|t| (*t).to_owned()).collect(),
        bcc: Vec::new(),
        message_id: "id@example.com".to_owned(),
    }
}

/// Runs [`send_on`] against the fake over a pipe.
async fn over_pipe(
    script: Script,
    login: Login<'_>,
    msg: &Outgoing,
) -> (Result<(), SmtpError>, Seen) {
    let (client, server) = tokio::io::duplex(64 * 1024);
    let server = tokio::spawn(async move {
        let mut seen = Seen::default();
        let mut server = server;
        server.write_all(b"220 fake ESMTP\r\n").await.unwrap();
        session(server, &script, &mut seen).await;
        seen
    });
    let mut conn = Conn::new(client);
    hello(&mut conn).await.unwrap();
    let result = send_on(&mut conn, "me@example.com", login, msg).await;
    conn.quit().await;
    drop(conn);
    (result, server.await.unwrap())
}

#[tokio::test]
async fn a_message_goes_out_with_its_envelope_and_the_exact_bytes_dot_stuffed() {
    let msg = message(&["ann@example.org", "bob@example.org"]);
    let (result, seen) = over_pipe(Script::default(), Login::Password(PASSWORD), &msg).await;
    result.unwrap();
    assert_eq!(
        seen.commands[1..]
            .iter()
            .filter(|c| !c.starts_with("AUTH"))
            .collect::<Vec<_>>(),
        [
            "MAIL FROM:<me@example.com>",
            "RCPT TO:<ann@example.org>",
            "RCPT TO:<bob@example.org>",
            "DATA",
            "QUIT"
        ]
    );
    // RFC 5321 section 4.5.2: each line starting with "." gets one more.
    assert_eq!(
        seen.data.unwrap(),
        b"From: me@example.com\r\nSubject: s\r\n\r\n..hidden\r\n...two\r\n..\r\nend\r\n"
    );
}

#[test]
fn dot_stuffing_ends_every_line_in_crlf_and_the_message_with_a_dot() {
    for (mime, wire) in [
        (&b"a\n.b\nc"[..], &b"a\r\n..b\r\nc\r\n.\r\n"[..]),
        (b".\r\n", b"..\r\n.\r\n"),
        (b"", b".\r\n"),
    ] {
        assert_eq!(
            dot_stuffed(mime),
            wire,
            "{:?}",
            String::from_utf8_lossy(mime)
        );
    }
}

#[tokio::test]
async fn auth_sends_plain_or_xoauth2_as_their_specs_say() {
    let msg = message(&["ann@example.org"]);
    for (login, want) in [
        (
            Login::Password(PASSWORD),
            "PLAIN \0me@example.com\0hunter2-test-secret".to_owned(),
        ),
        (
            Login::XOAuth2 {
                access_token: TOKEN,
            },
            "XOAUTH2 user=me@example.com\x01auth=Bearer ya29.test-access-token\x01\x01".to_owned(),
        ),
    ] {
        let (result, seen) = over_pipe(Script::default(), login, &msg).await;
        result.unwrap();
        let auth = seen
            .commands
            .iter()
            .find(|c| c.starts_with("AUTH "))
            .unwrap();
        let (mechanism, encoded) = auth["AUTH ".len()..].split_once(' ').unwrap();
        let decoded = String::from_utf8(BASE64.decode(encoded).unwrap()).unwrap();
        assert_eq!(format!("{mechanism} {decoded}"), want);
    }
}

#[tokio::test]
async fn a_failed_login_sends_nothing_and_never_shows_the_secret() {
    let msg = message(&["ann@example.org"]);
    let script = Script {
        auth_fails: true,
        ..Script::default()
    };
    for login in [
        Login::Password(PASSWORD),
        Login::XOAuth2 {
            access_token: TOKEN,
        },
    ] {
        let (result, seen) = over_pipe(script.clone(), login, &msg).await;
        let err = result.unwrap_err();
        assert!(matches!(err, SmtpError::Auth { .. }), "{err:?}");
        let shown = format!("{err} {err:?}");
        assert!(shown.contains("535"), "{shown}");
        for secret in [PASSWORD, TOKEN, &BASE64.encode(PASSWORD)] {
            assert!(!shown.contains(secret), "{shown}");
        }
        assert!(
            !seen.verbs().iter().any(|v| v == "MAIL" || v == "DATA"),
            "{seen:?}"
        );
    }
}

#[tokio::test]
async fn a_refused_recipient_stops_the_message_for_everyone() {
    // Refused first, last, or in the middle: no DATA in any case.
    for refused in ["ann@example.org", "bob@example.org", "cat@example.org"] {
        let msg = message(&["ann@example.org", "bob@example.org", "cat@example.org"]);
        let script = Script {
            refuse: vec![refused],
            ..Script::default()
        };
        let (result, seen) = over_pipe(script, Login::Password(PASSWORD), &msg).await;
        let err = result.unwrap_err();
        assert!(err.to_string().contains(refused), "{err}");
        assert!(
            matches!(&err, SmtpError::Refused { refused: r } if r.len() == 1),
            "{err:?}"
        );
        assert!(!seen.verbs().contains(&"DATA".to_owned()), "{seen:?}");
        assert!(seen.data.is_none());
        assert_eq!(seen.verbs()[seen.verbs().len() - 2..], ["RSET", "QUIT"]);
    }
}

/// RFC 1047: once the final "." is out, a lost reply may hide a delivered message, so the error
/// says it may have been sent rather than that nothing was.
#[tokio::test]
async fn a_message_with_no_reply_to_its_end_may_have_been_sent() {
    let script = Script {
        hang_up_after_data: true,
        ..Script::default()
    };
    let (result, seen) = over_pipe(
        script,
        Login::Password(PASSWORD),
        &message(&["a@example.org"]),
    )
    .await;
    assert!(seen.data.is_some(), "the server got the whole message");
    let err = result.unwrap_err();
    assert!(
        matches!(&err, SmtpError::Unconfirmed(cause) if matches!(**cause, SmtpError::Closed(_))),
        "{err:?}"
    );
    assert!(err.to_string().contains("may have been sent"), "{err}");
}

#[tokio::test]
async fn a_message_refused_after_data_is_an_error() {
    let script = Script {
        data_code: 554,
        ..Script::default()
    };
    let (result, _) = over_pipe(
        script,
        Login::Password(PASSWORD),
        &message(&["a@example.org"]),
    )
    .await;
    assert!(
        matches!(
            result,
            Err(SmtpError::Rejected {
                step: "the message",
                ..
            })
        ),
        "{result:?}"
    );
}

#[tokio::test]
async fn an_address_that_could_inject_a_command_is_never_sent() {
    for bad in [
        "a@example.org>\r\nRCPT TO:<b@example.org",
        "a@example.org> NOTIFY=NEVER",
        "",
    ] {
        let (result, seen) = over_pipe(
            Script::default(),
            Login::Password(PASSWORD),
            &message(&[bad]),
        )
        .await;
        assert!(
            matches!(result, Err(SmtpError::BadAddress(_))),
            "{result:?}"
        );
        assert!(
            !seen.verbs().iter().any(|v| v == "AUTH" || v == "RCPT"),
            "{seen:?}"
        );
    }
}

/// A fake server on 127.0.0.1 for one connection, with a fresh self-signed certificate: TLS from
/// the first byte, or STARTTLS. Returns its address, its certificate's pin and what it saw.
async fn tls_server(
    security: Security,
    script: Script,
) -> (SocketAddr, CertPin, tokio::task::JoinHandle<Seen>) {
    let rcgen::CertifiedKey { cert, signing_key } =
        rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_owned()]).unwrap();
    let pin = CertPin::of(cert.der().as_ref());
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(signing_key.serialize_der().into());
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert.der().clone()], key)
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let mut seen = Seen::default();
        let (mut tcp, _) = listener.accept().await.unwrap();
        if security == Security::StartTls {
            tcp.write_all(b"220 fake ESMTP\r\n").await.unwrap();
            let mut s = BufReader::new(tcp);
            loop {
                let mut line = String::new();
                if s.read_line(&mut line).await.unwrap_or(0) == 0 {
                    return seen;
                }
                let line = line.trim_end().to_owned();
                seen.before_tls.push(line.clone());
                let reply = match line.to_ascii_uppercase().as_str() {
                    l if l.starts_with("EHLO") && script.offer_starttls => {
                        "250-fake.example\r\n250-STARTTLS\r\n250 AUTH PLAIN\r\n"
                    }
                    l if l.starts_with("EHLO") => "250-fake.example\r\n250 AUTH PLAIN\r\n",
                    "STARTTLS" if script.inject => "220 go ahead\r\n250 injected\r\n",
                    "STARTTLS" => "220 go ahead\r\n",
                    "QUIT" => "221 bye\r\n",
                    _ => "530 5.7.0 issue STARTTLS first\r\n",
                };
                s.write_all(reply.as_bytes()).await.unwrap();
                match line.to_ascii_uppercase().as_str() {
                    "STARTTLS" => break,
                    "QUIT" => return seen,
                    _ => {}
                }
            }
            tcp = s.into_inner();
        }
        let Ok(mut tls) = acceptor.accept(tcp).await else {
            return seen;
        };
        if security == Security::Tls {
            tls.write_all(b"220 fake ESMTP\r\n").await.unwrap();
        }
        session(tls, &script, &mut seen).await;
        seen
    });
    (addr, pin, handle)
}

fn config(addr: SocketAddr, security: Security, pin: CertPin) -> SmtpConfig {
    SmtpConfig {
        host: addr.ip().to_string(),
        port: addr.port(),
        security,
        username: "me@example.com".to_owned(),
        cert_pin: Some(pin),
    }
}

#[tokio::test]
async fn submit_sends_over_tls_or_starttls_with_a_pinned_certificate() {
    for security in [Security::Tls, Security::StartTls] {
        let (addr, pin, server) = tls_server(security, Script::default()).await;
        let msg = message(&["ann@example.org"]);
        submit(
            &config(addr, security, pin),
            Login::Password(PASSWORD),
            &msg,
        )
        .await
        .unwrap();
        let seen = server.await.unwrap();
        assert!(seen.data.is_some(), "{security:?}: {seen:?}");
        // Before TLS, only the greeting's EHLO and STARTTLS: never AUTH, MAIL or the message.
        let before: Vec<&str> = seen
            .before_tls
            .iter()
            .map(|l| l.split(' ').next().unwrap())
            .collect();
        let want: &[&str] = if security == Security::Tls {
            &[]
        } else {
            &["EHLO", "STARTTLS"]
        };
        assert_eq!(before, want, "{security:?}");
        assert_eq!(
            seen.verbs()[0],
            "EHLO",
            "EHLO again after STARTTLS: {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_certificate_that_does_not_match_the_pin_gets_nothing() {
    for security in [Security::Tls, Security::StartTls] {
        let (addr, _, server) = tls_server(security, Script::default()).await;
        let wrong = CertPin::of(b"another certificate");
        let result = submit(
            &config(addr, security, wrong),
            Login::Password(PASSWORD),
            &message(&["a@example.org"]),
        )
        .await;
        assert!(
            matches!(result, Err(SmtpError::Handshake { .. })),
            "{result:?}"
        );
        let seen = server.await.unwrap();
        assert!(seen.commands.is_empty(), "{seen:?}");
    }
}

#[tokio::test]
async fn without_starttls_or_with_injected_data_nothing_is_said_in_plaintext() {
    type Expected = fn(&Result<(), SmtpError>) -> bool;
    let cases: [(Script, Expected); 2] = [
        (
            Script {
                offer_starttls: false,
                ..Script::default()
            },
            |r| matches!(r, Err(SmtpError::NoStartTls)),
        ),
        (
            Script {
                inject: true,
                ..Script::default()
            },
            |r| matches!(r, Err(SmtpError::InjectedData)),
        ),
    ];
    for (script, expected) in cases {
        let (addr, pin, server) = tls_server(Security::StartTls, script.clone()).await;
        let result = submit(
            &config(addr, Security::StartTls, pin),
            Login::Password(PASSWORD),
            &message(&["a@example.org"]),
        )
        .await;
        assert!(expected(&result), "{script:?}: {result:?}");
        let seen = server.await.unwrap();
        let said = format!("{:?}", seen.before_tls).to_ascii_uppercase();
        assert!(!said.contains("AUTH") && !said.contains("MAIL"), "{said}");
        assert!(seen.commands.is_empty(), "{seen:?}");
    }
}
