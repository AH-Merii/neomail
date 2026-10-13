//! `neomail send`, `reply` and `forward` through the binary, against a fake SMTP server on
//! 127.0.0.1 that records what it is told. Nothing leaves this machine.
#![allow(clippy::unwrap_used)] // Test helpers outside #[test] functions.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};

use nm_core::{Cache, FetchedMessage, Flags, Uid, UidValidity};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// What the fake SMTP server was told.
#[derive(Default, Debug)]
struct Seen {
    connections: usize,
    /// Each command line, without its CRLF; AUTH as its verb and mechanism alone.
    commands: Vec<String>,
    /// Each message received after DATA, dot-unstuffed.
    messages: Vec<String>,
}

/// An SMTP server on 127.0.0.1 with TLS from the first byte and a fresh self-signed certificate:
/// it accepts any login and refuses the recipients in `refuse` with 550.
struct FakeSmtp {
    addr: SocketAddr,
    /// The certificate's SHA-256, as the config's `cert_sha256` takes it.
    pin: String,
    seen: Arc<Mutex<Seen>>,
}

impl FakeSmtp {
    fn start(refuse: &[&str]) -> Self {
        Self::start_with(refuse, false)
    }

    /// With `hang_up`, it closes the connection after a message's final "." instead of answering.
    fn start_with(refuse: &[&str], hang_up: bool) -> Self {
        let rcgen::CertifiedKey { cert, signing_key } =
            rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_owned()]).unwrap();
        let pin: String = Sha256::digest(cert.der().as_ref())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let key = rustls::pki_types::PrivateKeyDer::Pkcs8(signing_key.serialize_der().into());
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = Arc::new(
            rustls::ServerConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(vec![cert.der().clone()], key)
                .unwrap(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Seen::default()));
        let refuse: Vec<String> = refuse.iter().map(|r| (*r).to_owned()).collect();
        let log = Arc::clone(&seen);
        std::thread::spawn(move || {
            for tcp in listener.incoming() {
                let Ok(tcp) = tcp else { return };
                log.lock().unwrap().connections += 1;
                let tls = rustls::ServerConnection::new(Arc::clone(&config)).unwrap();
                let stream = rustls::StreamOwned::new(tls, tcp);
                serve(BufReader::new(stream), &refuse, hang_up, &log);
            }
        });
        Self { addr, pin, seen }
    }

    fn seen(&self) -> std::sync::MutexGuard<'_, Seen> {
        self.seen.lock().unwrap()
    }
}

/// One SMTP session; it ends at QUIT, at the end of input or at an I/O error.
fn serve<S: Read + Write>(
    mut s: BufReader<S>,
    refuse: &[String],
    hang_up: bool,
    log: &Mutex<Seen>,
) {
    let reply = |s: &mut BufReader<S>, text: &str| s.get_mut().write_all(text.as_bytes()).is_ok();
    if !reply(&mut s, "220 fake ESMTP\r\n") {
        return;
    }
    loop {
        let mut line = String::new();
        if s.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let line = line.trim_end().to_owned();
        let verb = line
            .split(' ')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        let logged = match verb.as_str() {
            "AUTH" => line.split(' ').take(2).collect::<Vec<_>>().join(" "),
            _ => line.clone(),
        };
        log.lock().unwrap().commands.push(logged);
        let answer = match verb.as_str() {
            "EHLO" => "250-fake\r\n250 AUTH PLAIN XOAUTH2\r\n".to_owned(),
            "AUTH" => "235 ok\r\n".to_owned(),
            "RCPT" if refuse.iter().any(|r| line.contains(&format!("<{r}>"))) => {
                "550 no such user\r\n".to_owned()
            }
            "DATA" => {
                if !reply(&mut s, "354 go on\r\n") {
                    return;
                }
                let mut message = String::new();
                loop {
                    let mut l = String::new();
                    if s.read_line(&mut l).unwrap_or(0) == 0 {
                        return;
                    }
                    if l == ".\r\n" {
                        break;
                    }
                    message.push_str(l.strip_prefix('.').unwrap_or(&l));
                }
                log.lock().unwrap().messages.push(message);
                if hang_up {
                    return;
                }
                "250 queued\r\n".to_owned()
            }
            "QUIT" => {
                reply(&mut s, "221 bye\r\n");
                return;
            }
            _ => "250 ok\r\n".to_owned(),
        };
        if !reply(&mut s, &answer) {
            return;
        }
    }
}

/// A listener that counts connections and closes each at once: an IMAP server that fails every
/// login, so an append to Sent fails.
fn closed_imap() -> (SocketAddr, Arc<Mutex<usize>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let count = Arc::new(Mutex::new(0));
    let counter = Arc::clone(&count);
    std::thread::spawn(move || {
        for tcp in listener.incoming() {
            *counter.lock().unwrap() += 1;
            drop(tcp);
        }
    });
    (addr, count)
}

/// One password account "work" (me@example.com) whose SMTP is `smtp` and IMAP `imap`.
fn account(
    name: &str,
    address: &str,
    smtp: &FakeSmtp,
    imap: SocketAddr,
    save_sent: bool,
) -> String {
    format!(
        "[[accounts]]\nname = \"{name}\"\nusername = \"{address}\"\ndisplay_name = \"Me\"\n\
         password_command = \"echo test\"\n\
         [accounts.imap]\nhost = \"127.0.0.1\"\nport = {}\ncert_sha256 = \"{}\"\n\
         [accounts.smtp]\nhost = \"127.0.0.1\"\nport = {}\nsecurity = \"tls\"\n\
         cert_sha256 = \"{}\"\nsave_sent = {save_sent}\n",
        imap.port(),
        smtp.pin,
        smtp.addr.port(),
        smtp.pin
    )
}

/// A data directory whose config holds `config`.
fn setup(config: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"), config).unwrap();
    dir
}

/// Runs the binary with its config and data in `data`, standard input `stdin` (not a terminal),
/// a test keyring and none of the caller's settings.
fn neomail(data: &Path, args: &[&str], stdin: &str) -> Output {
    neomail_to(data, args, stdin, Stdio::piped())
}

/// [`neomail`] with standard output on `stdout`.
fn neomail_to(data: &Path, args: &[&str], stdin: &str, stdout: Stdio) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_neomail"));
    c.args(args)
        .env("NEOMAIL_DATA_DIR", data)
        .env("NEOMAIL_CONFIG", data.join("config.toml"))
        .env("NEOMAIL_TEST_KEYRING", data.join("keyring.json"))
        .env("HOME", data)
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent")
        .env("BROWSER", "true")
        .stdin(Stdio::piped())
        .stdout(stdout)
        .stderr(Stdio::piped());
    for var in [
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "NEOMAIL_NO_INPUT",
        "NO_COLOR",
    ] {
        c.env_remove(var);
    }
    let mut child = c.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// The value of header `name` in `message`, unfolded.
fn header(message: &str, name: &str) -> Option<String> {
    let head = message.split("\r\n\r\n").next().unwrap_or_default();
    let unfolded = head.replace("\r\n ", " ").replace("\r\n\t", " ");
    unfolded.split("\r\n").find_map(|l| {
        let (key, value) = l.split_once(':')?;
        key.eq_ignore_ascii_case(name)
            .then(|| value.trim().to_owned())
    })
}

const BODY: &str = "# Plans\n\n**Bold**, a [link](https://example.com) and <b>raw</b>.\n";

/// Off a terminal only --yes sends: not a "y" piped on standard input, whether or not the body
/// was read from it, for each of the commands that send.
#[test]
fn nothing_is_sent_without_a_yes_off_a_terminal() {
    let smtp = FakeSmtp::start(&[]);
    let (imap, _) = closed_imap();
    let dir = setup(&account("work", "me@example.com", &smtp, imap, false));
    let note = dir.path().join("note.md");
    std::fs::write(&note, BODY).unwrap();
    let note = note.to_str().unwrap();
    let id = cache(dir.path(), "work", ORIGINAL);
    let send = ["send", "--to", "ada@example.com", "--subject", "Hi"];
    let cases: [(Vec<&str>, &str); 6] = [
        ([&send[..], &["--body", "-"]].concat(), BODY),
        ([&send[..], &["--body", "-", "--no-input"]].concat(), BODY),
        ([&send[..], &["--body", note]].concat(), "y\n"),
        ([&send[..], &["--body", note]].concat(), "yes\n"),
        (vec!["reply", &id, "--body", note], "y\n"),
        (vec!["forward", &id, "--to", "cy@example.com"], "y\n"),
    ];
    for (args, stdin) in cases {
        let o = neomail(dir.path(), &args, stdin);
        assert_eq!(o.status.code(), Some(2), "{args:?}: {}", stderr(&o));
        assert!(stderr(&o).contains("nothing was sent"), "{}", stderr(&o));
        assert!(stderr(&o).contains("--yes"), "{}", stderr(&o));
        assert!(o.stdout.is_empty());
    }
    assert_eq!(smtp.seen().connections, 0);
}

#[test]
fn a_missing_flag_is_a_usage_error_that_names_it() {
    let smtp = FakeSmtp::start(&[]);
    let (imap, _) = closed_imap();
    let dir = setup(&account("work", "me@example.com", &smtp, imap, false));
    let full = ["--to", "a@example.com", "--subject", "Hi", "--body", "-"];
    for (i, flag) in ["--to", "--subject", "--body"].into_iter().enumerate() {
        let mut args = vec!["send", "--yes"];
        args.extend(
            full.iter()
                .enumerate()
                .filter(|(j, _)| j / 2 != i)
                .map(|(_, a)| *a),
        );
        let o = neomail(dir.path(), &args, BODY);
        assert_eq!(o.status.code(), Some(2), "{flag}: {}", stderr(&o));
        assert!(stderr(&o).contains(flag), "{flag}: {}", stderr(&o));
    }
    assert_eq!(smtp.seen().connections, 0);
}

#[test]
fn a_dry_run_prints_the_message_and_connects_to_nothing() {
    let smtp = FakeSmtp::start(&[]);
    let (imap, imap_connections) = closed_imap();
    let dir = setup(&account("work", "me@example.com", &smtp, imap, true));
    let o = neomail(
        dir.path(),
        &[
            "send",
            "--to",
            "Ada <ada@example.com>",
            "--bcc",
            "eve@example.com",
            "--subject",
            "Hi",
            "--body",
            "-",
            "--dry-run",
        ],
        BODY,
    );
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let message = stdout(&o);
    assert_eq!(
        header(&message, "To").as_deref(),
        Some("\"Ada\" <ada@example.com>")
    );
    assert_eq!(
        header(&message, "From").as_deref(),
        Some("\"Me\" <me@example.com>")
    );
    assert!(!message.contains("eve@example.com"), "{message}");
    // No header names Bcc, so the summary on standard error is where the dry run shows it.
    let summary = stderr(&o);
    assert!(
        summary
            .lines()
            .any(|l| l.starts_with("Bcc:") && l.contains("eve@example.com")),
        "{summary}"
    );
    assert_eq!(smtp.seen().connections, 0);
    assert_eq!(*imap_connections.lock().unwrap(), 0);
}

#[test]
fn a_sent_message_goes_to_every_recipient_and_no_header_names_bcc() {
    let smtp = FakeSmtp::start(&[]);
    let (imap, imap_connections) = closed_imap();
    let dir = setup(&account("work", "me@example.com", &smtp, imap, false));
    let o = neomail(
        dir.path(),
        &[
            "send",
            "--to",
            "ada@example.com,bob@example.com",
            "--cc",
            "cy@example.com",
            "--bcc",
            "eve@example.com",
            "--subject",
            "Plans",
            "--body",
            "-",
            "--yes",
            "--json",
        ],
        BODY,
    );
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let json: Value = serde_json::from_str(&stdout(&o)).unwrap();
    let seen = smtp.seen();
    let rcpts: Vec<&str> = seen
        .commands
        .iter()
        .filter(|c| c.starts_with("RCPT"))
        .map(String::as_str)
        .collect();
    assert_eq!(
        rcpts,
        [
            "RCPT TO:<ada@example.com>",
            "RCPT TO:<bob@example.com>",
            "RCPT TO:<cy@example.com>",
            "RCPT TO:<eve@example.com>"
        ]
    );
    assert!(
        seen.commands
            .contains(&"MAIL FROM:<me@example.com>".to_owned())
    );
    assert_eq!(seen.messages.len(), 1);
    let message = &seen.messages[0];
    assert!(!message.contains("eve@example.com"), "{message}");
    assert!(
        message.contains("**Bold**"),
        "the Markdown source is in the message: {message}"
    );
    assert!(
        // Quoted-printable soft line breaks undone.
        message
            .replace("=\r\n", "")
            .contains("&lt;b&gt;raw&lt;/b&gt;"),
        "raw HTML is escaped: {message}"
    );
    let id = header(message, "Message-ID").unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "account": "work",
            "from": {"name": "Me", "address": "me@example.com"},
            "to": [
                {"name": null, "address": "ada@example.com"},
                {"name": null, "address": "bob@example.com"}
            ],
            "cc": [{"name": null, "address": "cy@example.com"}],
            "bcc": [{"name": null, "address": "eve@example.com"}],
            "subject": "Plans",
            "message_id": id.trim_start_matches('<').trim_end_matches('>'),
            "in_reply_to": null,
            "sent_copy": "server",
        })
    );
    // The server keeps the copy in Sent: neomail adds none.
    assert_eq!(*imap_connections.lock().unwrap(), 0);
}

#[test]
fn a_refused_recipient_stops_the_message_for_everyone() {
    let smtp = FakeSmtp::start(&["bob@example.com"]);
    let (imap, imap_connections) = closed_imap();
    let dir = setup(&account("work", "me@example.com", &smtp, imap, true));
    let o = neomail(
        dir.path(),
        &[
            "send",
            "--to",
            "ada@example.com,bob@example.com",
            "--subject",
            "Hi",
            "--body",
            "-",
            "--yes",
        ],
        BODY,
    );
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    assert!(stderr(&o).contains("bob@example.com"), "{}", stderr(&o));
    assert!(stderr(&o).contains("nothing was sent"), "{}", stderr(&o));
    let seen = smtp.seen();
    assert!(
        !seen.commands.iter().any(|c| c == "DATA"),
        "{:?}",
        seen.commands
    );
    assert!(seen.messages.is_empty());
    assert_eq!(
        *imap_connections.lock().unwrap(),
        0,
        "nothing to copy to Sent"
    );
}

/// RFC 1047: the server got the whole message but never answered it, so it may have been
/// delivered. The command fails, says so, and stores no copy in Sent.
#[test]
fn a_message_the_server_never_confirmed_fails_saying_it_may_have_been_sent() {
    let smtp = FakeSmtp::start_with(&[], true);
    let (imap, imap_connections) = closed_imap();
    let dir = setup(&account("work", "me@example.com", &smtp, imap, true));
    let args = [
        "send",
        "--to",
        "ada@example.com",
        "--subject",
        "Hi",
        "--body",
        "-",
        "--yes",
    ];
    let o = neomail(dir.path(), &args, BODY);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    assert!(stderr(&o).contains("may have been sent"), "{}", stderr(&o));
    assert!(!stderr(&o).contains("nothing was sent"), "{}", stderr(&o));
    assert_eq!(smtp.seen().messages.len(), 1);
    assert_eq!(*imap_connections.lock().unwrap(), 0);
}

#[test]
fn a_copy_in_sent_that_fails_still_exits_0_with_a_warning() {
    let smtp = FakeSmtp::start(&[]);
    let (imap, imap_connections) = closed_imap();
    let dir = setup(&account("work", "me@example.com", &smtp, imap, true));
    let o = neomail(
        dir.path(),
        &[
            "send",
            "--to",
            "ada@example.com",
            "--subject",
            "Hi",
            "--body",
            "-",
            "--yes",
            "--json",
        ],
        BODY,
    );
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let json: Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(json["sent_copy"], "failed");
    assert!(
        stderr(&o).contains("could not be stored in Sent"),
        "{}",
        stderr(&o)
    );
    assert_eq!(smtp.seen().messages.len(), 1);
    assert_eq!(*imap_connections.lock().unwrap(), 1);
}

/// Once the server has the message, standard output that cannot be written (here /dev/full, so
/// ENOSPC) is a warning naming the message, and the exit is 0: a script that retries on exit 1
/// must not send it twice. Linux only: macOS has no /dev/full.
#[cfg(target_os = "linux")]
#[test]
fn a_sent_message_whose_output_cannot_be_written_still_exits_0() {
    let smtp = FakeSmtp::start(&[]);
    let (imap, _) = closed_imap();
    let dir = setup(&account("work", "me@example.com", &smtp, imap, false));
    for format in ["--json", "--jsonl", "--plain"] {
        let full = std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/full")
            .unwrap();
        let args = [
            "send",
            "--to",
            "ada@example.com",
            "--subject",
            "Hi",
            "--body",
            "-",
            "--yes",
            format,
        ];
        let o = neomail_to(dir.path(), &args, BODY, full.into());
        assert_eq!(o.status.code(), Some(0), "{format}: {}", stderr(&o));
        let seen = smtp.seen();
        let message = seen.messages.last().unwrap();
        let id = header(message, "Message-ID").unwrap();
        let err = stderr(&o);
        assert!(
            err.contains(&format!("the message {id} was sent")),
            "{format}: {id}: {err}"
        );
        assert!(err.contains("No space left on device"), "{format}: {err}");
        assert!(
            !err.contains("  "),
            "{format}: one line, no run of spaces: {err}"
        );
    }
    assert_eq!(smtp.seen().messages.len(), 3);
}

/// Caches `raw` in `account`'s inbox and returns its id, as `neomail list` prints it.
fn cache(dir: &Path, account: &str, raw: &str) -> String {
    let mut cache = Cache::open(&dir.join("cache.sqlite3")).unwrap();
    let inbox = cache
        .reset_mailbox(account, "INBOX", UidValidity::new(1).unwrap())
        .unwrap();
    let message = FetchedMessage {
        uid: Uid::new(7).unwrap(),
        flags: Flags::new(Vec::<String>::new()),
        received: chrono::DateTime::from_timestamp(1_791_000_000, 0),
        raw: raw.as_bytes().to_vec(),
    };
    cache.insert_messages(inbox.id, &[message]).unwrap();
    drop(cache);
    let o = neomail(dir, &["list", "--plain"], "");
    stdout(&o).split('\t').next().unwrap().to_owned()
}

const ORIGINAL: &str = "From: Ada <ada@example.com>\r\nTo: Me <ME@example.com>, bob@example.com\r\n\
    Cc: cy@example.com, ada@example.com\r\nReply-To: Ada Lists <ada-list@example.com>\r\n\
    Subject: RE: Plans\r\nDate: Sat, 10 Oct 2026 09:30:00 +0000\r\n\
    Message-ID: <three@example.com>\r\nIn-Reply-To: <two@example.com>\r\n\
    References: <one@example.com> <two@example.com>\r\n\r\nShall we?\r\n";

#[test]
fn a_reply_all_threads_on_the_original_and_never_lists_me() {
    let smtp = FakeSmtp::start(&[]);
    let (imap, _) = closed_imap();
    let dir = setup(&account("work", "me@example.com", &smtp, imap, false));
    let id = cache(dir.path(), "work", ORIGINAL);
    let o = neomail(
        dir.path(),
        &["reply", &id, "--all", "--body", "-", "--yes", "--json"],
        "Yes.\n",
    );
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let json: Value = serde_json::from_str(&stdout(&o)).unwrap();
    let addresses = |field: &str| -> Vec<String> {
        json[field]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["address"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(addresses("to"), ["ada-list@example.com"]);
    assert_eq!(
        addresses("cc"),
        ["bob@example.com", "cy@example.com", "ada@example.com"]
    );
    assert_eq!(json["subject"], "RE: Plans");
    assert_eq!(json["in_reply_to"], "three@example.com");
    let seen = smtp.seen();
    let message = &seen.messages[0];
    assert_eq!(
        header(message, "In-Reply-To").as_deref(),
        Some("<three@example.com>")
    );
    assert_eq!(
        header(message, "References").as_deref(),
        Some("<one@example.com> <two@example.com> <three@example.com>")
    );
    assert!(
        !seen
            .commands
            .iter()
            .any(|c| c.eq_ignore_ascii_case("rcpt to:<me@example.com>"))
    );
    assert!(message.contains("> Shall we?"), "{message}");
}

/// With an alias in `address`, the login (in the original's To) gets no copy of a reply to all.
#[test]
fn a_reply_all_from_an_alias_never_lists_the_login() {
    let smtp = FakeSmtp::start(&[]);
    let (imap, _) = closed_imap();
    let config = account("work", "me@example.com", &smtp, imap, false).replacen(
        "display_name",
        "address = \"alias@example.com\"\ndisplay_name",
        1,
    );
    let dir = setup(&config);
    let id = cache(dir.path(), "work", ORIGINAL);
    let o = neomail(
        dir.path(),
        &["reply", &id, "--all", "--body", "-", "--yes", "--json"],
        "Yes.\n",
    );
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let json: Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(json["from"]["address"], "alias@example.com");
    let cc: Vec<&str> = json["cc"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["address"].as_str().unwrap())
        .collect();
    assert_eq!(cc, ["bob@example.com", "cy@example.com", "ada@example.com"]);
    let seen = smtp.seen();
    assert!(
        !seen
            .commands
            .iter()
            .any(|c| c.eq_ignore_ascii_case("rcpt to:<me@example.com>")),
        "{:?}",
        seen.commands
    );
}

#[test]
fn a_forward_carries_the_original_and_warns_that_attachments_stay_behind() {
    let smtp = FakeSmtp::start(&[]);
    let (imap, _) = closed_imap();
    let dir = setup(&account("work", "me@example.com", &smtp, imap, false));
    let with_file = "From: ada@example.com\r\nTo: me@example.com\r\nSubject: Report\r\n\
        Message-ID: <r@example.com>\r\nMIME-Version: 1.0\r\n\
        Content-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\n\
        Content-Type: text/plain\r\n\r\nSee attached.\r\n--b\r\n\
        Content-Type: application/pdf\r\nContent-Disposition: attachment; filename=r.pdf\r\n\
        Content-Transfer-Encoding: base64\r\n\r\nJVBERi0=\r\n--b--\r\n";
    let id = cache(dir.path(), "work", with_file);
    let o = neomail(
        dir.path(),
        &["forward", &id, "--to", "cy@example.com", "--dry-run"],
        "",
    );
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert!(stderr(&o).contains("1 attachment"), "{}", stderr(&o));
    let message = stdout(&o);
    assert_eq!(header(&message, "Subject").as_deref(), Some("Fwd: Report"));
    assert!(message.contains("> See attached."), "{message}");
    assert!(header(&message, "In-Reply-To").is_none());
    assert_eq!(smtp.seen().connections, 0);
}

#[test]
fn from_picks_the_account_by_name_or_address_and_is_needed_with_several() {
    let smtp = FakeSmtp::start(&[]);
    let (imap, _) = closed_imap();
    let config = [
        account("work", "me@example.com", &smtp, imap, false),
        account("home", "me@home.example", &smtp, imap, false),
    ]
    .concat();
    let dir = setup(&config);
    let args = [
        "send",
        "--to",
        "a@example.com",
        "--subject",
        "Hi",
        "--body",
        "-",
        "--dry-run",
    ];
    let o = neomail(dir.path(), &args, BODY);
    assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
    assert!(stderr(&o).contains("work, home"), "{}", stderr(&o));
    for from in ["home", "ME@home.example"] {
        let o = neomail(dir.path(), &[&args[..], &["--from", from]].concat(), BODY);
        assert_eq!(o.status.code(), Some(0), "{from}: {}", stderr(&o));
        let sender = header(&stdout(&o), "From").unwrap();
        assert_eq!(sender, "\"Me\" <me@home.example>", "{from}");
    }
    let o = neomail(
        dir.path(),
        &[&args[..], &["--from", "nobody"]].concat(),
        BODY,
    );
    assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
}

#[test]
fn an_account_that_cannot_send_says_why_before_anything_is_sent() {
    let outlook = "[[accounts]]\nname = \"outlook\"\nusername = \"you@outlook.example\"\n\
        oauth = { provider = \"microsoft\", client_id = \"00000000-0000-0000-0000-000000000000\" }\n";
    let no_smtp = "[[accounts]]\nname = \"other\"\nusername = \"me@example.com\"\n\
        password_command = \"echo test\"\n[accounts.imap]\nhost = \"127.0.0.1\"\nport = 9\n";
    for (config, from, extra, says) in [
        (outlook, "outlook", &["--bcc", "eve@example.com"][..], "Bcc"),
        (no_smtp, "other", &[][..], "[accounts.smtp]"),
    ] {
        let dir = setup(config);
        let args = [
            "send",
            "--from",
            from,
            "--to",
            "a@example.com",
            "--subject",
            "Hi",
            "--body",
            "-",
        ];
        // A dry run refuses it too: it must not pass a message the real send refuses.
        for last in ["--yes", "--dry-run"] {
            let o = neomail(dir.path(), &[&args[..], extra, &[last]].concat(), BODY);
            assert_eq!(o.status.code(), Some(1), "{from} {last}: {}", stderr(&o));
            assert!(stderr(&o).contains(says), "{from}: {}", stderr(&o));
            assert!(o.stdout.is_empty(), "{from} {last}");
            assert!(
                stderr(&o).contains("nothing was sent") || from == "other",
                "{}",
                stderr(&o)
            );
        }
    }
}

/// With several accounts a reply or forward is sent from the account the original is in, unless
/// --from says otherwise; reply-all then leaves out that account's own address.
#[test]
fn a_reply_or_forward_is_sent_from_the_originals_account() {
    let smtp = FakeSmtp::start(&[]);
    let (imap, _) = closed_imap();
    let config = [
        account("work", "me@example.com", &smtp, imap, false),
        account("home", "me@home.example", &smtp, imap, false),
    ]
    .concat();
    let dir = setup(&config);
    let original = "From: Ada <ada@example.com>\r\nTo: me@home.example, me@example.com\r\n\
        Subject: Plans\r\nMessage-ID: <h@example.com>\r\n\r\nShall we?\r\n";
    let id = cache(dir.path(), "home", original);
    let reply = ["reply", &id, "--all", "--body", "-", "--dry-run"];
    let forward = ["forward", &id, "--to", "cy@example.com", "--dry-run"];
    for (args, from, to, cc) in [
        (
            &reply[..],
            "\"Me\" <me@home.example>",
            "\"Ada\" <ada@example.com>",
            Some("<me@example.com>"),
        ),
        (
            &forward[..],
            "\"Me\" <me@home.example>",
            "<cy@example.com>",
            None,
        ),
    ] {
        let o = neomail(dir.path(), args, BODY);
        assert_eq!(o.status.code(), Some(0), "{args:?}: {}", stderr(&o));
        let message = stdout(&o);
        assert_eq!(header(&message, "From").as_deref(), Some(from), "{args:?}");
        assert_eq!(header(&message, "To").as_deref(), Some(to), "{args:?}");
        assert_eq!(header(&message, "Cc").as_deref(), cc, "{args:?}");
    }
    let o = neomail(
        dir.path(),
        &[&reply[..], &["--from", "work"]].concat(),
        BODY,
    );
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let message = stdout(&o);
    assert_eq!(
        header(&message, "From").as_deref(),
        Some("\"Me\" <me@example.com>")
    );
    assert_eq!(header(&message, "Cc").as_deref(), Some("<me@home.example>"));
}

/// An Outlook sign-in from before neomail asked for Mail.Send is told to sign in again before
/// anything is asked. Without --yes and off a terminal, the question would be a usage error, so
/// exit 1 shows the grant was checked first; a sign-in that may send reaches the question.
#[test]
fn an_outlook_sign_in_that_cannot_send_is_told_before_the_question() {
    let outlook = "[[accounts]]\nname = \"outlook\"\nusername = \"you@outlook.example\"\n\
        oauth = { provider = \"microsoft\", client_id = \"00000000-0000-0000-0000-000000000000\" }\n";
    let args = [
        "send",
        "--to",
        "a@example.com",
        "--subject",
        "Hi",
        "--body",
        "-",
    ];
    for (grant, code) in [
        (None, 1),
        (Some("read"), 1),
        (Some("read-write"), 1),
        (Some("read-write-send"), 2),
    ] {
        let dir = setup(outlook);
        let mut entry = serde_json::json!({ "refresh_token": "not-a-real-token" });
        if let Some(grant) = grant {
            entry["mail_grant"] = grant.into();
        }
        let keyring = serde_json::json!({ "neomail-oauth\noutlook": entry.to_string() });
        std::fs::write(dir.path().join("keyring.json"), keyring.to_string()).unwrap();
        let o = neomail(dir.path(), &args, BODY);
        assert_eq!(o.status.code(), Some(code), "{grant:?}: {}", stderr(&o));
        assert!(stderr(&o).contains("nothing was sent"), "{}", stderr(&o));
        let login = stderr(&o).contains("neomail account login outlook");
        assert_eq!(login, code == 1, "{grant:?}: {}", stderr(&o));
    }
}

/// A comma separates addresses, but not one inside a quoted display name, on each address flag.
#[test]
fn a_comma_in_a_quoted_name_does_not_split_the_address() {
    let smtp = FakeSmtp::start(&[]);
    let (imap, _) = closed_imap();
    let dir = setup(&account("work", "me@example.com", &smtp, imap, false));
    let list = "\"Doe, Jane\" <jane@example.com>, bob@example.com";
    let both = "\"Doe, Jane\" <jane@example.com>, <bob@example.com>";
    let base = ["send", "--subject", "Hi", "--body", "-", "--dry-run"];
    let o = neomail(dir.path(), &[&base[..], &["--to", list]].concat(), BODY);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(header(&stdout(&o), "To").as_deref(), Some(both));
    let o = neomail(
        dir.path(),
        &[
            &base[..],
            &[
                "--to",
                "a@example.com",
                "--cc",
                list,
                "--bcc",
                "\"Roe, Ann\" <ann@example.com>, cy@example.com",
            ],
        ]
        .concat(),
        BODY,
    );
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(header(&stdout(&o), "Cc").as_deref(), Some(both));
    let summary = stderr(&o);
    let bcc = summary
        .lines()
        .find(|l| l.starts_with("Bcc:"))
        .unwrap_or_default();
    assert!(bcc.contains("Roe, Ann <ann@example.com>"), "{summary}");
    assert!(bcc.contains("cy@example.com"), "{summary}");
}

/// `send` has no original, so its --from default is the only account; reply's is the original's.
#[test]
fn from_help_fits_each_command() {
    let dir = setup("");
    let help = |command: &str| stdout(&neomail(dir.path(), &[command, "--help"], ""));
    let send = help("send");
    assert!(send.contains("[default: the only one]"), "{send}");
    assert!(!send.contains("original"), "{send}");
    assert!(help("reply").contains("[default: the original's, else the only one]"));
    assert!(help("forward").contains("[default: the original's, else the only one]"));
}
