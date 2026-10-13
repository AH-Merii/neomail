//! jev on 127.0.0.1, for the tests of `neomail classify` (tests/classify.rs) and of the
//! daemon's classifying (src/classify/daemon.rs, which includes this file by path, as the crate
//! has no library target). It uses only std and serde_json, and reaches nothing beyond this
//! machine.
#![allow(dead_code, reason = "each includer uses part of it")]
#![allow(
    clippy::unwrap_used,
    reason = "a test helper: a broken fake should panic"
)]

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

/// One scripted answer.
#[derive(Clone)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(&'static str, &'static str)>,
    pub body: String,
}

impl Reply {
    pub fn status(status: u16, body: &str) -> Self {
        Reply {
            status,
            headers: Vec::new(),
            body: body.to_owned(),
        }
    }
}

/// A request the fake received.
pub struct Captured {
    /// The request line, such as `POST /v1/systemone HTTP/1.1`.
    pub request_line: String,
    pub content_type: Option<String>,
    pub authorization: Option<String>,
    pub body: String,
}

impl Captured {
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap()
    }
}

/// jev on 127.0.0.1: it records every request and plays the scripted replies in order, then
/// answers each request by its subject (see [`default_answer`]). One connection at a time.
pub struct FakeJev {
    pub port: u16,
    pub captured: Arc<Mutex<Vec<Captured>>>,
    pub script: Arc<Mutex<VecDeque<Reply>>>,
}

impl FakeJev {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let captured = Arc::new(Mutex::new(Vec::new()));
        let script = Arc::new(Mutex::new(VecDeque::new()));
        let (c, s) = (captured.clone(), script.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                serve(stream, &c, &s);
            }
        });
        FakeJev {
            port,
            captured,
            script,
        }
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub fn play(&self, replies: impl IntoIterator<Item = Reply>) {
        self.script.lock().unwrap().extend(replies);
    }

    pub fn count(&self) -> usize {
        self.captured.lock().unwrap().len()
    }

    pub fn bodies(&self) -> Vec<Value> {
        self.captured
            .lock()
            .unwrap()
            .iter()
            .map(Captured::json)
            .collect()
    }
}

pub fn serve(stream: TcpStream, captured: &Mutex<Vec<Captured>>, script: &Mutex<VecDeque<Reply>>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
        return;
    }
    let request_line = request_line.trim_end().to_owned();
    let (mut length, mut authorization, mut content_type) = (0, None, None);
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
                "authorization" => authorization = Some(value.trim().to_owned()),
                "content-type" => content_type = Some(value.trim().to_owned()),
                _ => {}
            }
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    let body = String::from_utf8(body).unwrap();
    let reply = script
        .lock()
        .unwrap()
        .pop_front()
        .unwrap_or_else(|| default_answer(&body));
    captured.lock().unwrap().push(Captured {
        request_line,
        content_type,
        authorization,
        body,
    });
    let mut head = format!(
        "HTTP/1.1 {} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n",
        reply.status,
        reply.body.len()
    );
    for (name, value) in &reply.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    let mut stream = stream;
    let _ = stream.write_all(format!("{head}\r\n{}", reply.body).as_bytes());
}

/// jev's answer for a subject whose first word is the choice, with confidence 0.2 when the
/// second word is "unsure" and 0.9 otherwise: "spam unsure Win" answers spam with 0.2. A subject
/// that starts with no option's name answers main.
pub fn default_answer(request: &str) -> Reply {
    let request: Value = serde_json::from_str(request).unwrap();
    let subject = request["state"]["subject"].as_str().unwrap_or("");
    let mut words = subject.split_whitespace();
    let choice = words
        .next()
        .filter(|w| GROUPS.contains(w))
        .unwrap_or("main");
    let confidence = if words.next() == Some("unsure") {
        0.2
    } else {
        0.9
    };
    Reply::status(200, &answer(choice, confidence))
}

/// The ten options, in the order the request lists them.
pub const GROUPS: [&str; 10] = [
    "main",
    "follow-up",
    "recruitment",
    "receipts-invoices",
    "orders-shipping",
    "newsletters",
    "promotions",
    "notifications",
    "spam",
    "other",
];

/// A choice answer as https://docs.typesafe.ai/api.md gives one.
pub fn answer(choice: &str, confidence: f64) -> String {
    let probabilities: serde_json::Map<String, Value> = GROUPS
        .iter()
        .map(|g| (g.to_string(), json!(if *g == choice { 0.9 } else { 0.01 })))
        .collect();
    json!({
        "model": "jev-1.13.0",
        "answers": {"group": {
            "type": "choice", "choice": choice,
            "probabilities": probabilities, "confidence": confidence
        }},
        "usage": {"input_tokens": 400, "output_tokens": 20}
    })
    .to_string()
}
