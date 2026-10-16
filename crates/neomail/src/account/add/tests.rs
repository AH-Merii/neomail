//! `account add gmail|outlook` against scripted input, a mock keyring, a scripted HTTP client
//! for Google's and Microsoft's endpoints (as oauth/tests.rs fakes them: nothing leaves the
//! process), a fake sign-in, and fake IMAP and SMTP servers on 127.0.0.1 with TLS and a pinned
//! self-signed certificate.

#![allow(clippy::unwrap_used)] // Test helpers outside #[test] functions.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use keyring_core::mock;
use nm_imap::{CertPin, Security};
use sha2::{Digest, Sha256};

use super::*;
use crate::config::SmtpSettings;
use crate::http::Response;
use crate::oauth::{GOOGLE, MICROSOFT, MailGrant};
use crate::wizard::script::{self, Script};
use crate::wizard::term::Key;
use crate::wizard::{Event, Kind, Prompt};

const GMAIL_ID: &str = "1234-abc.apps.googleusercontent.com";
const OUTLOOK_ID: &str = "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0";
const CLIENT_SECRET: &str = "GOCSPX-not-a-real-secret";
const ACCESS: &str = "ya29.access-not-real";
const GOOGLE_KNOWN: (u16, &str) = (
    400,
    r#"{"error":"invalid_grant","error_description":"Bad Request"}"#,
);
const MICROSOFT_KNOWN: (u16, &str) = (
    200,
    r#"{"device_code":"dc","user_code":"UC","verification_uri":"https://example.com","expires_in":900,"interval":5,"message":"m"}"#,
);

/// Google's and Microsoft's endpoints: plays the scripted answers in order and records each
/// request. A request with no answer left fails the test.
#[derive(Default)]
struct FakeHttp {
    answers: RefCell<VecDeque<(u16, String)>>,
    seen: RefCell<Vec<(String, HashMap<String, String>)>>,
}

impl FakeHttp {
    fn new(answers: &[(u16, &str)]) -> Self {
        Self {
            answers: RefCell::new(answers.iter().map(|(s, b)| (*s, (*b).to_owned())).collect()),
            seen: RefCell::default(),
        }
    }

    fn seen(&self) -> Vec<(String, HashMap<String, String>)> {
        self.seen.borrow().clone()
    }
}

impl Http for FakeHttp {
    async fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<Response> {
        let form = form
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        self.seen.borrow_mut().push((url.to_owned(), form));
        let (status, body) = self
            .answers
            .borrow_mut()
            .pop_front()
            .unwrap_or_else(|| panic!("an unexpected request to {url}"));
        Ok(Response {
            status,
            retry_after: None,
            body: body.into_bytes(),
        })
    }
}

/// A TLS server on 127.0.0.1 that runs `serve` on each connection; what it logs is in `seen`.
struct Fake {
    port: u16,
    pin: CertPin,
    seen: Arc<Mutex<Vec<String>>>,
}

type Conn = BufReader<rustls::StreamOwned<rustls::ServerConnection, std::net::TcpStream>>;

impl Fake {
    fn start(serve: impl Fn(&mut Conn, &Mutex<Vec<String>>) + Send + 'static) -> Self {
        let rcgen::CertifiedKey { cert, signing_key } =
            rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_owned()]).unwrap();
        let hex: String = Sha256::digest(cert.der().as_ref())
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
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        std::thread::spawn(move || {
            for tcp in listener.incoming() {
                let Ok(tcp) = tcp else { return };
                let tls = rustls::ServerConnection::new(Arc::clone(&config)).unwrap();
                serve(
                    &mut BufReader::new(rustls::StreamOwned::new(tls, tcp)),
                    &log,
                );
            }
        });
        Self {
            port,
            pin: hex.parse().unwrap(),
            seen,
        }
    }

    fn seen(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

fn reply<S: Read + std::io::Write>(s: &mut BufReader<S>, text: &str) -> bool {
    s.get_mut().write_all(text.as_bytes()).is_ok()
}

fn read_line<S: Read + std::io::Write>(s: &mut BufReader<S>) -> Option<String> {
    let mut line = String::new();
    (s.read_line(&mut line).unwrap_or(0) > 0).then(|| line.trim_end().to_owned())
}

/// The XOAUTH2 string's user and token, as the log shows them.
fn xoauth2(base64: &str) -> String {
    let raw = String::from_utf8(B64.decode(base64.trim()).unwrap()).unwrap();
    raw.trim_end_matches('\x01').replace('\x01', " ")
}

/// IMAP that takes XOAUTH2 with the token `accept` only; logs `XOAUTH2 user=… auth=Bearer …`
/// and the other verbs.
fn imap(accept: &'static str) -> Fake {
    Fake::start(move |s, log| {
        if !reply(s, "* OK fake IMAP\r\n") {
            return;
        }
        while let Some(line) = read_line(s) {
            let mut words = line.split(' ');
            let tag = words.next().unwrap_or_default().to_owned();
            let verb = words.next().unwrap_or_default().to_ascii_uppercase();
            let answer = if verb == "AUTHENTICATE" {
                let response = match words.nth(1) {
                    Some(inline) => inline.to_owned(),
                    None => {
                        reply(s, "+ \r\n");
                        read_line(s).unwrap_or_default()
                    }
                };
                let login = xoauth2(&response);
                log.lock().unwrap().push(format!("XOAUTH2 {login}"));
                if login.ends_with(&format!("Bearer {accept}")) {
                    format!("{tag} OK authenticated\r\n")
                } else {
                    reply(s, "+ eyJzdGF0dXMiOiI0MDAifQ==\r\n");
                    read_line(s);
                    format!("{tag} NO [AUTHENTICATIONFAILED] Invalid credentials\r\n")
                }
            } else {
                log.lock().unwrap().push(verb.clone());
                match verb.as_str() {
                    "EXAMINE" => format!(
                        "* 3 EXISTS\r\n* OK [UIDVALIDITY 7] ok\r\n{tag} OK [READ-ONLY] done\r\n"
                    ),
                    "LOGOUT" => format!("* BYE\r\n{tag} OK bye\r\n"),
                    _ => format!("{tag} OK\r\n"),
                }
            };
            if !reply(s, &answer) || verb == "LOGOUT" {
                return;
            }
        }
    })
}

/// SMTP that takes XOAUTH2 with the token `accept` only; logs each verb and the login.
fn smtp(accept: &'static str) -> Fake {
    Fake::start(move |s, log| {
        if !reply(s, "220 fake ESMTP\r\n") {
            return;
        }
        while let Some(line) = read_line(s) {
            let mut words = line.split(' ');
            let verb = words.next().unwrap_or_default().to_ascii_uppercase();
            let answer = match (verb.as_str(), words.next(), words.next()) {
                ("AUTH", Some("XOAUTH2"), Some(response)) => {
                    let login = xoauth2(response);
                    log.lock().unwrap().push(format!("XOAUTH2 {login}"));
                    if login.ends_with(&format!("Bearer {accept}")) {
                        "235 ok\r\n"
                    } else {
                        "535 5.7.8 Username and Password not accepted\r\n"
                    }
                }
                ("EHLO", ..) => "250-fake\r\n250 AUTH XOAUTH2 PLAIN\r\n",
                ("QUIT", ..) => "221 bye\r\n",
                _ => "250 ok\r\n",
            };
            if !verb.starts_with("AUTH") {
                log.lock().unwrap().push(verb.clone());
            }
            if !reply(s, answer) || verb == "QUIT" {
                return;
            }
        }
    })
}

/// What a sign-in as `email` brings.
fn signed(email: &str) -> SignedIn {
    SignedIn {
        email: email.to_owned(),
        refresh_token: Secret::new("1//refresh-not-real".into()),
        grant: MailGrant::ReadWriteSend,
        access_token: Some(Secret::new(ACCESS.into())),
    }
}

/// A config directory, a mock keyring, the endpoints and the two mail servers.
struct World {
    dir: tempfile::TempDir,
    store: Arc<mock::Store>,
    keyring_fails: bool,
    http: FakeHttp,
    imap: Fake,
    smtp: Fake,
    /// The client ID, the secret and the flow of each sign-in.
    sign_ins: RefCell<Vec<(String, Option<String>, Flow)>>,
    graph_checks: RefCell<usize>,
}

impl World {
    fn new(http: &[(u16, &str)]) -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
            store: mock::Store::new().unwrap(),
            keyring_fails: false,
            http: FakeHttp::new(http),
            imap: imap(ACCESS),
            smtp: smtp(ACCESS),
            sign_ins: RefCell::default(),
            graph_checks: RefCell::default(),
        }
    }

    fn config(&self) -> std::path::PathBuf {
        self.dir.path().join("config.toml")
    }

    fn keyring(&self) -> Keyring {
        Keyring::with_store(self.store.clone(), "the mock", false)
    }

    /// Runs the wizard for `provider`; `sign_in` stands for the browser or the device code.
    /// Ok is what it printed on standard output.
    fn run(
        &self,
        provider: &'static Provider,
        flow: Flow,
        args: AddArgs,
        term: &mut Script,
        sign_in: impl Fn(&str) -> Result<SignedIn>,
    ) -> Result<String> {
        let open = || {
            if self.keyring_fails {
                anyhow::bail!("the OS keyring is unavailable (no Secret Service)");
            }
            Ok(self.keyring())
        };
        let fake_sign_in = |client: &oauth::Client, email: &str, flow: Flow| {
            self.sign_ins.borrow_mut().push((
                client.id.clone(),
                client.secret.as_ref().map(|s| s.expose().to_owned()),
                flow,
            ));
            sign_in(email)
        };
        let graph = |token: &Secret| {
            assert_eq!(token.expose(), ACCESS);
            *self.graph_checks.borrow_mut() += 1;
            Ok(42)
        };
        let gmail = Servers {
            imap: ImapConfig {
                host: "127.0.0.1".into(),
                port: self.imap.port,
                security: Security::Tls,
                username: String::new(),
                cert_pin: Some(self.imap.pin),
            },
            smtp: SmtpSettings {
                host: "127.0.0.1".into(),
                port: self.smtp.port,
                security: Security::Tls,
                cert_pin: Some(self.smtp.pin),
                save_sent: false,
            },
        };
        let deps = Deps {
            open_keyring: &open,
            http: &self.http,
            sign_in: &fake_sign_in,
            gmail,
            graph_count: &graph,
        };
        let input = Input {
            terminal: term.interactive,
            no_input: false,
        };
        let mut out = Vec::new();
        run(
            provider,
            flow,
            &self.config(),
            args,
            input,
            &mut script::ui(term),
            &deps,
            &mut out,
        )?;
        Ok(String::from_utf8(out).unwrap())
    }

    fn assert_nothing_written(&self, name: &str) {
        assert!(!self.config().exists(), "no config");
        assert!(
            self.keyring().get_oauth(name).unwrap().is_none(),
            "no sign-in"
        );
    }
}

fn gmail_args() -> AddArgs {
    AddArgs {
        email: Some("you@example.com".into()),
        client_id: Some(GMAIL_ID.into()),
        ..AddArgs::default()
    }
}

fn outlook_args() -> AddArgs {
    AddArgs {
        email: Some("you@outlook.example".into()),
        client_id: Some(OUTLOOK_ID.into()),
        ..AddArgs::default()
    }
}

/// An error that a real token exchange makes of the endpoint's `answer`.
fn exchange_error(provider: &Provider, answer: (u16, &str)) -> anyhow::Error {
    let http = FakeHttp::new(&[answer]);
    let client = oauth::Client {
        id: "id".into(),
        secret: Some(Secret::new(CLIENT_SECRET.into())),
    };
    let pkce = oauth::Pkce::new().unwrap();
    crate::account::runtime()
        .unwrap()
        .block_on(oauth::exchange_code(
            &http,
            provider,
            &client,
            &Secret::new("code".into()),
            &pkce,
            "http://127.0.0.1:1",
        ))
        .unwrap_err()
}

#[test]
fn gmail_from_flags_checks_the_client_and_both_mail_logins_then_writes() {
    let w = World::new(&[GOOGLE_KNOWN]);
    let mut term = Script::off_terminal(&[CLIENT_SECRET]);
    let out = w
        .run(&GOOGLE, Flow::Browser, gmail_args(), &mut term, |e| {
            Ok(signed(e))
        })
        .unwrap();
    assert!(
        out.starts_with("Added \"gmail\" (you@example.com) to "),
        "{out}"
    );
    // The client check: a token request with a made-up code, to Google's token endpoint.
    let seen = w.http.seen();
    assert_eq!(seen.len(), 1, "{seen:?}");
    let (url, form) = &seen[0];
    assert_eq!(url, "https://oauth2.googleapis.com/token");
    assert_eq!(form["grant_type"], "authorization_code");
    assert_eq!(form["client_id"], GMAIL_ID);
    assert_eq!(form["client_secret"], CLIENT_SECRET);
    // Both mail logins with the new sign-in's token, reading and sending nothing.
    let login = format!("XOAUTH2 user=you@example.com auth=Bearer {ACCESS}");
    assert_eq!(w.imap.seen(), [login.as_str(), "EXAMINE", "LOGOUT"]);
    assert_eq!(w.smtp.seen(), ["EHLO", login.as_str(), "QUIT"]);
    // The console steps are skipped off a terminal; what is written is what it was before.
    assert!(!term.said.contains("Google Cloud project"), "{}", term.said);
    let config = std::fs::read_to_string(w.config()).unwrap();
    assert!(
        config.contains(&format!("client_id = \"{GMAIL_ID}\"")),
        "{config}"
    );
    let entry = w.keyring().get_oauth("gmail").unwrap().unwrap();
    assert_eq!(entry.client_secret.as_deref(), Some(CLIENT_SECRET));
}

#[test]
fn the_keyring_and_the_config_are_checked_before_the_client_secret_is_asked() {
    // The keyring does not open.
    let mut w = World::new(&[]);
    w.keyring_fails = true;
    let mut term = Script::off_terminal(&[CLIENT_SECRET]);
    let e = w
        .run(&GOOGLE, Flow::Browser, gmail_args(), &mut term, |e| {
            Ok(signed(e))
        })
        .unwrap_err();
    assert!(
        e.to_string().contains("the OS keyring is unavailable"),
        "{e}"
    );
    assert_eq!(term.secrets_asked, 0);
    // The name is taken.
    let w = World::new(&[]);
    std::fs::write(
        w.config(),
        "[[accounts]]\nname = \"gmail\"\nusername = \"a@example.com\"\n\
         oauth = { provider = \"google\", client_id = \"x\" }\n",
    )
    .unwrap();
    let mut term = Script::off_terminal(&[CLIENT_SECRET]);
    let e = w
        .run(&GOOGLE, Flow::Browser, gmail_args(), &mut term, |e| {
            Ok(signed(e))
        })
        .unwrap_err();
    assert!(
        e.to_string()
            .contains("already has an account named \"gmail\""),
        "{e}"
    );
    assert_eq!(term.secrets_asked, 0);
    assert!(w.http.seen().is_empty() && w.sign_ins.borrow().is_empty());
}

#[test]
fn google_answers_to_the_client_check_name_the_step_to_change() {
    // (Google's answer, the step it names) from the live check of 2026-10-13.
    let cases = [
        (
            (
                401,
                r#"{"error":"invalid_client","error_description":"The OAuth client was not found."}"#,
            ),
            "step 12 of 17 (Paste the client ID): Google answered: invalid_client: The OAuth client was not found.",
        ),
        (
            (
                401,
                r#"{"error":"invalid_client","error_description":"The provided client secret is invalid."}"#,
            ),
            "step 13 of 17 (Paste the client secret): Google answered:",
        ),
        (
            (
                400,
                r#"{"error":"invalid_request","error_description":"client_secret is missing."}"#,
            ),
            "step 13 of 17 (Paste the client secret):",
        ),
    ];
    for (answer, want) in cases {
        let w = World::new(&[answer]);
        let mut term = Script::off_terminal(&[CLIENT_SECRET]);
        let e = w
            .run(&GOOGLE, Flow::Browser, gmail_args(), &mut term, |e| {
                Ok(signed(e))
            })
            .unwrap_err();
        assert!(e.to_string().starts_with(want), "{want}\n{e}");
        assert!(
            w.sign_ins.borrow().is_empty(),
            "no sign-in after a failed check"
        );
        w.assert_nothing_written("gmail");
    }
}

#[test]
fn an_answer_that_tells_nothing_lets_the_setup_go_on() {
    let w = World::new(&[(503, "<html>busy</html>")]);
    let mut term = Script::off_terminal(&[CLIENT_SECRET]);
    w.run(&GOOGLE, Flow::Browser, gmail_args(), &mut term, |e| {
        Ok(signed(e))
    })
    .unwrap();
    assert!(
        term.said
            .contains("does not tell whether the client is right"),
        "{}",
        term.said
    );
    assert_eq!(w.sign_ins.borrow().len(), 1);
}

#[test]
fn on_a_terminal_a_wrong_client_id_is_asked_again_and_the_secret_kept() {
    let unknown = (
        401,
        r#"{"error":"invalid_client","error_description":"The OAuth client was not found."}"#,
    );
    let w = World::new(&[unknown, GOOGLE_KNOWN]);
    let args = AddArgs {
        email: Some("you@example.com".into()),
        project: Some("my-project-42".into()),
        name: Some("mine".into()),
        ..AddArgs::default()
    };
    // Enter at the console steps, then a wrong ID, then the right one.
    let enter = [""; 7];
    let lines: Vec<&str> = [&enter[..], &["wrong.apps.googleusercontent.com", GMAIL_ID]].concat();
    let mut term = Script::terminal(&lines, &[CLIENT_SECRET]);
    w.run(&GOOGLE, Flow::Browser, args, &mut term, |e| Ok(signed(e)))
        .unwrap();
    let ids: Vec<String> = w
        .http
        .seen()
        .iter()
        .map(|(_, f)| f["client_id"].clone())
        .collect();
    assert_eq!(ids, ["wrong.apps.googleusercontent.com", GMAIL_ID]);
    assert_eq!(term.secrets_asked, 1, "the secret is kept");
    assert!(
        term.said.contains("Google knows no client with this ID"),
        "{}",
        term.said
    );
    assert!(w.keyring().get_oauth("mine").unwrap().is_some());
}

#[test]
fn a_refused_mail_login_writes_nothing() {
    for refusing in ["imap", "smtp"] {
        let mut w = World::new(&[GOOGLE_KNOWN]);
        match refusing {
            "imap" => w.imap = imap("another-token"),
            _ => w.smtp = smtp("another-token"),
        }
        let mut term = Script::off_terminal(&[CLIENT_SECRET]);
        let e = w
            .run(&GOOGLE, Flow::Browser, gmail_args(), &mut term, |e| {
                Ok(signed(e))
            })
            .unwrap_err();
        assert!(
            e.to_string()
                .starts_with("step 16 of 17 (Check the mail login): "),
            "{refusing}: {e}"
        );
        w.assert_nothing_written("gmail");
    }
}

#[test]
fn no_check_skips_the_client_and_mail_checks_but_not_the_sign_in() {
    let w = World::new(&[]);
    let args = AddArgs {
        no_check: true,
        ..gmail_args()
    };
    let mut term = Script::off_terminal(&[CLIENT_SECRET]);
    w.run(&GOOGLE, Flow::Browser, args, &mut term, |e| Ok(signed(e)))
        .unwrap();
    assert!(w.http.seen().is_empty());
    assert!(w.imap.seen().is_empty() && w.smtp.seen().is_empty());
    assert_eq!(w.sign_ins.borrow().len(), 1);
    assert!(w.keyring().get_oauth("gmail").unwrap().is_some());
}

#[test]
fn a_gmail_sign_in_that_times_out_names_the_steps_google_shows_mistakes_for() {
    let w = World::new(&[GOOGLE_KNOWN]);
    let mut term = Script::off_terminal(&[CLIENT_SECRET]);
    let e = w
        .run(&GOOGLE, Flow::Browser, gmail_args(), &mut term, |_| {
            Err(SignInError::TimedOut("no sign-in came back within 5 minutes".into()).into())
        })
        .unwrap_err()
        .to_string();
    for step in [
        "the step \"Add yourself as a test user\"",
        "the step \"Create a Desktop app client\"",
        "the step \"Data Access: add the mail scope\"",
    ] {
        assert!(e.contains(step), "{step}: {e}");
    }
    assert!(
        e.starts_with("step 6 of 17 (Add yourself as a test user): no sign-in came back"),
        "{e}"
    );
    w.assert_nothing_written("gmail");
}

#[test]
fn gmail_sign_in_errors_name_their_step() {
    let no_mail_scope = format!(
        r#"{{"access_token":"a","refresh_token":"r","scope":"openid email","id_token":"x.{}.y"}}"#,
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"email":"you@example.com"}"#)
    );
    let cases: [(anyhow::Error, &str); 4] = [
        (
            exchange_error(&GOOGLE, (200, &no_mail_scope)),
            "step 7 of 17 (Data Access: add the mail scope)",
        ),
        (
            exchange_error(&GOOGLE, (401, r#"{"error":"invalid_client"}"#)),
            "step 13 of 17 (Paste the client secret)",
        ),
        (
            SignInError::Declined {
                message: "access was declined in the browser; nothing was saved",
                description: None,
            }
            .into(),
            "step 8 of 17 (Testing or published: choose now)",
        ),
        (
            SignInError::Redirected {
                error: "redirect_uri_mismatch".into(),
                description: None,
            }
            .into(),
            "step 11 of 17 (Create a Desktop app client)",
        ),
    ];
    for (error, want) in cases {
        let w = World::new(&[GOOGLE_KNOWN]);
        let error = RefCell::new(Some(error));
        let mut term = Script::off_terminal(&[CLIENT_SECRET]);
        let e = w
            .run(&GOOGLE, Flow::Browser, gmail_args(), &mut term, |_| {
                Err(error.borrow_mut().take().unwrap())
            })
            .unwrap_err();
        assert!(e.to_string().starts_with(want), "{want}\n{e}");
        w.assert_nothing_written("gmail");
    }
    // Another Google account in the browser names the address.
    let w = World::new(&[GOOGLE_KNOWN]);
    let mut term = Script::off_terminal(&[CLIENT_SECRET]);
    let e = w
        .run(&GOOGLE, Flow::Browser, gmail_args(), &mut term, |_| {
            Ok(signed("other@example.com"))
        })
        .unwrap_err();
    assert!(
        e.to_string()
            .starts_with("step 5 of 17 (Your Gmail address): You signed in as other@example.com"),
        "{e}"
    );
}

#[test]
fn on_a_terminal_the_steps_link_the_project_and_explain_the_choices_when_made() {
    for (project, link) in [
        (
            "my-project-42",
            "https://console.developers.google.com/auth/clients?project=my-project-42",
        ),
        ("", "https://console.developers.google.com/auth/clients\n"),
    ] {
        let w = World::new(&[GOOGLE_KNOWN]);
        // project, the ID, API, branding, address, test user, scope, publishing, name, client,
        // client ID.
        let lines = [
            "",
            project,
            "",
            "",
            "you@example.com",
            "",
            "",
            "",
            "",
            "",
            GMAIL_ID,
        ];
        let mut term = Script::terminal(&lines, &[CLIENT_SECRET]);
        w.run(&GOOGLE, Flow::Browser, AddArgs::default(), &mut term, |e| {
            Ok(signed(e))
        })
        .unwrap();
        let said = &term.said;
        assert!(said.contains(link), "{link}\n{said}");
        let at = |text: &str| said.find(text).unwrap_or_else(|| panic!("{text}\n{said}"));
        // The publishing choice, with its effects, comes before the client is created.
        let publishing = at("Step 8/12 · Testing or published");
        assert!(publishing < at("Step 10/12 · Create a Desktop app client"));
        for effect in ["after 7 days", "100 users", "Advanced Protection"] {
            assert!(at(effect) > publishing, "{effect}");
        }
        assert!(at("only this once") > at("Step 10/12"));
    }
}

#[test]
fn a_leftover_sign_in_is_revoked_before_the_new_one() {
    let w = World::new(&[GOOGLE_KNOWN, (200, "")]);
    let leftover = OAuthEntry {
        client_secret: Some(CLIENT_SECRET.into()),
        refresh_token: Some("1//old".into()),
        mail_grant: None,
    };
    w.keyring().set_oauth("gmail", &leftover).unwrap();
    let mut term = Script::off_terminal(&[CLIENT_SECRET]);
    w.run(&GOOGLE, Flow::Browser, gmail_args(), &mut term, |e| {
        // By the time the browser opens, the old token is revoked at Google.
        let seen = w.http.seen();
        assert_eq!(
            seen.last().unwrap().0,
            "https://oauth2.googleapis.com/revoke"
        );
        assert_eq!(seen.last().unwrap().1["token"], "1//old");
        Ok(signed(e))
    })
    .unwrap();
    assert!(
        term.said.contains("revoked and deleted it"),
        "{}",
        term.said
    );
}

/// On a terminal the leftover is replaced only after a yes, and the sign-in after it runs in
/// cooked mode, so Ctrl-C stops it and its lines print below the question. Esc at the question
/// quits: nothing is revoked, signed in or written.
#[test]
fn on_a_terminal_a_leftover_is_replaced_after_a_yes_and_esc_quits() {
    let leftover = OAuthEntry {
        client_secret: Some(CLIENT_SECRET.into()),
        refresh_token: Some("1//old".into()),
        mail_grant: None,
    };
    for (key, replaced) in [(Key::Char('y'), true), (Key::Esc, false)] {
        let w = World::new(&[GOOGLE_KNOWN, (200, "")]);
        w.keyring().set_oauth("gmail", &leftover).unwrap();
        let mut term = Script::terminal(&[""; 20], &[CLIENT_SECRET]);
        term.confirms.push_back(key);
        let can_print = term.can_print.clone();
        let out = w.run(&GOOGLE, Flow::Browser, gmail_args(), &mut term, |e| {
            assert!(
                can_print.get(),
                "the sign-in runs in raw mode, or over the screen"
            );
            Ok(signed(e))
        });
        assert!(term.said.contains("Revoke and delete it?"), "{}", term.said);
        let revoked = w
            .http
            .seen()
            .iter()
            .any(|(url, _)| url.ends_with("/revoke"));
        assert_eq!(revoked, replaced, "{}", term.said);
        if replaced {
            out.unwrap();
        } else {
            let e = out.unwrap_err().to_string();
            assert!(e.starts_with("stopped at \"Sign in"), "{e}");
            assert!(e.contains("nothing was written"), "{e}");
            assert!(w.sign_ins.borrow().is_empty());
            let kept = w.keyring().get_oauth("gmail").unwrap().unwrap();
            assert_eq!(kept.refresh_token.as_deref(), Some("1//old"));
        }
        assert!(!term.raw);
    }
}

#[test]
fn outlook_from_flags_checks_the_app_id_then_the_mailbox_then_writes() {
    let w = World::new(&[MICROSOFT_KNOWN]);
    let mut term = Script::off_terminal(&[]);
    let out = w
        .run(&MICROSOFT, Flow::Browser, outlook_args(), &mut term, |e| {
            Ok(signed(e))
        })
        .unwrap();
    assert!(
        out.starts_with("Added \"outlook\" (you@outlook.example) to "),
        "{out}"
    );
    let seen = w.http.seen();
    assert_eq!(seen.len(), 1, "{seen:?}");
    let (url, form) = &seen[0];
    assert_eq!(
        url,
        "https://login.microsoftonline.com/common/oauth2/v2.0/devicecode"
    );
    assert_eq!(form["client_id"], OUTLOOK_ID);
    assert!(
        !form.contains_key("client_secret"),
        "a public client has no secret"
    );
    assert_eq!(term.secrets_asked, 0);
    assert_eq!(*w.graph_checks.borrow(), 1);
    assert_eq!(w.sign_ins.borrow()[0].2, Flow::Browser);
    assert!(w.keyring().get_oauth("outlook").unwrap().is_some());
}

#[test]
fn the_outlook_client_id_must_be_a_guid() {
    for id in [
        "abc",
        "0f1e2d3c-4b5a-6978-8796",
        "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1fz",
    ] {
        let w = World::new(&[]);
        let args = AddArgs {
            client_id: Some(id.into()),
            ..outlook_args()
        };
        let e = w
            .run(
                &MICROSOFT,
                Flow::Browser,
                args,
                &mut Script::off_terminal(&[]),
                |e| Ok(signed(e)),
            )
            .unwrap_err();
        assert!(e.is::<crate::UsageError>(), "{id}: {e}");
        assert!(
            e.to_string()
                .starts_with("--client-id: That is not an Application (client) ID"),
            "{e}"
        );
        assert!(w.http.seen().is_empty());
    }
    assert_eq!(
        steps::clean_guid(" 0F1E2D3C-4B5A-6978-8796-A5B4C3D2E1F0 ")
            .unwrap()
            .value,
        "0F1E2D3C-4B5A-6978-8796-A5B4C3D2E1F0"
    );
}

#[test]
fn microsoft_answers_to_the_app_check_name_the_step_to_change() {
    // From the live check of 2026-10-13: an unknown app on /common is AADSTS50059.
    let cases = [
        (
            r#"{"error":"invalid_request","error_codes":[50059],"error_description":"AADSTS50059: No tenant-identifying information found."}"#,
            "step 9 of 19 (Paste the Application (client) ID): Microsoft answered:",
        ),
        (
            r#"{"error":"unauthorized_client","error_codes":[700016],"error_description":"AADSTS700016: Application not found."}"#,
            "step 9 of 19 (Paste the Application (client) ID):",
        ),
        (
            r#"{"error":"invalid_request","error_codes":[50194],"error_description":"AADSTS50194: not multitenant."}"#,
            "step 7 of 19 (Register the app):",
        ),
    ];
    for (answer, want) in cases {
        let w = World::new(&[(400, answer)]);
        let e = w
            .run(
                &MICROSOFT,
                Flow::Browser,
                outlook_args(),
                &mut Script::off_terminal(&[]),
                |e| Ok(signed(e)),
            )
            .unwrap_err();
        assert!(e.to_string().starts_with(want), "{want}\n{e}");
        assert!(w.sign_ins.borrow().is_empty());
        w.assert_nothing_written("outlook");
    }
}

/// The titles shown on a terminal up to the first check, answering each question from
/// `answers` (by step id) and each console step with Enter; and each shown step's "next", as
/// its screen shows it with the answer about to be given.
fn outlook_steps_shown(answers: &[(&str, &str)]) -> Vec<(&'static str, Option<&'static str>)> {
    let mut session = Session::new(&steps::OUTLOOK, true, |_| None).unwrap();
    let mut shown = Vec::new();
    let mut prompt = session.prompt();
    loop {
        let event = match prompt {
            Prompt::Show(view) => match view.step.kind {
                Kind::Run(_) => {
                    shown.push((view.step.title, view.next));
                    Event::Answer(OUTLOOK_ID.to_owned())
                }
                Kind::Ask(_) | Kind::AskIf(..) => {
                    let answer = answers.iter().find(|(id, _)| *id == view.step.id);
                    let answer = answer.map_or("x@example.com", |(_, a)| a);
                    shown.push((view.step.title, session.preview(answer).next));
                    Event::Answer(answer.to_owned())
                }
                _ => {
                    shown.push((view.step.title, view.next));
                    Event::Continue
                }
            },
            _ => return shown,
        };
        prompt = session.next(event);
    }
}

#[test]
fn each_account_type_and_sign_in_method_sees_only_its_steps() {
    const DIRECTORY: &str = "Get a directory: a free Azure account";
    const REGISTER: &str = "Register the app";
    const PASTE: &str = "Paste the Application (client) ID";
    const REDIRECT: &str = "Add the redirect URI";
    const PUBLIC: &str = "Allow public client flows";
    const CONSENT: &str = "Your organisation may need to approve the app";
    const NAME_IT: &str = "Your directory";
    const WHICH: &str = "Which app";
    const RUN: &str = "Run the Azure CLI commands";
    // (account type, route, sign-in method, shown, not shown)
    type Case<'a> = (&'a str, &'a str, &'a str, &'a [&'a str], &'a [&'a str]);
    let cases: [Case<'_>; 4] = [
        (
            "personal",
            "portal",
            "browser",
            &[DIRECTORY, REGISTER, PASTE, REDIRECT],
            &[CONSENT, PUBLIC, NAME_IT, WHICH, RUN],
        ),
        (
            "work",
            "portal",
            "device-code",
            &[CONSENT, REGISTER, PASTE, PUBLIC],
            &[DIRECTORY, REDIRECT, NAME_IT, WHICH, RUN],
        ),
        (
            "personal",
            "commands",
            "browser",
            &[DIRECTORY, NAME_IT, WHICH, RUN],
            &[REGISTER, PASTE, REDIRECT, PUBLIC, CONSENT],
        ),
        (
            "work",
            "commands",
            "device-code",
            &[NAME_IT, WHICH, RUN, CONSENT],
            &[DIRECTORY, REGISTER, PASTE, REDIRECT, PUBLIC],
        ),
    ];
    for (kind, route, method, shown, hidden) in cases {
        let steps = outlook_steps_shown(&[
            ("account-type", kind),
            ("route", route),
            ("sign-in-method", method),
            ("directory", "contoso.onmicrosoft.com"),
            ("app", ""),
            ("client-id", OUTLOOK_ID),
        ]);
        let titles: Vec<&str> = steps.iter().map(|(t, _)| *t).collect();
        for title in shown {
            assert!(
                titles.contains(title),
                "{kind} {route} {method}: {title} in {titles:?}"
            );
        }
        for title in hidden {
            assert!(
                !titles.contains(title),
                "{kind} {route} {method}: {title} in {titles:?}"
            );
            // Nor is it announced as the next step.
            assert!(
                steps.iter().all(|(_, next)| next != &Some(*title)),
                "{title}: {steps:?}"
            );
        }
    }
}

#[test]
fn outlook_sign_in_errors_name_their_step() {
    let confidential = (
        400,
        r#"{"error":"invalid_client","error_codes":[7000218],"error_description":"AADSTS7000218: The request body must contain client_assertion or client_secret."}"#,
    );
    let declined = || -> anyhow::Error {
        SignInError::Declined {
            message: "access was declined in the browser; nothing was saved",
            description: Some("AADSTS65004: User declined to consent.".into()),
        }
        .into()
    };
    // (account type, flow, the error, the step it names)
    let cases: Vec<(&str, Flow, anyhow::Error, &str)> = vec![
        (
            "personal",
            Flow::DeviceCode,
            exchange_error(&MICROSOFT, confidential),
            "step 11 of 19 (Allow public client flows)",
        ),
        (
            "personal",
            Flow::Browser,
            exchange_error(&MICROSOFT, confidential),
            "step 10 of 19 (Add the redirect URI)",
        ),
        (
            "personal",
            Flow::Browser,
            exchange_error(
                &MICROSOFT,
                (
                    400,
                    r#"{"error":"invalid_request","error_codes":[9002332]}"#,
                ),
            ),
            "step 7 of 19 (Register the app)",
        ),
        (
            "work",
            Flow::Browser,
            declined(),
            "step 12 of 19 (Your organisation may need to approve the app)",
        ),
        // A personal account has no admin to ask: the run ends at the sign-in.
        (
            "personal",
            Flow::Browser,
            declined(),
            "step 17 of 19 (Sign in): access was declined",
        ),
    ];
    for (kind, flow, error, want) in cases {
        let w = World::new(&[MICROSOFT_KNOWN]);
        let args = AddArgs {
            account_type: Some(kind.into()),
            ..outlook_args()
        };
        let error = RefCell::new(Some(error));
        let e = w
            .run(
                &MICROSOFT,
                flow,
                args,
                &mut Script::off_terminal(&[]),
                |_| Err(error.borrow_mut().take().unwrap()),
            )
            .unwrap_err();
        assert!(e.to_string().starts_with(want), "{want}\n{e}");
        w.assert_nothing_written("outlook");
    }
}

#[test]
fn an_outlook_sign_in_that_times_out_names_the_steps_microsoft_shows_mistakes_for() {
    let w = World::new(&[MICROSOFT_KNOWN]);
    let e = w
        .run(
            &MICROSOFT,
            Flow::Browser,
            outlook_args(),
            &mut Script::off_terminal(&[]),
            |_| Err(SignInError::TimedOut("no sign-in came back within 5 minutes".into()).into()),
        )
        .unwrap_err()
        .to_string();
    assert!(
        e.starts_with("step 7 of 19 (Register the app): no sign-in came back"),
        "{e}"
    );
    assert!(
        e.contains("(the step \"Add the redirect URI\", AADSTS50011)"),
        "{e}"
    );
    assert!(
        e.contains("(the step \"Register the app\", AADSTS50194)"),
        "{e}"
    );
}

#[test]
fn device_code_is_the_sign_in_method_without_asking_when_chosen_or_over_ssh() {
    for flow in [Flow::DeviceCode, Flow::AutoDeviceCode] {
        let w = World::new(&[MICROSOFT_KNOWN]);
        // Account type, the console steps, the route, the client ID after Register, address
        // and name: no question about the method.
        let lines = [
            "personal",
            "",
            "portal",
            "",
            OUTLOOK_ID,
            "",
            "you@outlook.example",
            "",
        ];
        let mut term = Script::terminal(&lines, &[]);
        w.run(&MICROSOFT, flow, AddArgs::default(), &mut term, |e| {
            Ok(signed(e))
        })
        .unwrap();
        assert!(!term.said.contains("How you will sign in"), "{}", term.said);
        assert!(
            term.said.contains("Allow public client flows"),
            "{}",
            term.said
        );
        assert_eq!(w.sign_ins.borrow()[0].2, flow);
    }
}

#[test]
fn a_project_id_follows_googles_rules() {
    for ok in [
        "",
        "my-project-42",
        "abcdef",
        "a23456789012345678901234567890",
    ] {
        assert!(steps::clean_project(ok).is_ok(), "{ok:?}");
    }
    for bad in [
        "abcde",
        "My-project",
        "1project",
        "project-",
        "my_project",
        "a234567890123456789012345678901",
    ] {
        assert!(steps::clean_project(bad).is_err(), "{bad:?}");
    }
}
