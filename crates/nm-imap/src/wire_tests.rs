//! The read-only guarantee on the wire: `ImapSource` driven by the sync engine against a scripted
//! IMAP server over an in-memory pipe. It must open INBOX with EXAMINE and fetch with
//! BODY.PEEK[], and never send a command that can change the server.
#![allow(clippy::unwrap_used)] // Test helpers outside #[test] functions.

use std::num::NonZeroU32;

use async_imap::Client;
use nm_core::{Cache, sync_inbox};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

use super::{ImapSource, greeting};

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
    let session = client
        .login("user", "password")
        .await
        .map_err(|(e, _)| e)
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
