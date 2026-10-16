//! The Proton Mail wizard against scripted input, a mock keyring, and fake Bridge IMAP and SMTP
//! servers on 127.0.0.1 (random ports, never 1143 or 1025) with STARTTLS or TLS from the first
//! byte and self-signed certificates.

use std::cell::Cell;
use std::io::{BufRead, BufReader, Read};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use keyring_core::mock;
use sha2::{Digest, Sha256};

use super::*;
use crate::config::Config;
use crate::wizard::script::{self, Script};

/// A self-signed certificate for 127.0.0.1, as Bridge makes one.
struct Cert {
    der: rustls::pki_types::CertificateDer<'static>,
    key: Vec<u8>,
    pem: String,
}

impl Cert {
    fn new() -> Self {
        let rcgen::CertifiedKey { cert, signing_key } =
            rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_owned()]).unwrap();
        Self {
            der: cert.der().clone(),
            key: signing_key.serialize_der(),
            pem: pem(cert.der()),
        }
    }

    /// Its SHA-256 as `openssl x509 -noout -fingerprint -sha256` prints it.
    fn fingerprint(&self) -> String {
        Sha256::digest(self.der.as_ref())
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(":")
    }

    fn pin(&self) -> CertPin {
        self.fingerprint().parse().unwrap()
    }
}

/// `der` as a PEM certificate (RFC 7468): base64 in lines of 64 characters.
fn pem(der: &[u8]) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(der);
    let lines: Vec<&str> = b64
        .as_bytes()
        .chunks(64)
        .map(|l| std::str::from_utf8(l).unwrap())
        .collect();
    format!(
        "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
        lines.join("\n")
    )
}

/// A fake Bridge port: IMAP or SMTP, secured with `security`, taking the password `accept`. What
/// it was sent is in `seen`: each command's verb, and `LOGIN <password>` or `AUTH <password>`.
struct Fake {
    port: u16,
    seen: Arc<Mutex<Vec<String>>>,
}

impl Fake {
    fn start(protocol: Protocol, security: Security, cert: &Cert, accept: &'static str) -> Self {
        let key = rustls::pki_types::PrivateKeyDer::Pkcs8(cert.key.clone().into());
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = Arc::new(
            rustls::ServerConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(vec![cert.der.clone()], key)
                .unwrap(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        std::thread::spawn(move || {
            for tcp in listener.incoming() {
                let Ok(tcp) = tcp else { return };
                if security == Security::StartTls && !plain(protocol, &tcp, &log) {
                    continue;
                }
                let tls = rustls::ServerConnection::new(Arc::clone(&config)).unwrap();
                let mut s = BufReader::new(rustls::StreamOwned::new(tls, tcp));
                let greet = security == Security::Tls;
                match protocol {
                    Protocol::Imap => imap(&mut s, &log, accept, greet),
                    Protocol::Smtp => smtp(&mut s, &log, accept, greet),
                }
            }
        });
        Self { port, seen }
    }

    fn seen(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

fn reply<S: Read + std::io::Write>(s: &mut BufReader<S>, text: &str) -> bool {
    s.get_mut().write_all(text.as_bytes()).is_ok()
}

fn read_line<S: Read>(s: &mut BufReader<S>) -> Option<String> {
    let mut line = String::new();
    (s.read_line(&mut line).unwrap_or(0) > 0).then(|| line.trim_end().to_owned())
}

/// The plain-text part of a STARTTLS connection: the greeting, then STARTTLS. False when the
/// client left before it.
fn plain(protocol: Protocol, tcp: &TcpStream, log: &Mutex<Vec<String>>) -> bool {
    let mut s = BufReader::new(tcp.try_clone().unwrap());
    let greeting = match protocol {
        Protocol::Imap => "* OK fake Bridge IMAP\r\n",
        Protocol::Smtp => "220 fake Bridge ESMTP\r\n",
    };
    if !reply(&mut s, greeting) {
        return false;
    }
    while let Some(line) = read_line(&mut s) {
        let words: Vec<&str> = line.split(' ').collect();
        let answer = match (protocol, words.as_slice()) {
            (Protocol::Imap, [tag, verb, ..]) if verb.eq_ignore_ascii_case("STARTTLS") => {
                format!("{tag} OK begin TLS\r\n")
            }
            (Protocol::Imap, [tag, ..]) => format!("{tag} OK\r\n"),
            (Protocol::Smtp, [verb, ..]) if verb.eq_ignore_ascii_case("EHLO") => {
                "250-fake\r\n250 STARTTLS\r\n".to_owned()
            }
            (Protocol::Smtp, [verb]) if verb.eq_ignore_ascii_case("STARTTLS") => {
                "220 go ahead\r\n".to_owned()
            }
            _ => "250 ok\r\n".to_owned(),
        };
        if !reply(&mut s, &answer) {
            return false;
        }
        if answer.contains("begin TLS") || answer.starts_with("220") {
            log.lock().unwrap().push("STARTTLS".to_owned());
            return true;
        }
    }
    false
}

fn imap<S: Read + std::io::Write>(
    s: &mut BufReader<S>,
    log: &Mutex<Vec<String>>,
    accept: &str,
    greet: bool,
) {
    if greet && !reply(s, "* OK fake Bridge IMAP\r\n") {
        return;
    }
    while let Some(line) = read_line(s) {
        let (tag, rest) = line.split_once(' ').unwrap_or((&line, ""));
        let verb = rest
            .split(' ')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        let answer = match verb.as_str() {
            "LOGIN" => {
                let password = rest
                    .rsplit(' ')
                    .next()
                    .unwrap_or_default()
                    .trim_matches('"');
                log.lock().unwrap().push(format!("LOGIN {password}"));
                if password == accept {
                    format!("{tag} OK logged in\r\n")
                } else {
                    format!("{tag} NO [AUTHENTICATIONFAILED] Invalid credentials\r\n")
                }
            }
            "EXAMINE" => {
                format!("* 3 EXISTS\r\n* OK [UIDVALIDITY 7] ok\r\n{tag} OK [READ-ONLY] done\r\n")
            }
            "LOGOUT" => format!("* BYE\r\n{tag} OK bye\r\n"),
            _ => format!("{tag} OK\r\n"),
        };
        if verb != "LOGIN" {
            log.lock().unwrap().push(verb.clone());
        }
        if !reply(s, &answer) || verb == "LOGOUT" {
            return;
        }
    }
}

fn smtp<S: Read + std::io::Write>(
    s: &mut BufReader<S>,
    log: &Mutex<Vec<String>>,
    accept: &str,
    greet: bool,
) {
    if greet && !reply(s, "220 fake Bridge ESMTP\r\n") {
        return;
    }
    while let Some(line) = read_line(s) {
        let mut words = line.split(' ');
        let verb = words.next().unwrap_or_default().to_ascii_uppercase();
        let answer = match verb.as_str() {
            "EHLO" => "250-fake\r\n250 AUTH PLAIN\r\n",
            "AUTH" => {
                // AUTH PLAIN base64("\0user\0password") (RFC 4616).
                let plain = base64::engine::general_purpose::STANDARD
                    .decode(words.nth(1).unwrap_or_default())
                    .unwrap_or_default();
                let password = String::from_utf8_lossy(&plain)
                    .rsplit('\0')
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                log.lock().unwrap().push(format!("AUTH {password}"));
                if password == accept {
                    "235 ok\r\n"
                } else {
                    "535 5.7.8 authentication failed\r\n"
                }
            }
            "QUIT" => "221 bye\r\n",
            _ => "250 ok\r\n",
        };
        if verb != "AUTH" {
            log.lock().unwrap().push(verb.clone());
        }
        if !reply(s, answer) || verb == "QUIT" {
            return;
        }
    }
}

const PASSWORD: &str = "fake-bridge-password-1";
const USERNAME: &str = "you@example.com";

/// A config directory, a mock keyring, and a fake Bridge: IMAP and SMTP with one certificate,
/// unless the SMTP port is given its own.
struct World {
    dir: tempfile::TempDir,
    store: Arc<mock::Store>,
    keyring_opens: Cell<usize>,
    bridge_found: bool,
    cert: Cert,
    smtp_cert: Option<Cert>,
    imap: Fake,
    smtp: Fake,
}

impl World {
    fn new() -> Self {
        Self::with(Security::StartTls, Security::StartTls, false)
    }

    /// `own_smtp_cert`: the SMTP port presents another certificate than the IMAP one.
    fn with(imap: Security, smtp: Security, own_smtp_cert: bool) -> Self {
        let cert = Cert::new();
        let smtp_cert = own_smtp_cert.then(Cert::new);
        Self {
            dir: tempfile::tempdir().unwrap(),
            store: mock::Store::new().unwrap(),
            keyring_opens: Cell::new(0),
            bridge_found: true,
            imap: Fake::start(Protocol::Imap, imap, &cert, PASSWORD),
            smtp: Fake::start(
                Protocol::Smtp,
                smtp,
                smtp_cert.as_ref().unwrap_or(&cert),
                PASSWORD,
            ),
            cert,
            smtp_cert,
        }
    }

    fn config(&self) -> PathBuf {
        self.dir.path().join("config.toml")
    }

    /// What Bridge's `cert export` writes: cert.pem, and key.pem next to it.
    fn export(&self, cert: &Cert) -> PathBuf {
        let dir = self.dir.path().join("export");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("key.pem"), "-----BEGIN PRIVATE KEY-----\n").unwrap();
        std::fs::write(dir.join("cert.pem"), &cert.pem).unwrap();
        dir.join("cert.pem")
    }

    /// Every flag but the pin, with the fake's ports.
    fn args(&self) -> Args {
        Args {
            username: Some(USERNAME.into()),
            port: Some(self.imap.port),
            smtp_port: Some(self.smtp.port),
            ..Args::default()
        }
    }

    /// Runs the wizard; Ok is what it printed on standard output.
    fn run(&self, args: Args, term: &mut Script) -> Result<String> {
        let open = || {
            self.keyring_opens.set(self.keyring_opens.get() + 1);
            Ok(Keyring::with_store(self.store.clone(), "the mock", false))
        };
        let find = || {
            self.bridge_found
                .then(|| PathBuf::from("/usr/bin/protonmail-bridge"))
        };
        let deps = Deps {
            open_keyring: &open,
            find_bridge: &find,
        };
        let mut out = Vec::new();
        run(
            &self.config(),
            args,
            OFF_TERMINAL,
            &mut script::ui(term),
            &deps,
            &mut out,
        )?;
        Ok(String::from_utf8(out).unwrap())
    }

    /// The error of a run that must fail, after checking that it wrote and stored nothing.
    fn fails(&self, args: Args, term: &mut Script) -> String {
        let e = format!("{:#}", self.run(args, term).unwrap_err());
        self.assert_nothing_written();
        e
    }

    fn assert_nothing_written(&self) {
        assert!(!self.config().exists(), "the config was written");
        let keyring = Keyring::with_store(self.store.clone(), "the mock", false);
        assert!(keyring.get("proton").is_err(), "a password was stored");
    }

    fn account(&self) -> crate::config::Account {
        Config::load(&self.config())
            .unwrap()
            .find("proton")
            .unwrap()
            .clone()
    }

    fn stored(&self) -> String {
        let keyring = Keyring::with_store(self.store.clone(), "the mock", false);
        keyring.get("proton").unwrap().expose().to_owned()
    }

    fn logins(&self) -> Vec<String> {
        let mut seen = self.imap.seen();
        seen.extend(self.smtp.seen());
        seen.retain(|s| s.starts_with("LOGIN") || s.starts_with("AUTH"));
        seen
    }
}

const OFF_TERMINAL: Input = Input {
    terminal: false,
    no_input: false,
};

/// A port on 127.0.0.1 where nothing listens.
fn closed_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

#[test]
fn from_flags_both_answered_ports_are_checked_and_written_with_their_pins() {
    let w = World::new();
    let cert = w.export(&w.cert);
    let args = Args {
        bridge_cert: Some(cert.clone()),
        ..w.args()
    };
    let mut term = Script::off_terminal(&[PASSWORD]);
    let out = w.run(args, &mut term).unwrap();
    assert!(out.contains("Added \"proton\" (you@example.com)"), "{out}");
    assert!(term.said.contains(&w.cert.fingerprint()), "{}", term.said);
    let key = cert.with_file_name("key.pem");
    assert!(
        term.said.contains(&format!("Delete {}", key.display())),
        "{}",
        term.said
    );
    let account = w.account();
    let imap = account.imap().unwrap();
    assert_eq!(
        (imap.host.as_str(), imap.port, imap.security, imap.cert_pin),
        (
            "127.0.0.1",
            w.imap.port,
            Security::StartTls,
            Some(w.cert.pin())
        )
    );
    assert_eq!(
        account.smtp,
        Some(SmtpSettings {
            host: "127.0.0.1".into(),
            port: w.smtp.port,
            security: Security::StartTls,
            cert_pin: Some(w.cert.pin()),
            save_sent: false,
        })
    );
    assert_eq!(w.stored(), PASSWORD);
    assert_eq!(
        w.logins(),
        [format!("LOGIN {PASSWORD}"), format!("AUTH {PASSWORD}")]
    );
    assert!(w.imap.seen().contains(&"EXAMINE".to_owned()));
}

#[test]
fn on_a_terminal_each_step_is_shown_and_the_ports_typed_are_checked() {
    let w = World::with(Security::Tls, Security::Tls, false);
    let (imap, smtp) = (w.imap.port.to_string(), w.smtp.port.to_string());
    let lines: Vec<&'static str> = vec![
        "", // a paid plan
        "", // install
        "", // sign in
        "", // export the certificate
        "", // name: proton
        USERNAME,
        imap.leak(),
        "ssl", // Bridge's word for TLS from the first byte
        smtp.leak(),
        "tls",
        "y",
    ];
    let mut term = Script::terminal(&lines, &[PASSWORD]);
    w.run(Args::default(), &mut term).unwrap();
    for shown in [
        "Step 1/12 · A paid Proton plan",
        "https://proton.me/mail/bridge",
        "Step 4/12 · Export Bridge's certificate",
        "Step 6/12 · Mailbox details: the username",
        "never into neomail",
        &w.cert.fingerprint(),
    ] {
        assert!(term.said.contains(shown), "{shown:?} in {}", term.said);
    }
    let account = w.account();
    assert_eq!(account.imap().unwrap().port, w.imap.port);
    assert_eq!(account.imap().unwrap().security, Security::Tls);
    assert_eq!(account.smtp.unwrap().security, Security::Tls);
    assert_eq!(w.stored(), PASSWORD);
}

#[test]
fn a_port_where_nothing_listens_names_its_step_and_starting_bridge() {
    for smtp_closed in [false, true] {
        for found in [true, false] {
            let mut w = World::new();
            w.bridge_found = found;
            let mut args = Args {
                cert_sha256: Some(w.cert.pin()),
                ..w.args()
            };
            let step = if smtp_closed {
                args.smtp_port = Some(closed_port());
                "step 10 of 18 (Mailbox details: the SMTP port)"
            } else {
                args.port = Some(closed_port());
                "step 8 of 18 (Mailbox details: the IMAP port)"
            };
            let mut term = Script::off_terminal(&[PASSWORD]);
            let e = w.fails(args, &mut term);
            assert!(e.starts_with(step), "{e}");
            assert!(
                e.contains("connection was refused") && e.contains("Start Bridge"),
                "{e}"
            );
            assert_eq!(e.contains("finds neither protonmail-bridge"), !found, "{e}");
            assert_eq!(term.secrets_asked, 0, "no password before Bridge answers");
            assert_eq!(w.keyring_opens.get(), 0);
        }
    }
}

#[test]
fn on_a_terminal_a_refused_port_is_asked_again() {
    let w = World::new();
    let closed = closed_port().to_string();
    let open = w.imap.port.to_string();
    let args = Args {
        username: Some(USERNAME.into()),
        smtp_port: Some(w.smtp.port),
        ..Args::default()
    };
    let lines = ["", "", "", "", "", closed.leak(), "", "", open.leak(), "y"];
    let mut term = Script::terminal(&lines, &[PASSWORD]);
    w.run(args, &mut term).unwrap();
    assert!(term.said.contains("Start Bridge"), "{}", term.said);
    assert_eq!(w.account().imap().unwrap().port, w.imap.port);
}

#[test]
fn a_security_answer_bridge_does_not_speak_names_the_security_step() {
    use Security::{StartTls, Tls};
    // (IMAP serves, SMTP serves, --security, --smtp-security, the step, what it says)
    for (imap, smtp, imap_answer, smtp_answer, step, says) in [
        (
            Tls,
            StartTls,
            None,
            None,
            "step 9 of 18 (Mailbox details: the IMAP security)",
            "speaks TLS from the first byte",
        ),
        (
            StartTls,
            StartTls,
            Some(Tls),
            None,
            "step 9 of 18 (Mailbox details: the IMAP security)",
            "greets in plain text",
        ),
        (
            StartTls,
            Tls,
            None,
            None,
            "step 11 of 18 (Mailbox details: the SMTP security)",
            "speaks TLS from the first byte",
        ),
        (
            StartTls,
            StartTls,
            None,
            Some(Tls),
            "step 11 of 18 (Mailbox details: the SMTP security)",
            "greets in plain text",
        ),
    ] {
        let w = World::with(imap, smtp, false);
        let args = Args {
            security: imap_answer,
            smtp_security: smtp_answer,
            cert_sha256: Some(w.cert.pin()),
            ..w.args()
        };
        let e = w.fails(args, &mut Script::off_terminal(&[PASSWORD]));
        assert!(e.starts_with(step) && e.contains(says), "{e}");
        assert!(e.contains("Connection mode"), "{e}");
        assert!(w.logins().is_empty());
    }
}

#[test]
fn a_certificate_that_is_not_the_one_bridge_presents_is_refused_before_the_password() {
    let w = World::new();
    let other = Cert::new();
    let wrong_file = Args {
        bridge_cert: Some(w.export(&other)),
        ..w.args()
    };
    let wrong_pin = Args {
        cert_sha256: Some(other.pin()),
        ..w.args()
    };
    for args in [wrong_file, wrong_pin] {
        let mut term = Script::off_terminal(&[PASSWORD]);
        let e = w.fails(args, &mut term);
        assert!(
            e.starts_with("step 13 of 18 (Pin Bridge's certificate)"),
            "{e}"
        );
        assert!(
            e.contains(&other.fingerprint()) && e.contains(&w.cert.fingerprint()),
            "{e}"
        );
        assert_eq!(term.secrets_asked, 0);
        assert!(w.logins().is_empty());
    }
    // A file that holds no certificate, such as the key.pem next to it.
    let key = Args {
        bridge_cert: Some(w.export(&w.cert).with_file_name("key.pem")),
        ..w.args()
    };
    let e = w.fails(key, &mut Script::off_terminal(&[PASSWORD]));
    assert!(e.contains("holds no PEM certificate"), "{e}");
}

#[test]
fn on_a_terminal_the_cert_pem_is_compared_and_a_no_writes_nothing() {
    let w = World::new();
    let other = w.export(&Cert::new()).to_string_lossy().into_owned();
    let dir = tempfile::tempdir().unwrap();
    let good = dir.path().join("cert.pem");
    std::fs::write(&good, &w.cert.pem).unwrap();
    let good = good.to_string_lossy().into_owned();
    // Steps 1 to 4, then the certificate: a wrong cert.pem is asked again.
    let lines = ["", "", "", "", other.leak(), good.leak()];
    let mut term = Script::terminal(&lines, &[PASSWORD]);
    w.run(all_flags_for(&w), &mut term).unwrap();
    assert!(
        term.said.contains("is not the certificate"),
        "{}",
        term.said
    );
    assert!(
        term.said.contains("If the export wrote key.pem"),
        "{}",
        term.said
    );
    assert_eq!(w.account().imap().unwrap().cert_pin, Some(w.cert.pin()));

    let w = World::new();
    let mut term = Script::terminal(&["", "", "", "", "n"], &[]);
    let e = w.fails(all_flags_for(&w), &mut term);
    assert!(e.contains("not pinned"), "{e}");
    assert!(w.logins().is_empty());
}

fn all_flags_for(w: &World) -> Args {
    Args {
        name: Some("proton".into()),
        security: Some(Security::StartTls),
        smtp_security: Some(Security::StartTls),
        ..w.args()
    }
}

#[test]
fn a_different_smtp_certificate_is_shown_and_pinned_on_its_own() {
    let w = World::with(Security::StartTls, Security::StartTls, true);
    let smtp_cert = w.smtp_cert.as_ref().unwrap();
    let args = Args {
        cert_sha256: Some(w.cert.pin()),
        ..w.args()
    };
    let mut term = Script::off_terminal(&[PASSWORD]);
    w.run(args, &mut term).unwrap();
    assert!(
        term.said.contains(&smtp_cert.fingerprint()),
        "{}",
        term.said
    );
    let account = w.account();
    assert_eq!(account.imap().unwrap().cert_pin, Some(w.cert.pin()));
    assert_eq!(account.smtp.unwrap().cert_pin, Some(smtp_cert.pin()));
}

#[test]
fn the_bridge_password_is_checked_on_both_ports_before_anything_is_written() {
    // IMAP refuses it, or IMAP takes it and SMTP refuses it.
    for smtp_only in [false, true] {
        let w = World::new();
        let wrong = Fake::start(Protocol::Smtp, Security::StartTls, &w.cert, "another");
        let args = Args {
            cert_sha256: Some(w.cert.pin()),
            smtp_port: smtp_only.then_some(wrong.port).or(Some(w.smtp.port)),
            ..w.args()
        };
        let password = if smtp_only {
            PASSWORD
        } else {
            "my-proton-password"
        };
        let e = w.fails(args, &mut Script::off_terminal(&[password]));
        assert!(
            e.starts_with("step 16 of 18 (Paste the Bridge password)"),
            "{e}"
        );
        assert!(e.contains("not\nyour Proton password"), "{e}");
        // It names the steps it points back to by what they are: their numbers differ between
        // this message and a terminal's header.
        assert!(
            e.contains("check the username (on a terminal, Esc goes back to it)"),
            "{e}"
        );
        assert!(e.contains("sign in to Bridge first."), "{e}");
        assert!(!e.contains("(step "), "{e}");
    }
    // On a terminal the password is asked again.
    let w = World::new();
    let mut term = Script::terminal(&["", "", "", "", "y"], &["my-proton-password", PASSWORD]);
    w.run(all_flags_for(&w), &mut term).unwrap();
    assert_eq!(w.stored(), PASSWORD);
    assert_eq!(term.secrets_asked, 2);
}

#[test]
fn off_a_terminal_the_pin_comes_from_a_flag_and_the_password_never_does() {
    let w = World::new();
    for input in [
        OFF_TERMINAL,
        Input {
            terminal: true,
            no_input: true,
        },
    ] {
        let keyring = || -> Result<Keyring> { unreachable!("nothing is asked") };
        let deps = Deps {
            open_keyring: &keyring,
            find_bridge: &|| None,
        };
        let mut tty = crate::wizard::term::Tty::new(input, "password");
        let mut ui = cli::Ui::real(&mut tty, crate::style::Paint::OFF, true, "proton");
        let e = run(
            &w.config(),
            w.args(),
            input,
            &mut ui,
            &deps,
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(e.is::<crate::UsageError>(), "{e:#}");
        assert!(
            format!("{e:#}").contains("pass --cert-sha256 or --bridge-cert ("),
            "{e:#}"
        );
    }
    assert!(w.imap.seen().is_empty() && w.smtp.seen().is_empty());
    let password = PROTON.steps.iter().find(|s| s.id == "password").unwrap();
    assert!(matches!(&password.kind, Kind::Ask(q) if q.secret && q.flag.is_none()));
}

#[test]
fn no_check_pins_the_given_certificate_without_connecting() {
    let w = World::new();
    let args = Args {
        port: Some(closed_port()),
        smtp_port: Some(closed_port()),
        bridge_cert: Some(w.export(&w.cert)),
        password_command: Some("printf never-run".into()),
        no_check: true,
        ..w.args()
    };
    w.run(args, &mut Script::off_terminal(&[])).unwrap();
    let account = w.account();
    assert_eq!(account.imap().unwrap().cert_pin, Some(w.cert.pin()));
    assert_eq!(account.smtp.unwrap().cert_pin, Some(w.cert.pin()));
    assert_eq!(w.keyring_opens.get(), 0);

    let w = World::new();
    let e = w.run(
        Args {
            no_check: true,
            ..w.args()
        },
        &mut Script::off_terminal(&[]),
    );
    assert!(format!("{:#}", e.unwrap_err()).contains("pass --cert-sha256 or --bridge-cert too"));
    w.assert_nothing_written();
}

/// The certificate is exported before the mailbox details are opened, so no step from the
/// username to the Bridge password sends the user to another Bridge page while they are asked
/// for what the details show (#106).
#[test]
fn the_mailbox_details_stay_open_from_the_username_to_the_password() {
    let at = |id: &str| PROTON.position(id).unwrap();
    let exports =
        |s: &Step| s.why.contains("Export TLS certificates") || s.why.contains("cert export");
    let first_export = PROTON.steps.iter().position(exports).unwrap();
    assert!(
        first_export < at("username"),
        "export at step {}",
        first_export + 1
    );
    for step in &PROTON.steps[at("username")..=at("password")] {
        assert!(!exports(step), "{} sends the user away", step.id);
    }
}
