//! The read-only guarantee on the wire: `ImapSource` driven by the sync engine against a scripted
//! IMAP server over an in-memory pipe. It must open INBOX with EXAMINE and fetch with
//! BODY.PEEK[], and never send a command that can change the server.
#![allow(clippy::unwrap_used)] // Test helpers outside #[test] functions.

use std::num::NonZeroU32;

use async_imap::Client;
use nm_core::{Cache, sync_inbox};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

use super::{ImapError, ImapSource, Login, greeting, log_in, xoauth2_response};

/// The FETCH response for one whole, unread message.
fn message(uid: u32) -> String {
    let body = format!("From: a@mail.example\r\nSubject: Message {uid}\r\n\r\nBody {uid}\r\n");
    format!(
        "* {uid} FETCH (UID {uid} FLAGS () INTERNALDATE \"10-Oct-2026 09:30:00 +0000\" BODY[] {{{}}}\r\n{body})\r\n",
        body.len()
    )
}

/// Answers each command like a small server would, and returns every command line it got.
async fn scripted_server(stream: DuplexStream) -> Vec<String> {
    let (read, mut write) = tokio::io::split(stream);
    let mut lines = BufReader::new(read).lines();
    let mut seen = Vec::new();
    write
        .write_all(b"* OK scripted IMAP ready\r\n")
        .await
        .unwrap();
    while let Ok(Some(line)) = lines.next_line().await {
        let (tag, command) = line.split_once(' ').unwrap_or((&line, ""));
        let upper = command.to_ascii_uppercase();
        let reply = if upper.starts_with("LOGIN") {
            Some(String::new())
        } else if upper.starts_with("EXAMINE") {
            Some("* 2 EXISTS\r\n* OK [UIDVALIDITY 7] ok\r\n".to_owned())
        } else if upper.starts_with("FETCH 1:*") {
            "* 1 FETCH (UID 1 FLAGS () INTERNALDATE \"10-Oct-2026 09:30:00 +0000\")\r\n\
             * 2 FETCH (UID 2 FLAGS () INTERNALDATE \"10-Oct-2026 09:31:00 +0000\")\r\n"
                .to_owned()
                .into()
        } else if upper.starts_with("UID FETCH") && upper.contains("BODY.PEEK[]") {
            Some(message(1) + &message(2))
        } else if upper.starts_with("UID FETCH") {
            Some("* 1 FETCH (UID 1 FLAGS ())\r\n* 2 FETCH (UID 2 FLAGS ())\r\n".to_owned())
        } else if upper.starts_with("LOGOUT") {
            Some("* BYE logging out\r\n".to_owned())
        } else {
            None
        };
        seen.push(command.to_owned());
        let done = upper.starts_with("LOGOUT");
        let reply = match reply {
            Some(untagged) => format!("{untagged}{tag} OK done\r\n"),
            None => format!("{tag} BAD not scripted\r\n"),
        };
        write.write_all(reply.as_bytes()).await.unwrap();
        if done {
            break;
        }
    }
    seen
}

#[tokio::test]
async fn sync_sends_only_read_only_commands() {
    let (client_end, server_end) = tokio::io::duplex(64 * 1024);
    let server = tokio::spawn(scripted_server(server_end));

    let mut client = Client::new(client_end);
    greeting(&mut client).await.unwrap();
    let session = log_in(client, "user", Login::Password("password"))
        .await
        .unwrap();
    let mut source = ImapSource { session, exists: 0 };

    let mut cache = Cache::open_in_memory().unwrap();
    let limit = NonZeroU32::new(10).unwrap();
    let first = sync_inbox(&mut source, &mut cache, "work", limit)
        .await
        .unwrap();
    assert_eq!(first.new, 2);
    // The second sync lists flags with UID FETCH and fetches no bodies.
    let second = sync_inbox(&mut source, &mut cache, "work", limit)
        .await
        .unwrap();
    assert_eq!((second.new, second.missing), (0, 0));
    source.logout().await;

    let commands = server.await.unwrap();
    let upper: Vec<String> = commands.iter().map(|c| c.to_ascii_uppercase()).collect();
    assert_eq!(
        upper.iter().filter(|c| c.starts_with("EXAMINE")).count(),
        2,
        "{commands:?}"
    );
    assert!(
        upper.iter().any(|c| c.contains("BODY.PEEK[]")),
        "{commands:?}"
    );
    for c in &upper {
        let verb = c.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
        for forbidden in ["SELECT", "STORE", "COPY", "MOVE", "EXPUNGE", "APPEND"] {
            assert!(
                !verb.split(' ').any(|w| w == forbidden),
                "sent {forbidden}: {c}"
            );
        }
        assert!(!c.contains("BODY[]"), "a fetch without PEEK: {c}");
        assert!(!c.contains("+FLAGS") && !c.contains("-FLAGS"), "{c}");
    }
}

const TOKEN: &str = "ya29.not-a-real-token";

/// Gmail's side of `AUTHENTICATE XOAUTH2`: an empty challenge, then `OK` for the right token or,
/// for any other, the base64 JSON error challenge, an empty client response, and `NO`. Returns
/// the decoded client responses.
async fn xoauth2_server(stream: DuplexStream) -> Vec<String> {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD;
    let (read, mut write) = tokio::io::split(stream);
    let mut lines = BufReader::new(read).lines();
    write.write_all(b"* OK Gimap ready\r\n").await.unwrap();
    let line = lines.next_line().await.unwrap().unwrap();
    let (tag, command) = line.split_once(' ').unwrap();
    assert_eq!(command, "AUTHENTICATE XOAUTH2");
    write.write_all(b"+ \r\n").await.unwrap();
    let first = lines.next_line().await.unwrap().unwrap();
    let decoded = String::from_utf8(b64.decode(&first).unwrap()).unwrap();
    let mut responses = vec![decoded.clone()];
    if decoded == xoauth2_response("user@gmail.example", TOKEN) {
        let reply = format!("* CAPABILITY IMAP4rev1\r\n{tag} OK user authenticated\r\n");
        write.write_all(reply.as_bytes()).await.unwrap();
    } else {
        let json = r#"{"status":"400","schemes":"Bearer","scope":"https://mail.google.com/"}"#;
        let challenge = format!("+ {}\r\n", b64.encode(json));
        write.write_all(challenge.as_bytes()).await.unwrap();
        responses.push(lines.next_line().await.unwrap().unwrap());
        let reply = format!("{tag} NO [AUTHENTICATIONFAILED] Invalid credentials (Failure)\r\n");
        write.write_all(reply.as_bytes()).await.unwrap();
    }
    responses
}

async fn xoauth2_login(token: &str) -> (Result<(), ImapError>, Vec<String>) {
    let (client_end, server_end) = tokio::io::duplex(64 * 1024);
    let server = tokio::spawn(xoauth2_server(server_end));
    let mut client = Client::new(client_end);
    greeting(&mut client).await.unwrap();
    let login = Login::XOAuth2 {
        access_token: token,
    };
    let result = log_in(client, "user@gmail.example", login).await.map(drop);
    (result, server.await.unwrap())
}

#[test]
fn xoauth2_response_is_googles_format() {
    assert_eq!(
        xoauth2_response(
            "someuser@example.com",
            "ya29.vF9dft4qmTc2Nvb3RlckBhdHRhdmlzdGEuY29tCg"
        ),
        "user=someuser@example.com\x01auth=Bearer ya29.vF9dft4qmTc2Nvb3RlckBhdHRhdmlzdGEuY29tCg\x01\x01"
    );
    let login = Login::XOAuth2 {
        access_token: TOKEN,
    };
    assert!(!format!("{login:?}").contains(TOKEN));
    assert!(!format!("{:?}", Login::Password(TOKEN)).contains(TOKEN));
}

#[tokio::test]
async fn xoauth2_success() {
    let (result, responses) = xoauth2_login(TOKEN).await;
    result.unwrap();
    assert_eq!(responses, [xoauth2_response("user@gmail.example", TOKEN)]);
}

#[tokio::test]
async fn xoauth2_failure_answers_the_json_challenge_with_an_empty_line() {
    let (result, responses) = xoauth2_login("ya29.expired").await;
    let e = result.unwrap_err();
    assert!(matches!(e, ImapError::Login { .. }), "{e:?}");
    assert_eq!(responses.len(), 2, "{responses:?}");
    assert_eq!(responses[1], "", "the empty SASL response");
    let message = format!("{e} {e:?}");
    assert!(!message.contains("ya29.expired"), "{message}");
}
