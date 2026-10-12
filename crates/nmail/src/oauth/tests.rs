#![allow(clippy::unwrap_used)] // Test helpers outside #[test] functions.

use std::cell::RefCell;
use std::collections::HashMap;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::http::Response;
use super::loopback::{Listener, Request, parse_request};
use super::*;

const SECRET: &str = "GOCSPX-not-a-real-secret";
const REFRESH: &str = "1//refresh-not-real";
const ACCESS: &str = "ya29.access-not-real";
const CODE: &str = "4/0code-not-real";

/// A token endpoint that answers from a script and records each form it got.
struct FakeHttp {
    answers: RefCell<Vec<(u16, String)>>,
    forms: RefCell<Vec<(String, HashMap<String, String>)>>,
}

impl FakeHttp {
    fn new(answers: &[(u16, &str)]) -> Self {
        Self {
            answers: RefCell::new(answers.iter().map(|(s, b)| (*s, (*b).to_owned())).collect()),
            forms: RefCell::default(),
        }
    }
}

impl http::Http for FakeHttp {
    async fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<Response> {
        let form = form
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        self.forms.borrow_mut().push((url.to_owned(), form));
        let (status, body) = self.answers.borrow_mut().remove(0);
        Ok(Response {
            status,
            body: body.into_bytes(),
        })
    }
}

fn client() -> Client {
    Client {
        id: "1234-abc.apps.googleusercontent.com".into(),
        secret: Secret::new(SECRET.into()),
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
    let listener = Listener::bind().await.unwrap();
    let uri = listener.redirect_uri();
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

#[tokio::test]
async fn listener_reports_a_declined_sign_in_and_times_out() {
    let listener = Listener::bind().await.unwrap();
    let port: u16 = listener
        .redirect_uri()
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
async fn refresh_gets_an_access_token_and_explains_invalid_grant() {
    let http = FakeHttp::new(&[
        (
            200,
            &format!(r#"{{"access_token":"{ACCESS}","expires_in":3599}}"#),
        ),
        (
            400,
            r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#,
        ),
    ]);
    let refresh_token = Secret::new(REFRESH.into());
    let access = refresh(&http, &GOOGLE, &client(), &refresh_token, "gmail")
        .await
        .unwrap();
    assert_eq!(access.expose(), ACCESS);
    assert_eq!(http.forms.borrow()[0].1["grant_type"], "refresh_token");
    assert_eq!(http.forms.borrow()[0].1["refresh_token"], REFRESH);
    let e = refresh(&http, &GOOGLE, &client(), &refresh_token, "gmail")
        .await
        .unwrap_err();
    let message = format!("{e:#}");
    assert!(message.contains("expired or was revoked"), "{message}");
    assert!(message.contains("7 days"), "{message}");
    assert!(
        message.ends_with("run `nmail account login gmail`"),
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
    assert_eq!(provider("google").unwrap().imap_host, "imap.gmail.com");
    assert!(provider("microsoft").is_none());
    assert_eq!(
        format!("{:?}", Secret::new(SECRET.into())),
        "Secret(<redacted>)"
    );
    assert!(!format!("{:?}", client()).contains(SECRET));
}
