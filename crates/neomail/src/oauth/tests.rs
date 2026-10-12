#![allow(clippy::unwrap_used)] // Test helpers outside #[test] functions.

use std::cell::RefCell;
use std::collections::HashMap;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::loopback::{Listener, MAX_CONNECTIONS, Request, parse_request};
use super::*;
use crate::http::{self, Response};

const SECRET: &str = "GOCSPX-not-a-real-secret";
const REFRESH: &str = "1//refresh-not-real";
const ACCESS: &str = "ya29.access-not-real";
const CODE: &str = "4/0code-not-real";

/// A token endpoint that answers from a script and records each form it got, and when.
struct FakeHttp {
    answers: RefCell<Vec<(u16, String)>>,
    forms: RefCell<Vec<(String, HashMap<String, String>)>>,
    times: RefCell<Vec<tokio::time::Instant>>,
}

impl FakeHttp {
    fn new(answers: &[(u16, &str)]) -> Self {
        Self {
            answers: RefCell::new(answers.iter().map(|(s, b)| (*s, (*b).to_owned())).collect()),
            forms: RefCell::default(),
            times: RefCell::default(),
        }
    }

    /// Seconds from the first request to each later one.
    fn offsets(&self) -> Vec<u64> {
        let times = self.times.borrow();
        times
            .iter()
            .skip(1)
            .map(|t| (*t - times[0]).as_secs())
            .collect()
    }
}

impl http::Http for FakeHttp {
    async fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<Response> {
        let form = form
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        self.forms.borrow_mut().push((url.to_owned(), form));
        self.times.borrow_mut().push(tokio::time::Instant::now());
        let (status, body) = self.answers.borrow_mut().remove(0);
        Ok(Response {
            status,
            retry_after: None,
            body: body.into_bytes(),
        })
    }
}

fn client() -> Client {
    Client {
        id: "1234-abc.apps.googleusercontent.com".into(),
        secret: Some(Secret::new(SECRET.into())),
    }
}

/// A Microsoft public client: no secret.
fn public_client() -> Client {
    Client {
        id: "00000000-0000-0000-0000-000000000000".into(),
        secret: None,
    }
}

/// An unsigned ID token with these claims.
fn id_token(claims: &str) -> String {
    format!("eyJhbGciOiJub25lIn0.{}.sig", B64URL.encode(claims))
}

fn token_json(scope: &str, refresh: Option<&str>, email: &str) -> String {
    let refresh = refresh
        .map(|r| format!(r#""refresh_token":"{r}","#))
        .unwrap_or_default();
    let id = id_token(&format!(r#"{{"email":"{email}","email_verified":true}}"#));
    format!(r#"{{"access_token":"{ACCESS}",{refresh}"scope":"{scope}","id_token":"{id}"}}"#)
}

const FULL_SCOPE: &str =
    "openid https://mail.google.com/ https://www.googleapis.com/auth/userinfo.email";

#[test]
fn pkce_matches_rfc_7636_appendix_b() {
    let pkce = Pkce::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into());
    assert_eq!(
        pkce.challenge,
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
    let fresh = Pkce::new().unwrap();
    assert_eq!(fresh.verifier.expose().len(), 43);
    assert_ne!(fresh.verifier, Pkce::new().unwrap().verifier);
    assert!(!format!("{fresh:?}").contains(fresh.verifier.expose()));
}

#[test]
fn auth_url_asks_for_offline_mail_access_with_pkce() {
    let pkce = Pkce::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into());
    let url = auth_url(
        &GOOGLE,
        "1234-abc.apps.googleusercontent.com",
        "http://127.0.0.1:4242",
        &pkce,
        "st4te",
        Some("you+tag@example.com"),
    );
    assert_eq!(
        url,
        "https://accounts.google.com/o/oauth2/v2/auth?client_id=1234-abc.apps.googleusercontent.com\
         &redirect_uri=http%3A%2F%2F127.0.0.1%3A4242&response_type=code\
         &scope=https%3A%2F%2Fmail.google.com%2F%20openid%20email\
         &code_challenge=E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM&code_challenge_method=S256\
         &state=st4te&access_type=offline&prompt=consent&login_hint=you%2Btag%40example.com"
    );
    assert!(!url.contains("dBjftJeZ"), "the verifier stays local");
}

#[test]
fn redirect_parsing() {
    let get = |target: &str| format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
    let parse = |target: &str| parse_request(get(target).as_bytes(), "st4te");
    assert_eq!(
        parse("/?state=st4te&code=4%2F0code&scope=x"),
        Request::Code(Secret::new("4/0code".into()))
    );
    assert_eq!(parse("/?code=4%2F0code&state=other"), Request::WrongState);
    assert_eq!(parse("/?code=4%2F0code"), Request::WrongState);
    assert_eq!(
        parse("/?error=access_denied&state=other"),
        Request::WrongState
    );
    assert_eq!(parse("/?error=access_denied&state=st4te"), Request::Denied);
    assert_eq!(
        parse("/?error=server_error&state=st4te"),
        Request::Error("server_error".into())
    );
    assert_eq!(parse("/?state=st4te"), Request::NoCode);
    assert_eq!(parse("/?state=st4te&code="), Request::NoCode);
    assert_eq!(parse("/favicon.ico"), Request::Stray);
    assert_eq!(parse("/"), Request::Stray);
    assert_eq!(
        parse_request(b"POST /?state=st4te&code=c HTTP/1.1\r\n\r\n", "st4te"),
        Request::Stray
    );
    assert_eq!(parse_request(b"\xff\xfe", "st4te"), Request::Stray);
    assert!(!format!("{:?}", parse("/?state=st4te&code=4%2F0code")).contains("0code"));
}

/// Sends `request` to the listener and returns the response.
async fn send(port: u16, request: &[u8]) -> String {
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    s.write_all(request).await.unwrap();
    s.shutdown().await.unwrap();
    let mut response = String::new();
    s.read_to_string(&mut response).await.unwrap();
    response
}

#[tokio::test]
async fn listener_skips_stray_and_oversized_requests_and_returns_the_code() {
    let listener = Listener::bind(false).await.unwrap();
    let uri = listener.redirect_uri("127.0.0.1");
    let port: u16 = uri.rsplit(':').next().unwrap().parse().unwrap();
    let browser = tokio::spawn(async move {
        let favicon = send(port, b"GET /favicon.ico HTTP/1.1\r\n\r\n").await;
        let mut huge = b"GET /?".to_vec();
        huge.resize(64 * 1024, b'a');
        let oversized = send(port, &huge).await;
        let wrong = send(port, b"GET /?state=nope&code=x HTTP/1.1\r\n\r\n").await;
        let good = send(
            port,
            format!("GET /?state=st4te&code={CODE} HTTP/1.1\r\n\r\n").as_bytes(),
        )
        .await;
        (favicon, oversized, wrong, good)
    });
    let code = listener
        .wait("st4te", Duration::from_secs(30))
        .await
        .unwrap();
    assert_eq!(code.expose(), CODE);
    let (favicon, oversized, wrong, good) = browser.await.unwrap();
    assert!(favicon.starts_with("HTTP/1.1 404 "), "{favicon}");
    assert!(oversized.starts_with("HTTP/1.1 431 "), "{oversized}");
    assert!(wrong.starts_with("HTTP/1.1 400 "), "{wrong}");
    assert!(good.starts_with("HTTP/1.1 200 "), "{good}");
    assert!(!good.contains(CODE), "the page never echoes the code");
}

/// Sends `request` to the listener on [::1] and returns the response.
async fn send_v6(port: u16, request: &[u8]) -> String {
    let mut s = tokio::net::TcpStream::connect((std::net::Ipv6Addr::LOCALHOST, port))
        .await
        .unwrap();
    s.write_all(request).await.unwrap();
    s.shutdown().await.unwrap();
    let mut response = String::new();
    s.read_to_string(&mut response).await.unwrap();
    response
}

#[tokio::test]
async fn a_localhost_listener_takes_the_redirect_on_either_address() {
    // Without IPv6 on this machine the listener falls back to 127.0.0.1 alone; that is only
    // allowed when [::1] cannot be bound at all.
    let v6_works = tokio::net::TcpListener::bind((std::net::Ipv6Addr::LOCALHOST, 0))
        .await
        .is_ok();
    for code_over_v6 in [true, false] {
        let listener = Listener::bind(true).await.unwrap();
        assert_eq!(listener.has_v6(), v6_works);
        let uri = listener.redirect_uri("localhost");
        assert!(uri.starts_with("http://localhost:"), "{uri}");
        let port: u16 = uri.rsplit(':').next().unwrap().parse().unwrap();
        if !v6_works {
            let code = format!("GET /?state=st4te&code={CODE} HTTP/1.1\r\n\r\n");
            tokio::spawn(async move { send(port, code.as_bytes()).await });
            let got = listener.wait("st4te", Duration::from_secs(3)).await;
            assert_eq!(got.unwrap().expose(), CODE);
            return;
        }
        let browser = tokio::spawn(async move {
            let code = format!("GET /?state=st4te&code={CODE} HTTP/1.1\r\n\r\n");
            let stray = b"GET /favicon.ico HTTP/1.1\r\n\r\n";
            if code_over_v6 {
                (
                    send(port, stray).await,
                    send_v6(port, code.as_bytes()).await,
                )
            } else {
                (
                    send_v6(port, stray).await,
                    send(port, code.as_bytes()).await,
                )
            }
        });
        let got = listener.wait("st4te", Duration::from_secs(3)).await;
        assert_eq!(got.unwrap().expose(), CODE);
        let (stray, good) = browser.await.unwrap();
        assert!(stray.starts_with("HTTP/1.1 404 "), "{stray}");
        assert!(good.starts_with("HTTP/1.1 200 "), "{good}");
    }
    // A 127.0.0.1 redirect (Google) listens on IPv4 only.
    assert!(!Listener::bind(false).await.unwrap().has_v6());
}

#[tokio::test]
async fn an_idle_connection_does_not_hold_up_the_redirect() {
    let listener = Listener::bind(false).await.unwrap();
    let port: u16 = listener
        .redirect_uri("127.0.0.1")
        .rsplit(':')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    // A browser's speculative preconnect: opened first, kept open, never sends a byte.
    let idle = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let redirect = format!("GET /?state=st4te&code={CODE} HTTP/1.1\r\n\r\n");
    tokio::spawn(async move { send(port, redirect.as_bytes()).await });
    // Well under the 10 s a single request may take, so a serial listener would time out.
    let code = listener
        .wait("st4te", Duration::from_secs(3))
        .await
        .unwrap();
    assert_eq!(code.expose(), CODE);
    drop(idle);
}

#[tokio::test]
async fn the_listener_handles_at_most_max_connections_at_once() {
    let listener = Listener::bind(false).await.unwrap();
    let port: u16 = listener
        .redirect_uri("127.0.0.1")
        .rsplit(':')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    // A full set of idle connections, then the redirect. Connections are accepted in order.
    let mut idle = Vec::new();
    for _ in 0..MAX_CONNECTIONS {
        idle.push(
            tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .unwrap(),
        );
    }
    let redirect = format!("GET /?state=st4te&code={CODE} HTTP/1.1\r\n\r\n");
    let browser = tokio::spawn(async move { send(port, redirect.as_bytes()).await });
    // At the cap the redirect is not accepted, so this wait ends without a code. Without the
    // cap the redirect would be answered at once, as in the idle-connection test above.
    let e = listener
        .wait("st4te", Duration::from_millis(500))
        .await
        .unwrap_err();
    assert!(format!("{e:#}").contains("no sign-in came back"), "{e:#}");
    // The redirect waited in the queue, not lost: once the slots are free it is handled.
    let code = listener
        .wait("st4te", Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(code.expose(), CODE);
    assert!(browser.await.unwrap().starts_with("HTTP/1.1 200 "));
    drop(idle);
}

#[tokio::test]
async fn listener_reports_a_declined_sign_in_and_times_out() {
    let listener = Listener::bind(false).await.unwrap();
    let port: u16 = listener
        .redirect_uri("127.0.0.1")
        .rsplit(':')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    tokio::spawn(send(
        port,
        b"GET /?error=access_denied&state=s HTTP/1.1\r\n\r\n",
    ));
    let e = listener
        .wait("s", Duration::from_secs(30))
        .await
        .unwrap_err();
    assert!(format!("{e:#}").contains("declined"), "{e:#}");
    let e = listener
        .wait("s", Duration::from_millis(50))
        .await
        .unwrap_err();
    assert!(format!("{e:#}").contains("no sign-in came back"), "{e:#}");
}

#[tokio::test]
async fn sign_in_end_to_end_with_a_local_browser() {
    let http = FakeHttp::new(&[(
        200,
        &token_json(FULL_SCOPE, Some(REFRESH), "you@example.com"),
    )]);
    let browser = RefCell::new(None);
    let signed_in = sign_in(&http, &GOOGLE, &client(), Some("you@example.com"), |url| {
        // The browser: follow the URL's redirect_uri back with the state it carries.
        let query = url.split_once('?').unwrap().1;
        let param = |name: &str| {
            query
                .split('&')
                .find_map(|p| p.strip_prefix(&format!("{name}=")))
                .unwrap()
                .to_owned()
        };
        let state = param("state");
        let port: u16 = param("redirect_uri")
            .rsplit("%3A")
            .next()
            .unwrap()
            .parse()
            .unwrap();
        let request = format!("GET /?state={state}&code={CODE} HTTP/1.1\r\n\r\n");
        *browser.borrow_mut() = Some(tokio::spawn(
            async move { send(port, request.as_bytes()).await },
        ));
    })
    .await
    .unwrap();
    browser.into_inner().unwrap().await.unwrap();
    assert_eq!(signed_in.email, "you@example.com");
    assert_eq!(signed_in.refresh_token.expose(), REFRESH);
    let forms = http.forms.borrow();
    let (url, form) = &forms[0];
    assert_eq!(url, "https://oauth2.googleapis.com/token");
    assert_eq!(form["grant_type"], "authorization_code");
    assert_eq!(form["code"], CODE);
    assert_eq!(form["client_secret"], SECRET);
    assert_eq!(form["code_verifier"].len(), 43);
    assert!(form["redirect_uri"].starts_with("http://127.0.0.1:"));
    assert!(!format!("{signed_in:?}").contains(REFRESH));
}

async fn exchange(answer: (u16, &str)) -> Result<SignedIn> {
    let http = FakeHttp::new(&[answer]);
    let pkce = Pkce::new().unwrap();
    let code = Secret::new(CODE.into());
    exchange_code(
        &http,
        &GOOGLE,
        &client(),
        &code,
        &pkce,
        "http://127.0.0.1:1",
    )
    .await
}

#[tokio::test]
async fn token_responses_without_mail_scope_or_refresh_token_are_refused() {
    let no_mail = token_json(
        "openid https://www.googleapis.com/auth/userinfo.email",
        Some(REFRESH),
        "a@example.com",
    );
    let e = exchange((200, &no_mail)).await.unwrap_err();
    assert!(
        format!("{e:#}").contains("did not grant mail access"),
        "{e:#}"
    );
    let no_refresh = token_json(FULL_SCOPE, None, "a@example.com");
    let e = exchange((200, &no_refresh)).await.unwrap_err();
    assert!(format!("{e:#}").contains("no refresh token"), "{e:#}");
    let e = exchange((
        401,
        r#"{"error":"invalid_client","error_description":"Unauthorized"}"#,
    ))
    .await
    .unwrap_err();
    let message = format!("{e:#}");
    assert!(
        message.contains("invalid_client: Unauthorized; check the client ID"),
        "{message}"
    );
    let e = exchange((200, &format!("{{\"access_token\": \"{ACCESS}\"")))
        .await
        .unwrap_err();
    assert!(!format!("{e:#} {e:?}").contains(ACCESS), "{e:#}");
}

#[tokio::test]
async fn refresh_gets_an_access_token_and_explains_invalid_grant_and_invalid_client() {
    let http = FakeHttp::new(&[
        (
            200,
            &format!(r#"{{"access_token":"{ACCESS}","expires_in":3599}}"#),
        ),
        (
            400,
            r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#,
        ),
        (
            401,
            r#"{"error":"invalid_client","error_description":"Unauthorized"}"#,
        ),
    ]);
    let refresh_token = Secret::new(REFRESH.into());
    let refreshed = refresh(
        &http,
        &GOOGLE,
        &client(),
        &refresh_token,
        MailGrant::ReadWrite,
        "gmail",
    )
    .await
    .unwrap();
    assert_eq!(refreshed.access_token.expose(), ACCESS);
    assert_eq!(refreshed.expires_in, Some(Duration::from_secs(3599)));
    assert!(refreshed.refresh_token.is_none(), "Google does not rotate");
    let form = http.forms.borrow()[0].1.clone();
    assert_eq!(form["grant_type"], "refresh_token");
    assert_eq!(form["refresh_token"], REFRESH);
    assert_eq!(
        form["client_secret"], SECRET,
        "Gmail's form still sends the secret"
    );
    assert!(!form.contains_key("scope"));
    let e = refresh(
        &http,
        &GOOGLE,
        &client(),
        &refresh_token,
        MailGrant::ReadWrite,
        "gmail",
    )
    .await
    .unwrap_err();
    let message = format!("{e:#}");
    assert!(message.contains("expired or was revoked"), "{message}");
    assert!(message.contains("7 days"), "{message}");
    assert!(
        message.ends_with("run `neomail account login gmail`"),
        "{message}"
    );
    let e = refresh(
        &http,
        &GOOGLE,
        &client(),
        &refresh_token,
        MailGrant::ReadWrite,
        "gmail",
    )
    .await
    .unwrap_err();
    let message = format!("{e:#}");
    assert!(
        message.contains("check the client ID and secret"),
        "{message}"
    );
    assert!(
        message.ends_with("run `neomail account logout gmail`, then `neomail account login gmail`"),
        "{message}"
    );
}

#[tokio::test]
async fn revoke_posts_the_token() {
    let http = FakeHttp::new(&[(200, "{}"), (400, r#"{"error":"invalid_token"}"#)]);
    let token = Secret::new(REFRESH.into());
    revoke(&http, &GOOGLE, &token).await.unwrap();
    assert_eq!(
        http.forms.borrow()[0].0,
        "https://oauth2.googleapis.com/revoke"
    );
    assert_eq!(http.forms.borrow()[0].1["token"], REFRESH);
    let e = revoke(&http, &GOOGLE, &token).await.unwrap_err();
    assert_eq!(format!("{e:#}"), "invalid_token");
}

#[test]
fn id_token_email_claim() {
    assert_eq!(
        id_token_email(&id_token(
            r#"{"email":"you@example.com","email_verified":true}"#
        ))
        .unwrap(),
        "you@example.com"
    );
    assert!(id_token_email(&id_token(r#"{"sub":"1"}"#)).is_err());
    assert!(id_token_email(&id_token(r#"{"email":"a@b","email_verified":false}"#)).is_err());
    assert!(id_token_email("not-a-jwt").is_err());
    assert!(id_token_email("a.!!!.c").is_err());
}

#[test]
fn providers_are_data() {
    assert_eq!(
        provider("google").unwrap().mail,
        MailApi::Imap {
            host: "imap.gmail.com",
            port: 993
        }
    );
    assert_eq!(provider("microsoft").unwrap().mail, MailApi::Graph);
    assert!(provider("yahoo").is_none());
    assert_eq!(
        format!("{:?}", Secret::new(SECRET.into())),
        "Secret(<redacted>)"
    );
    assert!(!format!("{:?}", client()).contains(SECRET));
}

const DEVICE: &str = "GMQCQAFFAAEAAAAAAAAA-device-not-real";
const MS_REFRESH: &str = "M.C5-refresh-not-real";

/// Microsoft's token answer: `scope` is left out when `None`, and the ID token has only
/// `preferred_username`, as a personal account's may.
fn ms_token(scope: Option<&str>, refresh: &str) -> String {
    let scope = scope
        .map(|s| format!(r#""scope":"{s}","#))
        .unwrap_or_default();
    let id = id_token(r#"{"preferred_username":"you@example.com"}"#);
    format!(
        r#"{{"token_type":"Bearer",{scope}"access_token":"{ACCESS}","refresh_token":"{refresh}","id_token":"{id}"}}"#
    )
}

fn device_answer(extra: &str) -> String {
    format!(
        r#"{{"device_code":"{DEVICE}","user_code":"ABCD-EFGH","verification_uri":"https://microsoft.com/devicelogin"{extra}}}"#
    )
}

const PENDING: (u16, &str) = (400, r#"{"error":"authorization_pending"}"#);

async fn device_sign_in(http: &FakeHttp) -> (Result<SignedIn>, String) {
    let shown = RefCell::new(String::new());
    let result = device::sign_in(http, &MICROSOFT, &public_client(), |text| {
        *shown.borrow_mut() = text.to_owned();
    })
    .await;
    (result, shown.into_inner())
}

#[tokio::test(start_paused = true)]
async fn device_code_polls_at_the_interval_and_slow_down_adds_5_s_for_good() {
    let message = "To sign in, use a web browser to open the page \
                   https://microsoft.com/devicelogin and enter the code ABCD-EFGH.";
    let token = ms_token(
        Some("https://graph.microsoft.com/Mail.Read openid"),
        MS_REFRESH,
    );
    let http = FakeHttp::new(&[
        (
            200,
            &device_answer(&format!(
                r#","expires_in":900,"interval":5,"message":"{message}""#
            )),
        ),
        PENDING,
        (400, r#"{"error":"slow_down"}"#),
        PENDING,
        (200, &token),
    ]);
    let (result, shown) = device_sign_in(&http).await;
    let signed_in = result.unwrap();
    assert_eq!(shown, message);
    assert_eq!(signed_in.email, "you@example.com");
    assert_eq!(signed_in.refresh_token.expose(), MS_REFRESH);
    // Polls 5 s after the code, then 5, 10 and 10 s apart.
    assert_eq!(http.offsets(), [5, 10, 20, 30]);
    let forms = http.forms.borrow();
    let (url, form) = &forms[0];
    assert_eq!(
        url,
        "https://login.microsoftonline.com/common/oauth2/v2.0/devicecode"
    );
    assert_eq!(form["scope"], MICROSOFT.scope);
    assert_eq!(form["client_id"], public_client().id);
    for (url, form) in &forms[1..] {
        assert_eq!(url, MICROSOFT.token_url);
        assert_eq!(
            form["grant_type"],
            "urn:ietf:params:oauth:grant-type:device_code"
        );
        assert_eq!(form["device_code"], DEVICE);
        assert_eq!(form["client_id"], public_client().id);
    }
    assert!(forms.iter().all(|(_, f)| !f.contains_key("client_secret")));
}

#[tokio::test(start_paused = true)]
async fn device_code_stops_when_declined_expired_unknown_or_past_its_deadline() {
    let cases: [(&[(u16, &str)], &str); 4] = [
        (
            &[PENDING, (400, r#"{"error":"authorization_declined"}"#)],
            "the sign-in was declined",
        ),
        // RFC 8628's name for the same thing.
        (
            &[(400, r#"{"error":"access_denied"}"#)],
            "the sign-in was declined",
        ),
        (&[(400, r#"{"error":"expired_token"}"#)], "the code expired"),
        (
            &[(400, r#"{"error":"bad_verification_code"}"#)],
            "did not recognise the device code",
        ),
    ];
    for (polls, expected) in cases {
        let mut answers = vec![(200, device_answer(""))];
        answers.extend(polls.iter().map(|(s, b)| (*s, (*b).to_owned())));
        let answers: Vec<(u16, &str)> = answers.iter().map(|(s, b)| (*s, b.as_str())).collect();
        let http = FakeHttp::new(&answers);
        let (result, shown) = device_sign_in(&http).await;
        let e = result.unwrap_err();
        let message = format!("{e:#} {e:?}");
        assert!(message.contains(expected), "{message}");
        assert!(!message.contains(DEVICE), "{message}");
        // Without `message`, the URL and the code are shown.
        assert_eq!(
            shown,
            "To sign in, open https://microsoft.com/devicelogin and enter the code ABCD-EFGH."
        );
        // Default interval: 5 s.
        assert_eq!(http.offsets()[0], 5);
    }

    // expires_in 120 with a 50 s interval: polls at 50 and 100 s, then the deadline.
    let device = device_answer(r#","expires_in":120,"interval":50"#);
    let http = FakeHttp::new(&[(200, &device), PENDING, PENDING]);
    let (result, _) = device_sign_in(&http).await;
    let e = result.unwrap_err();
    assert!(
        format!("{e:#}").contains("did not finish within 2 minutes"),
        "{e:#}"
    );
    assert_eq!(http.offsets(), [50, 100]);
}

#[tokio::test(start_paused = true)]
async fn device_code_instructions_reach_the_terminal_without_control_characters() {
    let device =
        device_answer(r#","message":"\u001b[2JOpen https://microsoft.com/devicelogin\u0007""#);
    let token = ms_token(
        Some("https://graph.microsoft.com/Mail.Read openid"),
        MS_REFRESH,
    );
    let http = FakeHttp::new(&[(200, &device), (200, &token)]);
    let (result, shown) = device_sign_in(&http).await;
    result.unwrap();
    assert_eq!(shown, "[2JOpen https://microsoft.com/devicelogin");
}

#[tokio::test]
async fn a_device_code_error_names_the_cause_and_never_the_code() {
    let http = FakeHttp::new(&[(
        400,
        r#"{"error":"invalid_client","error_description":"AADSTS7000218: The request body must contain the following parameter: 'client_assertion' or 'client_secret'.","error_codes":[7000218]}"#,
    )]);
    let (result, _) = device_sign_in(&http).await;
    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.starts_with(
            "Microsoft refused the device code request: invalid_client: AADSTS7000218"
        ),
        "{message}"
    );
    assert!(
        message.ends_with("; probably the app registration's public client flows are off: set Authentication > Advanced settings > Allow public client flows to Yes"),
        "{message}"
    );
}

async fn exchange_with(provider: &Provider, client: &Client, answer: &str) -> Result<SignedIn> {
    let http = FakeHttp::new(&[(200, answer)]);
    let result = exchange_code(
        &http,
        provider,
        client,
        &Secret::new(CODE.into()),
        &Pkce::new().unwrap(),
        "http://localhost:1",
    )
    .await;
    // A confidential client without its secret fails before anything is sent.
    if let Some((_, form)) = http.forms.borrow().first() {
        assert_eq!(
            form.contains_key("client_secret"),
            provider.client == ClientKind::Confidential
        );
    }
    result
}

#[tokio::test]
async fn a_public_client_never_sends_a_secret_and_a_confidential_one_needs_one() {
    // Even a secret the client holds stays out of a public client's form.
    let holding = Client {
        secret: Some(Secret::new(SECRET.into())),
        ..public_client()
    };
    let token = ms_token(None, MS_REFRESH);
    let signed_in = exchange_with(&MICROSOFT, &holding, &token).await.unwrap();
    assert_eq!(signed_in.email, "you@example.com");
    let e = exchange_with(
        &GOOGLE,
        &Client {
            secret: None,
            ..client()
        },
        &token,
    )
    .await;
    assert!(format!("{:#}", e.unwrap_err()).contains("the Google client secret is missing"));
}

#[tokio::test]
async fn graph_scopes_are_matched_without_prefix_or_case() {
    assert!(
        grants_mail(&MICROSOFT, None),
        "missing scope: the requested ones"
    );
    assert!(grants_mail(
        &MICROSOFT,
        Some("openid https://graph.microsoft.com/mail.read")
    ));
    assert!(grants_mail(&MICROSOFT, Some("Mail.ReadWrite profile")));
    assert!(!grants_mail(
        &MICROSOFT,
        Some("openid profile email https://graph.microsoft.com/User.Read")
    ));
    assert!(!grants_mail(
        &MICROSOFT,
        Some("https://graph.microsoft.com/Mail.Send")
    ));
    assert!(grants_mail(&GOOGLE, Some(FULL_SCOPE)));
    assert!(!grants_mail(&GOOGLE, Some("openid email")));
    let no_mail = ms_token(
        Some("openid https://graph.microsoft.com/User.Read"),
        MS_REFRESH,
    );
    let e = exchange_with(&MICROSOFT, &public_client(), &no_mail)
        .await
        .unwrap_err();
    assert!(
        format!("{e:#}").contains(
            "did not grant mail access (https://graph.microsoft.com/Mail.Read or \
             https://graph.microsoft.com/Mail.ReadWrite)"
        ),
        "{e:#}"
    );
}

#[tokio::test]
async fn microsoft_refresh_asks_for_the_refresh_scope_and_returns_the_rotated_token() {
    let http = FakeHttp::new(&[
        (200, &ms_token(None, MS_REFRESH)),
        (
            400,
            r#"{"error":"invalid_grant","error_description":"AADSTS70000: The grant is expired.","error_codes":[70000]}"#,
        ),
    ]);
    let saved = Secret::new(REFRESH.into());
    let refreshed = refresh(
        &http,
        &MICROSOFT,
        &public_client(),
        &saved,
        MailGrant::Read,
        "outlook",
    )
    .await
    .unwrap();
    assert_eq!(refreshed.access_token.expose(), ACCESS);
    assert_eq!(refreshed.grant, MailGrant::Read);
    assert_eq!(refreshed.rotated(&saved).unwrap().expose(), MS_REFRESH);
    let debug = format!("{refreshed:?}");
    assert!(
        !debug.contains(ACCESS) && !debug.contains(MS_REFRESH),
        "{debug}"
    );
    let form = http.forms.borrow()[0].1.clone();
    assert_eq!(
        form["scope"],
        "offline_access https://graph.microsoft.com/Mail.Read"
    );
    assert!(!form.contains_key("client_secret"));
    let e = refresh(
        &http,
        &MICROSOFT,
        &public_client(),
        &saved,
        MailGrant::Read,
        "outlook",
    )
    .await
    .unwrap_err();
    let message = format!("{e:#}");
    assert!(message.contains("unused for 90 days"), "{message}");
    assert!(
        message.ends_with("run `neomail account login outlook`"),
        "{message}"
    );
}

#[tokio::test]
async fn expires_in_is_read_as_a_number_or_a_string_and_may_be_missing() {
    let http = FakeHttp::new(&[
        (
            200,
            &format!(r#"{{"access_token":"{ACCESS}","expires_in":"3600"}}"#),
        ),
        (200, &format!(r#"{{"access_token":"{ACCESS}"}}"#)),
        (
            200,
            &format!(r#"{{"access_token":"{ACCESS}","expires_in":"soon"}}"#),
        ),
    ]);
    let refresh_token = Secret::new(REFRESH.into());
    let mut got = Vec::new();
    for _ in 0..3 {
        let r = refresh(
            &http,
            &GOOGLE,
            &client(),
            &refresh_token,
            MailGrant::ReadWrite,
            "gmail",
        )
        .await
        .unwrap();
        got.push(r.expires_in);
    }
    assert_eq!(got, [Some(Duration::from_secs(3600)), None, None]);
}

#[test]
fn a_refresh_token_is_written_back_only_when_it_changed() {
    let saved = Secret::new(REFRESH.into());
    let with = |token: Option<&str>| Refreshed {
        access_token: Secret::new(ACCESS.into()),
        grant: MailGrant::Read,
        refresh_token: token.map(|t| Secret::new(t.into())),
        expires_in: None,
    };
    assert!(with(None).rotated(&saved).is_none());
    assert!(with(Some(REFRESH)).rotated(&saved).is_none());
    assert_eq!(
        with(Some(MS_REFRESH)).rotated(&saved).unwrap().expose(),
        MS_REFRESH
    );
}

#[tokio::test]
async fn revoke_fails_without_a_revocation_endpoint() {
    let http = FakeHttp::new(&[]);
    let e = revoke(&http, &MICROSOFT, &Secret::new(REFRESH.into()))
        .await
        .unwrap_err();
    assert_eq!(
        format!("{e:#}"),
        "Microsoft has no token revocation endpoint"
    );
    assert!(http.forms.borrow().is_empty());
}

#[test]
fn the_address_falls_back_to_preferred_username() {
    let token = |claims: &str| id_token_email(&id_token(claims));
    assert_eq!(
        token(r#"{"preferred_username":"you@example.com"}"#).unwrap(),
        "you@example.com"
    );
    assert_eq!(
        token(r#"{"email":"a@example.com","preferred_username":"b@example.com"}"#).unwrap(),
        "a@example.com"
    );
    assert!(token(r#"{"preferred_username":"not-an-address"}"#).is_err());
}

#[test]
fn microsoft_is_offered_as_a_public_graph_client() {
    assert_eq!(provider("microsoft"), Some(&MICROSOFT));
    assert_eq!(MICROSOFT.mail, MailApi::Graph);
    assert_eq!(MICROSOFT.client, ClientKind::Public);
    assert!(MICROSOFT.revoke_url.is_none());
    assert!(GOOGLE.device_code_url.is_none());
    assert!(MICROSOFT.auth_params.is_empty(), "no prompt=consent");
}

#[test]
fn the_client_id_prompt_points_at_the_step_that_shows_the_id() {
    for p in PROVIDERS {
        let step = p
            .setup
            .client_id_prompt
            .split("step ")
            .nth(1)
            .and_then(|rest| rest.split(')').next())
            .unwrap();
        let text = p
            .setup
            .steps
            .split(&format!("\n {step}. "))
            .nth(1)
            .unwrap_or_else(|| panic!("{}: no step {step}", p.id));
        let text = text.split("\n 6. ").next().unwrap();
        assert!(
            text.contains("client ID") || text.contains("(client) ID"),
            "{}: {text}",
            p.id
        );
        assert!(p.setup.steps.contains("GUIDE"), "{}", p.id);
    }
}

/// Microsoft's refresh scope must be the original's or a subset
/// (<https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-auth-code-flow>): a
/// sign-in that granted Mail.Read is never refreshed with Mail.ReadWrite, and one that granted
/// Mail.ReadWrite keeps it.
#[tokio::test]
async fn microsoft_refreshes_each_sign_in_for_no_more_than_it_granted() {
    let saved = Secret::new(REFRESH.into());
    for (grant, want) in [
        (
            MailGrant::Read,
            "offline_access https://graph.microsoft.com/Mail.Read",
        ),
        (
            MailGrant::ReadWrite,
            "offline_access https://graph.microsoft.com/Mail.ReadWrite",
        ),
    ] {
        let http = FakeHttp::new(&[(200, &ms_token(None, MS_REFRESH))]);
        let refreshed = refresh(
            &http,
            &MICROSOFT,
            &public_client(),
            &saved,
            grant,
            "outlook",
        )
        .await
        .unwrap();
        assert_eq!(refreshed.grant, grant);
        assert_eq!(http.forms.borrow()[0].1["scope"], want);
    }
    // Google sends no scope with a refresh, whatever the grant.
    for grant in [MailGrant::Read, MailGrant::ReadWrite] {
        let http = FakeHttp::new(&[(200, &format!(r#"{{"access_token":"{ACCESS}"}}"#))]);
        refresh(&http, &GOOGLE, &client(), &saved, grant, "gmail")
            .await
            .unwrap();
        assert!(!http.forms.borrow()[0].1.contains_key("scope"));
    }
}

/// A new sign-in asks for Mail.ReadWrite and Mail.Read, and what it records follows what
/// Microsoft granted. A refresh may ask only for what the sign-in asked for or a subset
/// (<https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-auth-code-flow>), so
/// this version's refreshes and the previous release's (`offline_access` and Mail.Read) all stay
/// inside what a new sign-in asks for.
#[test]
fn a_sign_in_records_whether_it_may_change_mail() {
    let asked: Vec<&str> = MICROSOFT.scope.split_ascii_whitespace().collect();
    for scope in [
        "https://graph.microsoft.com/Mail.ReadWrite",
        "https://graph.microsoft.com/Mail.Read",
    ] {
        assert!(asked.contains(&scope), "{asked:?}");
    }
    let refresh = MICROSOFT.refresh_scope.as_ref().unwrap();
    for refreshed in [
        "offline_access https://graph.microsoft.com/Mail.Read",
        refresh.for_grant(MailGrant::Read),
        refresh.for_grant(MailGrant::ReadWrite),
    ] {
        assert!(
            refreshed
                .split_ascii_whitespace()
                .all(|s| asked.contains(&s)),
            "{refreshed:?} asks for more than {asked:?}"
        );
    }
    let grant = |provider: &Provider, answer: String| {
        check_sign_in(provider, serde_json::from_str(&answer).unwrap())
            .unwrap()
            .grant
    };
    let ms = |scope| grant(&MICROSOFT, ms_token(scope, REFRESH));
    assert_eq!(
        ms(Some("https://graph.microsoft.com/Mail.ReadWrite openid")),
        MailGrant::ReadWrite
    );
    assert_eq!(
        ms(Some("https://graph.microsoft.com/Mail.Read openid")),
        MailGrant::Read
    );
    // No scope in the answer means the scopes asked for.
    assert_eq!(ms(None), MailGrant::ReadWrite);
    let google = token_json(FULL_SCOPE, Some(REFRESH), "you@example.com");
    assert_eq!(grant(&GOOGLE, google), MailGrant::ReadWrite);
}
