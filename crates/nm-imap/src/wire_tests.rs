//! The read-only guarantee on the wire: `ImapSource` driven by the sync engine against a scripted
//! IMAP server over an in-memory pipe. It must open INBOX with EXAMINE and fetch with
//! BODY.PEEK[], and never send a command that can change the server. The push [`Watcher`] keeps
//! the same guarantee. The one write, `store_seen`, selects INBOX, stores `\Seen` and never
//! sends a command that can remove a message.
#![allow(clippy::unwrap_used)] // Test helpers outside #[test] functions.

use std::num::NonZeroU32;
use std::time::Duration;

use async_imap::Client;
use nm_core::{Cache, sync_inbox};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

use super::{
    ImapConfig, ImapError, ImapSource, Login, Security, WatchEvent, Watcher, check_on, greeting,
    log_in, server_certificate, xoauth2_response,
};

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
    scripted_server_refusing(stream, false).await
}

/// [`scripted_server`], answering LOGIN with `NO` when `refuse_login` is set.
async fn scripted_server_refusing(stream: DuplexStream, refuse_login: bool) -> Vec<String> {
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
        if refuse_login && upper.starts_with("LOGIN") {
            let reply = format!("{tag} NO [AUTHENTICATIONFAILED] Invalid credentials\r\n");
            write.write_all(reply.as_bytes()).await.unwrap();
            continue;
        }
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
    assert_read_only(&commands);
}

/// No command in `commands` can change the server: no SELECT (which can clear \Recent), STORE,
/// COPY, MOVE, EXPUNGE or APPEND, and no fetch that would set \Seen.
fn assert_read_only(commands: &[String]) {
    assert!(!commands.is_empty());
    for c in commands.iter().map(|c| c.to_ascii_uppercase()) {
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

async fn login_check(refuse: bool) -> (Result<u32, ImapError>, Vec<String>) {
    let (client_end, server_end) = tokio::io::duplex(64 * 1024);
    let server = tokio::spawn(scripted_server_refusing(server_end, refuse));
    let mut client = Client::new(client_end);
    greeting(&mut client).await.unwrap();
    let result = check_on(client, "user", Login::Password("password")).await;
    (result, server.await.unwrap())
}

#[tokio::test]
async fn login_check_examines_inbox_and_logs_out() {
    let (result, commands) = login_check(false).await;
    assert_eq!(result.unwrap(), 2);
    let verbs: Vec<String> = commands
        .iter()
        .map(|c| {
            c.split_whitespace()
                .next()
                .unwrap_or("")
                .to_ascii_uppercase()
        })
        .collect();
    assert_eq!(verbs, ["LOGIN", "EXAMINE", "LOGOUT"], "{commands:?}");
    assert!(commands[1].contains("INBOX"), "{commands:?}");
}

#[tokio::test]
async fn login_check_reports_a_refused_login_without_the_password() {
    let (result, commands) = login_check(true).await;
    let e = result.unwrap_err();
    assert!(matches!(e, ImapError::Login { .. }), "{e:?}");
    assert_eq!(e.to_string(), "login as user failed");
    assert!(!format!("{e:?}").contains("\"password\""), "{e:?}");
    assert_eq!(
        commands.len(),
        1,
        "nothing after the refused login: {commands:?}"
    );
}

#[tokio::test]
async fn reading_the_certificate_of_a_closed_port_fails_to_connect() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let config = ImapConfig {
        host: "127.0.0.1".to_owned(),
        port,
        security: Security::StartTls,
        username: "user".to_owned(),
        cert_pin: None,
    };
    let e = server_certificate(&config).await.unwrap_err();
    assert!(matches!(e, ImapError::Connect { .. }), "{e:?}");
}

/// What the scripted IDLE server does once the watcher's first IDLE runs, after it sent the
/// script's untagged lines.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AfterIdle {
    /// Closes the connection without a word.
    Close,
    /// Says BYE, then closes.
    Bye,
    /// Answers DONE (with an EXISTS before the tagged OK), then closes after the next IDLE.
    Renew,
    /// Never answers DONE: a connection that died while idling.
    IgnoreDone,
}

struct IdleScript {
    /// Whether CAPABILITY lists IDLE.
    idle: bool,
    /// Untagged lines sent once the first IDLE is accepted.
    during: &'static [&'static str],
    after: AfterIdle,
}

/// A server with IDLE, scripted by `script`. Returns each command line (without its tag) and when
/// it arrived on the paused clock, counted from the greeting.
async fn idle_server(stream: DuplexStream, script: IdleScript) -> Vec<(String, Duration)> {
    let start = tokio::time::Instant::now();
    let (read, mut write) = tokio::io::split(stream);
    let mut lines = BufReader::new(read).lines();
    let mut seen = Vec::new();
    let mut idle_tag = String::new();
    let mut idles = 0;
    write
        .write_all(b"* OK scripted IMAP ready\r\n")
        .await
        .unwrap();
    while let Ok(Some(line)) = lines.next_line().await {
        // DONE is the one line without a tag.
        let (tag, command) = line.split_once(' ').unwrap_or(("", &line));
        seen.push((command.to_owned(), start.elapsed()));
        let upper = command.to_ascii_uppercase();
        let reply = if upper == "DONE" {
            if script.after == AfterIdle::IgnoreDone {
                continue;
            }
            format!("* 4 EXISTS\r\n{idle_tag} OK IDLE terminated\r\n")
        } else if upper == "IDLE" {
            idles += 1;
            idle_tag = tag.to_owned();
            write.write_all(b"+ idling\r\n").await.unwrap();
            if idles > 1 {
                break;
            }
            for untagged in script.during {
                write
                    .write_all(format!("{untagged}\r\n").as_bytes())
                    .await
                    .unwrap();
            }
            match script.after {
                AfterIdle::Close => break,
                AfterIdle::Bye => {
                    write.write_all(b"* BYE shutting down\r\n").await.unwrap();
                    break;
                }
                AfterIdle::Renew | AfterIdle::IgnoreDone => continue,
            }
        } else if upper.starts_with("LOGIN") {
            format!("{tag} OK logged in\r\n")
        } else if upper == "CAPABILITY" {
            let idle = if script.idle { " IDLE" } else { "" };
            format!("* CAPABILITY IMAP4rev1{idle}\r\n{tag} OK done\r\n")
        } else if upper.starts_with("EXAMINE") {
            format!("* 2 EXISTS\r\n* OK [UIDVALIDITY 7] ok\r\n{tag} OK [READ-ONLY] done\r\n")
        } else if upper == "LOGOUT" {
            write
                .write_all(format!("* BYE bye\r\n{tag} OK done\r\n").as_bytes())
                .await
                .unwrap();
            break;
        } else {
            format!("{tag} BAD not scripted\r\n")
        };
        write.write_all(reply.as_bytes()).await.unwrap();
    }
    seen
}

struct Watched {
    /// Why the watcher ended; `None` when there was no watcher.
    ended: Option<ImapError>,
    events: Vec<WatchEvent>,
    /// Each command's verb, uppercased, and when it arrived.
    commands: Vec<(String, Duration)>,
    /// How long the watcher ran, on the paused clock.
    ran: Duration,
}

/// Starts a watcher against [`idle_server`] and runs it until it ends. Every command it sent must
/// be read-only.
async fn watch(script: IdleScript) -> Watched {
    let (client_end, server_end) = tokio::io::duplex(64 * 1024);
    let server = tokio::spawn(idle_server(server_end, script));
    let mut client = Client::new(client_end);
    greeting(&mut client).await.unwrap();
    let watcher = Watcher::start(client, "user", Login::Password("password"))
        .await
        .unwrap();
    let mut events = Vec::new();
    let started = tokio::time::Instant::now();
    let ended = match watcher {
        Some(watcher) => Some(watcher.run(|e| events.push(e)).await),
        None => None,
    };
    let ran = started.elapsed();
    let seen = server.await.unwrap();
    assert_read_only(&seen.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>());
    let examines_inbox = |c: &String| c.starts_with("EXAMINE") && c.contains("INBOX");
    assert!(
        ended.is_none() || seen.iter().any(|(c, _)| examines_inbox(c)),
        "{seen:?}"
    );
    let commands = seen
        .into_iter()
        .map(|(c, at)| (c.split(' ').next().unwrap_or("").to_ascii_uppercase(), at))
        .collect();
    Watched {
        ended,
        events,
        commands,
        ran,
    }
}

fn verbs(watched: &Watched) -> Vec<&str> {
    watched.commands.iter().map(|(c, _)| c.as_str()).collect()
}

#[tokio::test(start_paused = true)]
async fn watcher_examines_inbox_then_idles_and_wakes_on_each_change() {
    let watched = watch(IdleScript {
        idle: true,
        // A keep-alive and RECENT are not changes; EXISTS, EXPUNGE and FETCH are.
        during: &[
            "* OK still here",
            "* 3 EXISTS",
            "* 1 RECENT",
            "* 1 EXPUNGE",
            "* 2 FETCH (FLAGS (\\Seen))",
        ],
        after: AfterIdle::Close,
    })
    .await;
    assert_eq!(verbs(&watched), ["LOGIN", "CAPABILITY", "EXAMINE", "IDLE"]);
    use WatchEvent::{Changed, Idling};
    assert_eq!(watched.events, [Idling, Changed, Changed, Changed]);
    let ended = watched.ended.unwrap();
    assert!(matches!(ended, ImapError::Closed), "{ended:?}");
}

#[tokio::test(start_paused = true)]
async fn watcher_renews_idle_after_29_minutes_on_the_same_connection() {
    let watched = watch(IdleScript {
        idle: true,
        during: &[],
        after: AfterIdle::Renew,
    })
    .await;
    assert_eq!(
        verbs(&watched),
        ["LOGIN", "CAPABILITY", "EXAMINE", "IDLE", "DONE", "IDLE"]
    );
    let at = |i: usize| watched.commands[i].1;
    assert_eq!(at(4) - at(3), Duration::from_secs(29 * 60), "IDLE to DONE");
    assert_eq!(at(5), at(4), "IDLE again at once");
    // The EXISTS the server sent before DONE's OK wakes too.
    assert_eq!(watched.events, [WatchEvent::Idling, WatchEvent::Changed]);
    assert!(
        matches!(watched.ended, Some(ImapError::Closed)),
        "{:?}",
        watched.ended
    );
}

#[tokio::test(start_paused = true)]
async fn a_server_that_never_answers_done_ends_the_watcher_by_its_deadline() {
    let watched = watch(IdleScript {
        idle: true,
        during: &[],
        after: AfterIdle::IgnoreDone,
    })
    .await;
    assert_eq!(verbs(&watched).last(), Some(&"DONE"));
    assert!(
        matches!(watched.ended, Some(ImapError::Timeout("DONE"))),
        "{:?}",
        watched.ended
    );
    // 29 minutes of IDLE, then at most a minute for DONE's reply: not TCP's own timeout.
    assert!(
        watched.ran <= Duration::from_secs(30 * 60),
        "{:?}",
        watched.ran
    );
    assert_eq!(watched.events, [WatchEvent::Idling]);
}

#[tokio::test(start_paused = true)]
async fn bye_ends_the_watcher() {
    let watched = watch(IdleScript {
        idle: true,
        during: &[],
        after: AfterIdle::Bye,
    })
    .await;
    let ended = watched.ended.unwrap();
    assert!(
        matches!(&ended, ImapError::Bye(text) if text == "shutting down"),
        "{ended:?}"
    );
    assert_eq!(watched.ran, Duration::ZERO, "no wait for a deadline");
}

#[tokio::test(start_paused = true)]
async fn a_server_without_idle_gets_no_watcher() {
    let watched = watch(IdleScript {
        idle: false,
        during: &[],
        after: AfterIdle::Close,
    })
    .await;
    assert!(watched.ended.is_none());
    assert_eq!(verbs(&watched), ["LOGIN", "CAPABILITY", "LOGOUT"]);
}

/// A server for the write side: SELECT reports UIDVALIDITY 7, and `answer` gives the untagged
/// responses to `UID STORE` and `UID FETCH` (by their command line). Returns every command line.
async fn store_server(
    stream: DuplexStream,
    answer: impl Fn(&str) -> String + Send + 'static,
) -> Vec<String> {
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
        seen.push(command.to_owned());
        let untagged = if upper.starts_with("LOGIN") {
            String::new()
        } else if upper.starts_with("SELECT") {
            "* 3 EXISTS\r\n* OK [UIDVALIDITY 7] ok\r\n".to_owned()
        } else if upper.starts_with("UID STORE") || upper.starts_with("UID FETCH") {
            answer(&upper)
        } else if upper.starts_with("LOGOUT") {
            "* BYE logging out\r\n".to_owned()
        } else {
            let reply = format!("{tag} BAD not scripted\r\n");
            write.write_all(reply.as_bytes()).await.unwrap();
            continue;
        };
        let reply = format!("{untagged}{tag} OK done\r\n");
        write.write_all(reply.as_bytes()).await.unwrap();
        if upper.starts_with("LOGOUT") {
            break;
        }
    }
    seen
}

/// Runs `store_seen` against [`store_server`] and returns its result and the commands sent.
async fn store(
    validity: u32,
    uids: &[u32],
    seen: bool,
    answer: impl Fn(&str) -> String + Send + 'static,
) -> (
    Result<nm_core::Written<nm_core::Uid>, ImapError>,
    Vec<String>,
) {
    let (client_end, server_end) = tokio::io::duplex(64 * 1024);
    let server = tokio::spawn(store_server(server_end, answer));
    let mut client = Client::new(client_end);
    greeting(&mut client).await.unwrap();
    let session = log_in(client, "user", Login::Password("password"))
        .await
        .unwrap();
    let uids: Vec<nm_core::Uid> = uids
        .iter()
        .map(|&u| nm_core::Uid::new(u).unwrap())
        .collect();
    let validity = nm_core::UidValidity::new(validity).unwrap();
    let result = super::store_seen_on(session, validity, &uids, seen).await;
    (result, server.await.unwrap())
}

/// Nothing that can remove a message: no EXPUNGE, no CLOSE (it expunges, RFC 3501 6.4.2), no
/// MOVE, COPY or `\Deleted`.
fn assert_never_removes(commands: &[String]) {
    assert!(!commands.is_empty());
    for c in commands.iter().map(|c| c.to_ascii_uppercase()) {
        for forbidden in ["EXPUNGE", "CLOSE", "MOVE", "COPY", "DELETED"] {
            assert!(!c.contains(forbidden), "sent {forbidden}: {c}");
        }
    }
}

fn uid_flags(written: &nm_core::Written<nm_core::Uid>) -> Vec<(u32, Vec<String>)> {
    written
        .done
        .iter()
        .map(|(uid, flags)| {
            let flags = flags.as_ref().unwrap().iter().map(str::to_owned).collect();
            (uid.get(), flags)
        })
        .collect()
}

#[tokio::test]
async fn mark_read_selects_inbox_stores_plus_seen_and_takes_the_server_s_flags() {
    let (written, commands) = store(7, &[1, 2], true, |_| {
        "* 1 FETCH (UID 1 FLAGS (\\Seen \\Flagged))\r\n* 2 FETCH (UID 2 FLAGS (\\Seen))\r\n"
            .to_owned()
    })
    .await;
    let written = written.unwrap();
    assert_eq!(
        uid_flags(&written),
        [
            (1, vec!["\\Flagged".to_owned(), "\\Seen".to_owned()]),
            (2, vec!["\\Seen".to_owned()])
        ]
    );
    assert!(written.gone.is_empty());
    assert_eq!(
        commands,
        [
            "LOGIN \"user\" \"password\"",
            "SELECT \"INBOX\"",
            "UID STORE 1,2 +FLAGS (\\Seen)",
            "LOGOUT"
        ]
    );
    assert_never_removes(&commands);
}

#[tokio::test]
async fn mark_unread_stores_minus_seen() {
    let (written, commands) = store(7, &[3], false, |_| {
        "* 3 FETCH (UID 3 FLAGS ())\r\n".to_owned()
    })
    .await;
    assert_eq!(uid_flags(&written.unwrap()), [(3, Vec::<String>::new())]);
    assert_eq!(commands[2], "UID STORE 3 -FLAGS (\\Seen)");
    assert_never_removes(&commands);
}

#[tokio::test]
async fn another_uidvalidity_stores_nothing_and_says_to_sync() {
    let (written, commands) = store(8, &[1], true, |_| panic!("nothing is stored")).await;
    let e = written.unwrap_err();
    assert!(matches!(e, ImapError::UidValidityChanged), "{e:?}");
    assert!(e.to_string().contains("neomail sync"), "{e}");
    assert!(
        !commands
            .iter()
            .any(|c| c.to_ascii_uppercase().contains("STORE")),
        "{commands:?}"
    );
    assert_eq!(commands.last().unwrap(), "LOGOUT");
    assert_never_removes(&commands);
}

/// The server answers STORE for UID 1 only; a UID FETCH finds 2 (already read, so no FETCH
/// came back for it) but not 3, which is gone.
#[tokio::test]
async fn a_uid_the_server_no_longer_has_is_gone() {
    let (written, commands) = store(7, &[1, 2, 3], true, |command| {
        if command.starts_with("UID STORE") {
            // An unsolicited FETCH for a message not asked about is ignored.
            "* 1 FETCH (UID 1 FLAGS (\\Seen))\r\n* 9 FETCH (UID 9 FLAGS ())\r\n".to_owned()
        } else {
            "* 2 FETCH (UID 2 FLAGS (\\Seen))\r\n".to_owned()
        }
    })
    .await;
    let written = written.unwrap();
    assert_eq!(
        uid_flags(&written),
        [
            (1, vec!["\\Seen".to_owned()]),
            (2, vec!["\\Seen".to_owned()])
        ]
    );
    assert_eq!(written.gone, [nm_core::Uid::new(3).unwrap()]);
    assert_eq!(commands[3], "UID FETCH 2,3 (UID FLAGS)");
    assert_never_removes(&commands);
}
