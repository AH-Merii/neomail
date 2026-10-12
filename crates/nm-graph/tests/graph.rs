//! `GraphSource` against a fake Graph: a mailbox model that serves pages and next page links,
//! plus scripted replies that are served first. No network.
#![allow(clippy::unwrap_used)] // Test helpers outside #[test] functions.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::num::NonZeroU32;
use std::time::Duration;

use chrono::{DateTime, Utc};
use nm_core::{Flags, IdSource, RemoteId};
use nm_graph::{GraphError, GraphSource, MAX_JSON, MAX_MIME, Reply, Transport, TransportError};
use serde_json::{Value, json};

const TOKEN: &str = "eyJ0eXAi.not-a-real-token";
const API: &str = "https://graph.microsoft.com/v1.0";
const T0: i64 = 1_790_000_000;

fn at(n: u32) -> DateTime<Utc> {
    DateTime::from_timestamp(T0 + i64::from(n) * 60, 0).unwrap()
}

/// Graph-like ids: mixed case, with characters a path must encode.
fn id(n: u32) -> String {
    format!("AAMkAGI2/Tg+{n}=")
}

struct Message {
    id: String,
    read: bool,
    received: Option<DateTime<Utc>>,
}

/// One request as the fake saw it.
#[derive(Debug)]
struct Seen {
    url: String,
    headers: Vec<(String, String)>,
    max_body: usize,
}

struct FakeGraph {
    messages: Vec<Message>,
    /// The most entries a page holds, whatever `$top` says.
    page_size: usize,
    script: RefCell<VecDeque<Reply>>,
    seen: RefCell<Vec<Seen>>,
    in_flight: Cell<usize>,
    max_in_flight: Cell<usize>,
}

impl FakeGraph {
    fn new(messages: impl IntoIterator<Item = u32>) -> Self {
        Self {
            messages: messages
                .into_iter()
                .map(|n| Message {
                    id: id(n),
                    read: n % 2 == 0,
                    received: Some(at(n)),
                })
                .collect(),
            page_size: 1000,
            script: RefCell::default(),
            seen: RefCell::default(),
            in_flight: Cell::new(0),
            max_in_flight: Cell::new(0),
        }
    }

    fn script(self, replies: impl IntoIterator<Item = Reply>) -> Self {
        self.script.borrow_mut().extend(replies);
        self
    }

    fn urls(&self) -> Vec<String> {
        self.seen.borrow().iter().map(|s| s.url.clone()).collect()
    }

    fn serve(&self, url: &str) -> Reply {
        let rest = url.strip_prefix(API).unwrap();
        let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
        if let Some(segment) = path
            .strip_prefix("/me/messages/")
            .and_then(|p| p.strip_suffix("/$value"))
        {
            let wanted = decode(segment);
            return match self.messages.iter().find(|m| m.id == wanted) {
                Some(m) => reply(200, raw(&m.id).into_bytes()),
                None => json_reply(404, &json!({"error": {"code": "ErrorItemNotFound"}})),
            };
        }
        if path == "/me/mailFolders/inbox" {
            return json_reply(
                200,
                &json!({"id": "inbox", "totalItemCount": self.messages.len()}),
            );
        }
        assert_eq!(path, "/me/mailFolders/inbox/messages", "{url}");
        let param = |name: &str| {
            query
                .split('&')
                .find_map(|p| p.strip_prefix(&format!("{name}=")))
                .map(decode)
        };
        let since = param("$filter").map(|f| {
            let t = f.strip_prefix("receivedDateTime ge ").unwrap();
            DateTime::parse_from_rfc3339(t).unwrap().with_timezone(&Utc)
        });
        assert_eq!(param("$orderby").as_deref(), Some("receivedDateTime desc"));
        let top: usize = param("$top").unwrap().parse().unwrap();
        assert!((1..=1000).contains(&top), "$top {top}");
        let skip: usize = param("$skip").map_or(0, |s| s.parse().unwrap());
        let mut matching: Vec<&Message> = self
            .messages
            .iter()
            .filter(|m| since.is_none_or(|s| m.received.is_some_and(|r| r >= s)))
            .collect();
        matching.sort_by_key(|m| std::cmp::Reverse(m.received));
        let end = matching.len().min(skip + top.min(self.page_size));
        let value: Vec<Value> = matching[skip.min(end)..end]
            .iter()
            .map(|m| json!({"id": m.id, "isRead": m.read, "receivedDateTime": m.received}))
            .collect();
        let mut page = json!({ "value": value });
        if end < matching.len() {
            let base: Vec<&str> = query
                .split('&')
                .filter(|p| !p.starts_with("$skip="))
                .collect();
            page["@odata.nextLink"] = json!(format!("{API}{path}?{}&$skip={end}", base.join("&")));
        }
        json_reply(200, &page)
    }
}

impl Transport for FakeGraph {
    async fn get(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        max_body: usize,
    ) -> Result<Reply, TransportError> {
        self.seen.borrow_mut().push(Seen {
            url: url.to_owned(),
            headers: headers
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            max_body,
        });
        self.in_flight.set(self.in_flight.get() + 1);
        self.max_in_flight
            .set(self.max_in_flight.get().max(self.in_flight.get()));
        tokio::task::yield_now().await;
        self.in_flight.set(self.in_flight.get() - 1);
        let scripted = self.script.borrow_mut().pop_front();
        Ok(scripted.unwrap_or_else(|| self.serve(url)))
    }
}

fn decode(s: &str) -> String {
    let mut out = Vec::new();
    let mut bytes = s.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let hex = [bytes.next().unwrap(), bytes.next().unwrap()];
            out.push(u8::from_str_radix(std::str::from_utf8(&hex).unwrap(), 16).unwrap());
        } else {
            out.push(b);
        }
    }
    String::from_utf8(out).unwrap()
}

fn raw(id: &str) -> String {
    format!(
        "From: Ada <ada@mail.example>\r\nDate: Mon, 12 Oct 2026 08:00:00 +0000\r\n\
         Subject: Message {id}\r\n\r\nBody.\r\n"
    )
}

fn reply(status: u16, body: Vec<u8>) -> Reply {
    Reply {
        status,
        retry_after: None,
        body,
    }
}

fn json_reply(status: u16, body: &Value) -> Reply {
    reply(status, serde_json::to_vec(body).unwrap())
}

fn throttled(status: u16, retry_after: Option<u64>) -> Reply {
    Reply {
        status,
        retry_after: retry_after.map(Duration::from_secs),
        body: Vec::new(),
    }
}

fn source(fake: FakeGraph) -> GraphSource<FakeGraph> {
    GraphSource::new(fake, TOKEN)
}

fn transport(source: &GraphSource<FakeGraph>) -> &FakeGraph {
    // The source owns the fake; tests read it back through this.
    source.transport()
}

fn n(n: u32) -> NonZeroU32 {
    NonZeroU32::new(n).unwrap()
}

const SELECT: &str = "$select=id,isRead,receivedDateTime&$orderby=receivedDateTime%20desc";

#[tokio::test]
async fn newest_sends_the_exact_urls_and_headers_and_follows_next_links() {
    let mut fake = FakeGraph::new(1..=7);
    fake.page_size = 2;
    let mut s = source(fake);
    let listed = s.newest(n(5)).await.unwrap();
    let got: Vec<(&str, bool, DateTime<Utc>)> = listed
        .iter()
        .map(|m| (m.id.as_str(), m.flags.is_seen(), m.received))
        .collect();
    let want: Vec<(String, bool, DateTime<Utc>)> =
        (3..=7).rev().map(|k| (id(k), k % 2 == 0, at(k))).collect();
    assert_eq!(
        got,
        want.iter()
            .map(|(i, r, t)| (i.as_str(), *r, *t))
            .collect::<Vec<_>>()
    );
    let first = format!("{API}/me/mailFolders/inbox/messages?{SELECT}&$top=5");
    assert_eq!(
        transport(&s).urls(),
        [
            first.clone(),
            format!("{first}&$skip=2"),
            format!("{first}&$skip=4")
        ],
        "stops once it has 5"
    );
    for seen in transport(&s).seen.borrow().iter() {
        assert_eq!(
            seen.headers,
            [
                ("Authorization".to_owned(), format!("Bearer {TOKEN}")),
                ("Prefer".to_owned(), "IdType=\"ImmutableId\"".to_owned()),
                ("Accept".to_owned(), "application/json".to_owned()),
            ]
        );
        assert_eq!(seen.max_body, MAX_JSON);
    }
}

#[tokio::test]
async fn top_is_capped_at_1000_and_a_next_link_is_followed_verbatim() {
    let next =
        "https://graph.microsoft.com/v1.0/me/mailFolders/inbox/messages?$skiptoken=AbC%3D%3D&odd=x";
    let fake = FakeGraph::new([]).script([
        json_reply(
            200,
            &json!({"value": [{"id": "a", "isRead": true, "receivedDateTime": "2026-10-12T08:00:00Z"},
                              {"id": "no-arrival", "isRead": false}],
                    "@odata.nextLink": next}),
        ),
        json_reply(200, &json!({"value": []})),
    ]);
    let mut s = source(fake);
    let listed = s.newest(n(5000)).await.unwrap();
    assert_eq!(
        listed.len(),
        1,
        "an item without receivedDateTime is skipped"
    );
    assert_eq!(listed[0].flags, Flags::new([Flags::SEEN]));
    assert_eq!(
        transport(&s).urls(),
        [
            format!("{API}/me/mailFolders/inbox/messages?{SELECT}&$top=1000"),
            next.to_owned()
        ]
    );
}

#[tokio::test]
async fn arrived_since_filters_on_the_ordered_property() {
    let mut fake = FakeGraph::new(1..=5);
    fake.page_size = 2;
    let mut s = source(fake);
    let mut listed: Vec<String> = s
        .arrived_since(at(2))
        .await
        .unwrap()
        .into_iter()
        .map(|m| m.id.as_str().to_owned())
        .collect();
    listed.sort();
    let mut want: Vec<String> = (2..=5).map(id).collect();
    want.sort();
    assert_eq!(listed, want);
    let urls = transport(&s).urls();
    assert_eq!(
        urls[0],
        format!(
            "{API}/me/mailFolders/inbox/messages?$filter=receivedDateTime%20ge%20\
             2026-09-21T14:15:20Z&{SELECT}&$top=1000"
        )
    );
    assert_eq!(at(2).to_rfc3339(), "2026-09-21T14:15:20+00:00");
    assert_eq!(urls.len(), 2);
}

#[tokio::test]
async fn fetch_encodes_ids_leaves_out_gone_ones_and_runs_four_at_once() {
    let mut s = source(FakeGraph::new(1..=10));
    let mut ids: Vec<RemoteId> = (1..=10).map(|k| RemoteId::new(id(k)).unwrap()).collect();
    ids.push(RemoteId::new(id(1).to_lowercase()).unwrap());
    let fetched = s.fetch(&ids).await.unwrap();
    assert_eq!(fetched.len(), 10, "the unknown id is left out");
    for f in &fetched {
        assert_eq!(f.raw, raw(f.id.as_str()).into_bytes());
    }
    let fake = transport(&s);
    assert_eq!(fake.max_in_flight.get(), 4);
    let seen = fake.seen.borrow();
    assert!(
        seen.iter()
            .any(|r| r.url == format!("{API}/me/messages/AAMkAGI2%2FTg%2B1%3D/$value"))
    );
    for r in seen.iter() {
        assert_eq!(r.max_body, MAX_MIME);
        let names: Vec<&str> = r.headers.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(names, ["Authorization", "Prefer"]);
    }
}

#[tokio::test]
async fn a_next_link_off_graph_is_refused_without_a_request() {
    for link in [
        "https://graph.microsoft.com.evil.example/v1.0/x",
        "http://graph.microsoft.com/v1.0/x",
        "https://evil.example/https://graph.microsoft.com/",
    ] {
        let fake = FakeGraph::new([]).script([json_reply(
            200,
            &json!({"value": [], "@odata.nextLink": link}),
        )]);
        let mut s = source(fake);
        let e = s.newest(n(5)).await.unwrap_err();
        assert!(matches!(e, GraphError::ForeignLink), "{link}: {e}");
        assert_eq!(transport(&s).urls().len(), 1, "{link}");
    }
}

#[tokio::test(start_paused = true)]
async fn throttling_waits_retry_after_else_backs_off() {
    let fake = FakeGraph::new(1..=2).script([throttled(429, Some(7)), throttled(503, None)]);
    let mut s = source(fake);
    let start = tokio::time::Instant::now();
    assert_eq!(s.newest(n(2)).await.unwrap().len(), 2);
    assert_eq!(start.elapsed(), Duration::from_secs(7 + 2));
    assert_eq!(transport(&s).urls().len(), 3);

    let fake = FakeGraph::new(1..=2).script((0..5).map(|_| throttled(429, None)));
    let mut s = source(fake);
    let start = tokio::time::Instant::now();
    let e = s.newest(n(2)).await.unwrap_err();
    assert_eq!(start.elapsed(), Duration::from_secs(1 + 2 + 4 + 8));
    assert_eq!(transport(&s).urls().len(), 5);
    assert_eq!(
        e.to_string(),
        "Microsoft Graph is throttling requests (HTTP 429) after 5 tries; try again later"
    );
}

#[tokio::test]
async fn statuses_become_short_errors_that_never_hold_the_token() {
    let cases = [
        (reply(401, b"{}".to_vec()), "access token refused"),
        (reply(403, b"{}".to_vec()), "Mail.Read not granted"),
        (
            json_reply(
                400,
                &json!({"error": {"code": "InefficientFilter", "message": TOKEN}}),
            ),
            "Microsoft Graph answered HTTP 400 (InefficientFilter)",
        ),
        (
            reply(200, TOKEN.as_bytes().to_vec()),
            "Microsoft Graph's answer is not the expected JSON",
        ),
    ];
    for (answer, want) in cases {
        let mut s = source(FakeGraph::new([]).script([answer]));
        let e = s.newest(n(1)).await.unwrap_err();
        assert_eq!(e.to_string(), want);
        assert!(!format!("{e:?} {s:?}").contains("not-a-real-token"));
    }
    // A failed $value other than 404 is an error, not a missing message.
    let mut s = source(FakeGraph::new([1]).script([reply(500, Vec::new())]));
    let e = s.fetch(&[RemoteId::new(id(1)).unwrap()]).await.unwrap_err();
    assert_eq!(e.to_string(), "Microsoft Graph answered HTTP 500");
}

#[tokio::test]
async fn the_inbox_check_reads_the_folder() {
    let s = source(FakeGraph::new(1..=3));
    assert_eq!(s.inbox_count().await.unwrap(), 3);
    assert_eq!(
        transport(&s).urls(),
        [format!(
            "{API}/me/mailFolders/inbox?$select=id,totalItemCount"
        )]
    );
}

#[tokio::test]
async fn the_fake_graph_conforms() {
    let mut fake = FakeGraph::new(1..=12);
    fake.page_size = 4;
    let mut s = source(fake);
    nm_core::conformance::id_source(&mut s).await.unwrap();
}
