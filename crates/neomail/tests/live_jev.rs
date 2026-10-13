//! The one live check of classification (DEC-2026-10-13-classify-send): made-up mail, the real
//! jev, the owner's key. Off by default: run it with
//!
//! ```sh
//! NEOMAIL_LIVE_JEV=1 NEOMAIL_JEV_LEDGER=PATH JEV_API_KEY=... \
//!     cargo test -p neomail --test live_jev -- --ignored --nocapture
//! ```
//!
//! The binary talks to a counting forwarder on 127.0.0.1, which passes requests on to
//! api.typesafe.ai over TLS, with the binary's own method, path and Content-Type, and refuses
//! any past the budget, so no run, retries included, takes the ledger past 20 requests in all.
//! The forwarder appends `{"date", "requests": 1}` to the ledger before it forwards each
//! request, so a run that is killed still counts what it sent; each finished run then appends
//! one summary line (date, model, `requests: 0`, `forwarded`, input_tokens, estimated cost).
//! A ledger line that does not parse counts as the whole budget.
#![allow(clippy::unwrap_used)] // Test helpers outside #[test] functions.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{Duration, Utc};
use nm_core::{Cache, FetchedMessage, Flags, Uid, UidValidity};
use rustls_platform_verifier::BuilderVerifierExt;
use serde_json::{Value, json};

/// Requests allowed across every run in the ledger.
const BUDGET: u64 = 20;
const HOST: &str = "api.typesafe.ai";
const USD_PER_MILLION_INPUT: f64 = 0.042;

/// (expected group, days old, From, extra headers, subject, body). Every address is made up.
const MAIL: [(&str, i64, &str, &str, &str, &str); 12] = [
    (
        "receipts-invoices",
        2,
        "receipts@coffee.example.com",
        "",
        "Your receipt from Example Coffee",
        "Thanks for your order. Total paid: EUR 4.50 by card ending 1234. Receipt no. 88812.",
    ),
    (
        "orders-shipping",
        3,
        "shipping@shop.example.com",
        "",
        "Your order has shipped",
        "Good news: your order of 2 items is on its way. Track your parcel with the link below. Expected delivery: Thursday.",
    ),
    (
        "newsletters",
        4,
        "newsletter@bytes.example.com",
        "List-Unsubscribe: <https://bytes.example.com/u>\r\nList-Id: <weekly.bytes.example.com>\r\n",
        "The Weekly Byte, issue 112",
        "This week: three essays on databases, a reader's letter, and links we enjoyed. You subscribed to this newsletter.",
    ),
    (
        "promotions",
        5,
        "marketing@store.example.com",
        "List-Unsubscribe: <https://store.example.com/u>\r\nPrecedence: bulk\r\n",
        "48 hours only: 30% off everything",
        "Our biggest sale of the season. Use code SAVE30 at checkout. Shop now before it ends.",
    ),
    (
        "notifications",
        6,
        "security@cloud.example.com",
        "Auto-Submitted: auto-generated\r\n",
        "New sign-in to your account",
        "We noticed a new sign-in to your account from a new device. If this was you, no action is needed.",
    ),
    (
        "recruitment",
        7,
        "jordan@talent.example.com",
        "",
        "Senior engineer role at Northwind",
        "Hi, I'm a recruiter at Talent Partners. Your profile looks like a great fit for a senior backend role at Northwind. Open to a quick call?",
    ),
    (
        "follow-up",
        1,
        "sam@example.com",
        "",
        "Can you review the budget by Friday?",
        "Hi, could you look over the attached budget and send me your comments by Friday? I need them before the board meeting.",
    ),
    (
        "follow-up-old",
        120,
        "lee@example.com",
        "",
        "Could you send me the signed form?",
        "Hi, just checking: could you sign the rental form and send it back to me? I still need it to finish the paperwork.",
    ),
    (
        "spam",
        8,
        "support@paypa1-secure.example.com",
        "",
        "Your account is suspended: verify now",
        "Dear customer, your account has been suspended. Click here within 24 hours and enter your password and card number to restore access.",
    ),
    (
        "main",
        9,
        "alex@example.com",
        "",
        "Photos from the weekend",
        "It was so good to see everyone on Saturday. Here are a few photos from the hike. No need to reply, just wanted to share.",
    ),
    (
        "notifications",
        10,
        "no-reply@calendar.example.com",
        "Auto-Submitted: auto-generated\r\n",
        "Reminder: dentist appointment tomorrow",
        "This is a reminder of your appointment tomorrow at 9:00.",
    ),
    (
        "main",
        11,
        "kim@example.com",
        "In-Reply-To: <a1@example.com>\r\n",
        "Re: book club",
        "Thanks, I loved the book too. See you at the next meeting.",
    ),
];

#[test]
#[ignore = "calls the real jev: NEOMAIL_LIVE_JEV=1, JEV_API_KEY and NEOMAIL_JEV_LEDGER"]
fn live_jev_sorts_made_up_mail() {
    if std::env::var("NEOMAIL_LIVE_JEV").as_deref() != Ok("1") {
        eprintln!("skipped: set NEOMAIL_LIVE_JEV=1 to call jev");
        return;
    }
    let key = std::env::var("JEV_API_KEY").expect("JEV_API_KEY");
    assert!(!key.trim().is_empty(), "JEV_API_KEY is empty");
    let ledger = std::env::var("NEOMAIL_JEV_LEDGER").expect("NEOMAIL_JEV_LEDGER names the ledger");
    // Only a ledger that does not exist yet is empty: one that cannot be read fails closed.
    let text = match std::fs::read_to_string(&ledger) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => panic!("cannot read the ledger {ledger}: {e}"),
    };
    let spent = spent(&text);
    let left = BUDGET.saturating_sub(spent);
    assert!(
        left >= MAIL.len() as u64,
        "the ledger has {spent} of {BUDGET} requests spent (a line that does not parse counts \
         as all of them); a run needs {}",
        MAIL.len()
    );

    let dir = tempfile::tempdir().unwrap();
    let proxy = Forwarder::start(left, ledger.clone());
    std::fs::write(
        dir.path().join("config.toml"),
        format!(
            "[[accounts]]\nname = \"live\"\nusername = \"you@example.com\"\n\
             imap = {{ host = \"imap.example.com\" }}\n\n[classify]\nurl = \"http://127.0.0.1:{}\"\n",
            proxy.port
        ),
    )
    .unwrap();
    seed(dir.path());
    let output = Command::new(env!("CARGO_BIN_EXE_neomail"))
        .args(["classify", "--yes", "--all", "--json"])
        .env("NEOMAIL_DATA_DIR", dir.path())
        .env("NEOMAIL_CONFIG", dir.path().join("config.toml"))
        .env("HOME", dir.path())
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent")
        .env("JEV_API_KEY", key.trim())
        .output()
        .unwrap();
    let requests = proxy.requests.load(Ordering::SeqCst);
    let tokens = proxy.input_tokens.load(Ordering::SeqCst);
    // Each forwarded request is already in the ledger; this line adds none.
    let line = json!({
        "date": Utc::now().to_rfc3339(),
        "model": proxy.model.lock().unwrap().clone(),
        "requests": 0,
        "forwarded": requests,
        "input_tokens": tokens,
        "estimated_cost_usd": tokens as f64 / 1_000_000.0 * USD_PER_MILLION_INPUT,
    });
    append(&ledger, &line);
    eprintln!("ledger: {line}");
    eprintln!("{}", String::from_utf8_lossy(&output.stderr));
    assert!(requests <= left, "the forwarder let {requests} through");

    let results: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap_or_default();
    let mut right = 0;
    for r in &results {
        let subject = subject_of(dir.path(), &r["id"]);
        let expected = MAIL.iter().find(|m| m.4 == subject).map_or("?", |m| m.0);
        right += usize::from(r["group"] == expected);
        eprintln!(
            "{expected:<18} {:<18} {:<18} {:.2}  {subject}",
            r["group"].as_str().unwrap_or("-"),
            r["choice"]
                .as_str()
                .unwrap_or(r["error"].as_str().unwrap_or("-")),
            r["confidence"].as_f64().unwrap_or(0.0),
        );
    }
    eprintln!(
        "{right} of {} as expected (expected, group, choice, confidence)",
        MAIL.len()
    );
    assert!(output.status.success(), "neomail classify failed");
    assert_eq!(results.len(), MAIL.len());
}

/// Requests the ledger records: the sum of its `requests` fields, with a line that does not
/// parse (one half-written by a kill, say) counting as the whole budget.
fn spent(ledger: &str) -> u64 {
    ledger
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            serde_json::from_str::<Value>(l)
                .ok()
                .and_then(|v| v["requests"].as_u64())
                .unwrap_or(BUDGET)
        })
        .fold(0, u64::saturating_add)
}

/// Appends one line to the ledger and syncs it, so it survives the process being killed.
fn append(ledger: &str, line: &Value) {
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(ledger)
        .unwrap();
    file.write_all(format!("{line}\n").as_bytes()).unwrap();
    file.sync_data().unwrap();
}

#[test]
fn the_ledger_counts_every_request_and_a_broken_line_as_the_whole_budget() {
    let per_request = r#"{"date":"2026-10-13T10:00:00Z","requests":1}"#;
    let summary = r#"{"date":"2026-10-13T10:01:00Z","requests":0,"forwarded":2}"#;
    assert_eq!(spent(""), 0);
    assert_eq!(
        spent(&format!("{per_request}\n{per_request}\n{summary}\n")),
        2
    );
    // A run killed while writing leaves half a line.
    assert_eq!(
        spent(&format!("{per_request}\n{{\"date\":\"2026-1")),
        BUDGET + 1
    );
    assert_eq!(spent(r#"{"date":"2026-10-13T10:00:00Z"}"#), BUDGET);
}

fn seed(dir: &std::path::Path) {
    let mut cache = Cache::open(&dir.join("cache.sqlite3")).unwrap();
    let inbox = cache
        .reset_mailbox("live", "INBOX", UidValidity::new(1).unwrap())
        .unwrap();
    let messages: Vec<FetchedMessage> = MAIL
        .iter()
        .zip(1u32..)
        .map(
            |((_, days, from, extra, subject, body), uid)| FetchedMessage {
                uid: Uid::new(uid).unwrap(),
                flags: Flags::new(Vec::<String>::new()),
                received: Some(Utc::now() - Duration::days(*days)),
                raw: format!(
                    "Message-ID: <live{uid}@example.com>\r\nFrom: {from}\r\nTo: you@example.com\r\n\
                 {extra}Subject: {subject}\r\n\r\n{body}\r\n"
                )
                .into_bytes(),
            },
        )
        .collect();
    cache.insert_messages(inbox.id, &messages).unwrap();
}

fn subject_of(dir: &std::path::Path, id: &Value) -> String {
    let cache = Cache::open(&dir.join("cache.sqlite3")).unwrap();
    let stored = cache
        .get(nm_core::MessageId::new(id.as_i64().unwrap()))
        .unwrap()
        .unwrap();
    nm_core::parse::parse(&stored.raw)
        .unwrap()
        .subject
        .unwrap_or_default()
}

/// Plain HTTP on 127.0.0.1 in, TLS to api.typesafe.ai out, one request per connection. It
/// records each request in the ledger before forwarding it, and answers 400 itself once
/// `budget` is spent, which stops the run.
struct Forwarder {
    port: u16,
    requests: Arc<AtomicU64>,
    input_tokens: Arc<AtomicU64>,
    model: Arc<Mutex<Option<String>>>,
}

impl Forwarder {
    fn start(budget: u64, ledger: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(AtomicU64::new(0));
        let input_tokens = Arc::new(AtomicU64::new(0));
        let model = Arc::new(Mutex::new(None));
        let (r, t, m) = (requests.clone(), input_tokens.clone(), model.clone());
        let ledger = Arc::new(Mutex::new(ledger));
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (r, t, m, l) = (r.clone(), t.clone(), m.clone(), ledger.clone());
                std::thread::spawn(move || forward(stream, budget, &l, &r, &t, &m));
            }
        });
        Forwarder {
            port,
            requests,
            input_tokens,
            model,
        }
    }
}

fn forward(
    mut client: TcpStream,
    budget: u64,
    ledger: &Mutex<String>,
    requests: &AtomicU64,
    input_tokens: &AtomicU64,
    model: &Mutex<Option<String>>,
) {
    let mut reader = BufReader::new(client.try_clone().unwrap());
    // The binary's own request line and Content-Type go upstream as they are, so a wrong
    // method, path or type fails here as it would for a user.
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
        return;
    }
    let request_line = request_line.trim_end().to_owned();
    let (mut length, mut authorization) = (0usize, String::new());
    let (mut content_type, mut accept) = (String::new(), String::new());
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            match name.to_ascii_lowercase().as_str() {
                "content-length" => length = value.trim().parse().unwrap(),
                "authorization" => authorization = value.trim().to_owned(),
                "content-type" => content_type = format!("Content-Type: {}\r\n", value.trim()),
                "accept" => accept = format!("Accept: {}\r\n", value.trim()),
                _ => {}
            }
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    if requests.fetch_add(1, Ordering::SeqCst) >= budget {
        requests.fetch_sub(1, Ordering::SeqCst);
        let refusal = "the live check's request budget is spent";
        let _ = write!(
            client,
            "HTTP/1.1 400 Budget\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{refusal}",
            refusal.len()
        );
        return;
    }
    {
        let path = ledger.lock().unwrap();
        append(
            &path,
            &json!({"date": Utc::now().to_rfc3339(), "requests": 1}),
        );
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_platform_verifier()
        .unwrap()
        .with_no_client_auth();
    let name = rustls::pki_types::ServerName::try_from(HOST).unwrap();
    let connection = rustls::ClientConnection::new(Arc::new(config), name).unwrap();
    let tcp = TcpStream::connect((HOST, 443)).unwrap();
    let mut tls = rustls::StreamOwned::new(connection, tcp);
    let head = format!(
        "{request_line}\r\nHost: {HOST}\r\nAuthorization: {authorization}\r\n\
         {content_type}{accept}Content-Length: {length}\r\nConnection: close\r\n\r\n"
    );
    tls.write_all(head.as_bytes()).unwrap();
    tls.write_all(&body).unwrap();
    let mut response = Vec::new();
    // A server that closes without TLS close_notify ends the read with an error; keep what came.
    let _ = tls.read_to_end(&mut response);
    if let Some(answer) = answer_of(&response) {
        input_tokens.fetch_add(
            answer["usage"]["input_tokens"].as_u64().unwrap_or(0),
            Ordering::SeqCst,
        );
        if let Some(m) = answer["model"].as_str() {
            *model.lock().unwrap() = Some(m.to_owned());
        }
    }
    let _ = client.write_all(&response);
}

/// The JSON body of a response, de-chunked when it is sent in chunks.
fn answer_of(response: &[u8]) -> Option<Value> {
    let split = response.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(&response[..split]).to_ascii_lowercase();
    let mut body = &response[split + 4..];
    if !head.contains("transfer-encoding: chunked") {
        return serde_json::from_slice(body).ok();
    }
    let mut out = Vec::new();
    loop {
        let line_end = body.windows(2).position(|w| w == b"\r\n")?;
        let size_text = String::from_utf8_lossy(&body[..line_end]);
        let size = usize::from_str_radix(size_text.split(';').next()?.trim(), 16).ok()?;
        if size == 0 {
            return serde_json::from_slice(&out).ok();
        }
        let start = line_end + 2;
        out.extend_from_slice(body.get(start..start + size)?);
        body = body.get(start + size + 2..)?;
    }
}
