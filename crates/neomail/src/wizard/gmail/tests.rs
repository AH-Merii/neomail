//! The Gmail app-password wizard against scripted input, a mock keyring, and fake IMAP and SMTP
//! servers on 127.0.0.1 with TLS and a pinned self-signed certificate.

use std::cell::Cell;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use keyring_core::mock;
use nm_imap::CertPin;
use sha2::{Digest, Sha256};

use super::*;

/// Scripted input: `lines` for typed answers, `secrets` for hidden ones.
#[derive(Default)]
struct Script {
    interactive: bool,
    lines: VecDeque<&'static str>,
    secrets: VecDeque<&'static str>,
    secrets_asked: usize,
    said: String,
    /// Each prompt, in order.
    prompts: Vec<String>,
}

impl Term for Script {
    fn interactive(&self) -> bool {
        self.interactive
    }
    fn say(&mut self, text: &str) {
        self.said.push_str(text);
        self.said.push('\n');
    }
    fn line(&mut self, prompt: &str) -> Result<String> {
        self.prompts.push(prompt.to_owned());
        let line = self.lines.pop_front();
        Ok(line
            .unwrap_or_else(|| panic!("no answer for {prompt:?}:\n{}", self.said))
            .to_owned())
    }
    fn secret(&mut self, prompt: &str) -> Result<String> {
        self.secrets_asked += 1;
        self.prompts.push(prompt.to_owned());
        let secret = self.secrets.pop_front();
        Ok(secret
            .unwrap_or_else(|| panic!("no secret for {prompt:?}:\n{}", self.said))
            .to_owned())
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

/// IMAP that takes the password `accept` only; logs `LOGIN <password>` and the other verbs.
fn imap(accept: &'static str) -> Fake {
    Fake::start(move |s, log| {
        if !reply(s, "* OK fake IMAP\r\n") {
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
                "EXAMINE" => format!(
                    "* 3 EXISTS\r\n* OK [UIDVALIDITY 7] ok\r\n{tag} OK [READ-ONLY] done\r\n"
                ),
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
    })
}

/// SMTP that refuses every AUTH with 535 when `refuse`; logs each verb.
fn smtp(refuse: bool) -> Fake {
    Fake::start(move |s, log| {
        if !reply(s, "220 fake ESMTP\r\n") {
            return;
        }
        while let Some(line) = read_line(s) {
            let verb = line
                .split(' ')
                .next()
                .unwrap_or_default()
                .to_ascii_uppercase();
            log.lock().unwrap().push(verb.clone());
            let answer = match verb.as_str() {
                "EHLO" => "250-fake\r\n250 AUTH PLAIN\r\n",
                "AUTH" if refuse => "535 5.7.8 Username and Password not accepted\r\n",
                "AUTH" => "235 ok\r\n",
                "QUIT" => "221 bye\r\n",
                _ => "250 ok\r\n",
            };
            if !reply(s, answer) || verb == "QUIT" {
                return;
            }
        }
    })
}

const OFF_TERMINAL: Input = Input {
    terminal: false,
    no_input: false,
};
const APP_PASSWORD: &str = "abcdefghijklmnop";

/// A config directory, a mock keyring, and the two fake servers.
struct World {
    dir: tempfile::TempDir,
    store: Arc<mock::Store>,
    keyring_opens: Cell<usize>,
    keyring_fails: bool,
    imap: Fake,
    smtp: Fake,
}

impl World {
    fn new(imap: Fake, smtp: Fake) -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
            store: mock::Store::new().unwrap(),
            keyring_opens: Cell::new(0),
            keyring_fails: false,
            imap,
            smtp,
        }
    }

    fn config(&self) -> std::path::PathBuf {
        self.dir.path().join("config.toml")
    }

    fn keyring(&self) -> Keyring {
        Keyring::with_store(self.store.clone(), "the mock", false)
    }

    /// Runs the wizard against the fakes; Ok is what it printed on standard output.
    fn run(&self, args: Args, term: &mut Script) -> Result<String> {
        self.run_with(args, term, self.servers())
    }

    fn servers(&self) -> Servers {
        Servers {
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
        }
    }

    fn run_with(&self, args: Args, term: &mut Script, servers: Servers) -> Result<String> {
        let open = || {
            self.keyring_opens.set(self.keyring_opens.get() + 1);
            if self.keyring_fails {
                anyhow::bail!("no Secret Service");
            }
            Ok(self.keyring())
        };
        let deps = Deps {
            open_keyring: &open,
            servers,
        };
        let mut out = Vec::new();
        run(&self.config(), args, OFF_TERMINAL, term, &deps, &mut out)?;
        Ok(String::from_utf8(out).unwrap())
    }

    fn stored(&self, name: &str) -> Option<String> {
        self.keyring().get(name).ok().map(|p| p.expose().to_owned())
    }

    /// Nothing reached the config or the keyring.
    fn assert_nothing_written(&self) {
        assert!(!self.config().exists());
        assert_eq!(self.stored("gmail"), None);
    }
}

fn address() -> Args {
    Args {
        username: Some("you@example.com".into()),
        ..Args::default()
    }
}

fn piped(secret: &'static str) -> Script {
    Script {
        secrets: [secret].into(),
        ..Script::default()
    }
}

#[test]
fn a_pasted_app_password_loses_its_spaces_and_logs_in_to_imap_and_smtp_before_the_write() {
    for pasted in ["abcd efgh ijkl mnop", "abcd\u{a0}efgh\u{a0}ijkl\u{a0}mnop"] {
        let world = World::new(imap(APP_PASSWORD), smtp(false));
        let mut term = piped(pasted);
        let out = world.run(address(), &mut term).unwrap();
        assert!(out.contains("Next: neomail sync --account gmail"), "{out}");
        assert_eq!(
            world.imap.seen(),
            [
                format!("LOGIN {APP_PASSWORD}"),
                "EXAMINE".into(),
                "LOGOUT".into()
            ]
        );
        // AUTH, then QUIT: no MAIL, RCPT or DATA.
        assert_eq!(world.smtp.seen(), ["EHLO", "AUTH", "QUIT"]);
        assert_eq!(world.stored("gmail").as_deref(), Some(APP_PASSWORD));
        let config = Config::load(&world.config()).unwrap();
        let account = config.find("gmail").unwrap();
        assert_eq!(account.imap().unwrap().port, world.imap.port);
        assert_eq!(account.smtp.as_ref().unwrap().port, world.smtp.port);
        assert!(!term.said.contains("Warning"), "{}", term.said);
    }
}

#[test]
fn a_value_that_is_not_16_letters_warns_and_is_still_checked() {
    let world = World::new(imap("short123"), smtp(false));
    let mut term = piped("short123");
    world.run(address(), &mut term).unwrap();
    assert!(
        term.said
            .contains("Warning: that is not 16 letters or digits"),
        "{}",
        term.said
    );
    assert_eq!(world.imap.seen()[0], "LOGIN short123");
    assert_eq!(world.stored("gmail").as_deref(), Some("short123"));
}

#[test]
fn a_refused_imap_or_smtp_login_names_the_password_step_and_writes_nothing() {
    for (imap_accepts, smtp_refuses) in [("another-password", false), (APP_PASSWORD, true)] {
        let world = World::new(imap(imap_accepts), smtp(smtp_refuses));
        let e = world.run(address(), &mut piped(APP_PASSWORD)).unwrap_err();
        let e = format!("{e:#}");
        assert!(
            e.starts_with("step 8 of 10 (Paste the app password): "),
            "{e}"
        );
        for case in [
            "security keys",
            "work, school",
            "Advanced Protection",
            "revokes",
        ] {
            assert!(e.contains(case), "{case}: {e}");
        }
        assert!(e.ends_with("Nothing was written."), "{e}");
        assert!(!e.contains(APP_PASSWORD), "{e}");
        world.assert_nothing_written();
    }
}

/// The answers of a terminal run up to the password: the first case, Enter twice, the default
/// name and an address.
fn typed(extra: &[&'static str]) -> Script {
    let mut lines: VecDeque<_> = ["1", "", "", "", "you@example.com"].into();
    lines.extend(extra);
    Script {
        interactive: true,
        lines,
        ..Script::default()
    }
}

#[test]
fn on_a_terminal_a_refused_login_asks_for_the_app_password_again() {
    let world = World::new(imap(APP_PASSWORD), smtp(false));
    let mut term = Script {
        secrets: ["your-google-password", APP_PASSWORD].into(),
        ..typed(&[])
    };
    world.run(Args::default(), &mut term).unwrap();
    assert_eq!(term.secrets_asked, 2);
    assert_eq!(
        term.said.matches("Gmail refused this address").count(),
        1,
        "{}",
        term.said
    );
    assert_eq!(world.stored("gmail").as_deref(), Some(APP_PASSWORD));
}

#[test]
fn a_work_or_school_account_goes_on_and_learns_at_step_3_what_to_do_if_none_is_offered() {
    let world = World::new(imap(APP_PASSWORD), smtp(false));
    let mut lines = typed(&[]).lines;
    lines[0] = "3";
    let mut term = Script {
        interactive: true,
        lines,
        secrets: [APP_PASSWORD].into(),
        ..Script::default()
    };
    world.run(Args::default(), &mut term).unwrap();
    assert_eq!(world.stored("gmail").as_deref(), Some(APP_PASSWORD));
    // Step 2 names step 3 as next, so step 3 starts at the title's last mention.
    let (_, from_step_3) = term
        .said
        .rsplit_once("Create an app password")
        .expect("step 3 is shown");
    let step_3 = from_step_3.split("Next:").next().unwrap();
    for advice in [
        "If the page has no option to add one",
        "administrator",
        "neomail account add gmail`",
    ] {
        assert!(step_3.contains(advice), "{advice}: {}", term.said);
    }
}

#[test]
fn each_case_without_an_app_password_stops_with_its_own_advice_before_any_work() {
    for (case, advice, not) in [
        ("2", "Add another second step", "account add gmail`"),
        (
            "4",
            "neomail has no way to read this account",
            "account add gmail`",
        ),
    ] {
        let world = World::new(imap(APP_PASSWORD), smtp(false));
        let mut term = Script {
            interactive: true,
            lines: [case].into(),
            ..Script::default()
        };
        let e = format!("{:#}", world.run(Args::default(), &mut term).unwrap_err());
        assert!(e.contains(advice), "{case}: {e}");
        assert!(!e.contains(not), "{case}: {e}");
        assert!(
            e.contains("https://support.google.com/accounts/answer/"),
            "{e}"
        );
        assert_eq!((world.keyring_opens.get(), term.secrets_asked), (0, 0));
        assert!(world.imap.seen().is_empty());
        world.assert_nothing_written();
    }
}

#[test]
fn the_keyring_is_checked_before_the_app_password_is_asked() {
    let mut world = World::new(imap(APP_PASSWORD), smtp(false));
    world.keyring_fails = true;
    for mut term in [typed(&[]), piped(APP_PASSWORD)] {
        let args = if term.interactive {
            Args::default()
        } else {
            address()
        };
        let e = format!("{:#}", world.run(args, &mut term).unwrap_err());
        assert!(e.contains("no Secret Service"), "{e}");
        assert_eq!(term.secrets_asked, 0);
        world.assert_nothing_written();
    }
}

#[test]
fn a_taken_name_is_asked_again_on_a_terminal_and_fails_off_one_before_the_password() {
    let world = World::new(imap(APP_PASSWORD), smtp(false));
    let config = world.config();
    std::fs::write(
        &config,
        "[[accounts]]\nname = \"gmail\"\nusername = \"old@example.com\"\n[accounts.imap]\nhost = \"imap.example.com\"\n",
    )
    .unwrap();
    let mut term = piped(APP_PASSWORD);
    let e = format!("{:#}", world.run(address(), &mut term).unwrap_err());
    assert!(e.starts_with("step 4 of 10 (Name the account): "), "{e}");
    assert_eq!(term.secrets_asked, 0);
    assert!(world.imap.seen().is_empty());

    // The name is asked again right after it, before the address.
    let mut term = Script {
        interactive: true,
        lines: ["1", "", "", "", "work", "you@example.com"].into(),
        secrets: [APP_PASSWORD].into(),
        ..Script::default()
    };
    world.run(Args::default(), &mut term).unwrap();
    assert!(term.said.contains("Choose another name"), "{}", term.said);
    assert!(Config::load(&config).unwrap().find("work").is_some());
}

#[test]
fn a_missing_address_off_a_terminal_is_a_usage_error_that_names_the_flag() {
    let world = World::new(imap(APP_PASSWORD), smtp(false));
    let mut term = Script::default();
    let e = world.run(Args::default(), &mut term).unwrap_err();
    assert!(e.downcast_ref::<crate::UsageError>().is_some(), "{e:#}");
    assert!(format!("{e:#}").contains("pass --username"), "{e:#}");
    assert_eq!(term.secrets_asked, 0);
}

#[test]
fn a_16_character_value_of_letters_and_digits_gets_no_warning() {
    // Google calls it "a 16-digit passcode" (support.google.com/accounts/answer/185833).
    let world = World::new(imap("abcd1234efgh5678"), smtp(false));
    let mut term = piped("abcd 1234 efgh 5678");
    world.run(address(), &mut term).unwrap();
    assert!(!term.said.contains("Warning"), "{}", term.said);
    assert_eq!(world.stored("gmail").as_deref(), Some("abcd1234efgh5678"));
}

#[test]
fn a_password_command_is_run_for_the_login_and_written_without_the_keyring() {
    let mut world = World::new(imap(APP_PASSWORD), smtp(false));
    world.keyring_fails = true;
    let command = format!("printf {APP_PASSWORD}");
    let args = Args {
        password_command: Some(command.clone()),
        ..address()
    };
    // No secrets: reading one panics.
    let mut term = Script::default();
    world.run(args, &mut term).unwrap();
    assert_eq!((term.secrets_asked, world.keyring_opens.get()), (0, 0));
    assert_eq!(world.imap.seen()[0], format!("LOGIN {APP_PASSWORD}"));
    assert_eq!(world.smtp.seen(), ["EHLO", "AUTH", "QUIT"]);
    assert_eq!(world.stored("gmail"), None);
    let config = Config::load(&world.config()).unwrap();
    assert_eq!(
        config.find("gmail").unwrap().login,
        config::Login::Password(config::PasswordSource::Command(command))
    );
    let text = std::fs::read_to_string(world.config()).unwrap();
    assert!(!text.contains(&format!("\"{APP_PASSWORD}\"")), "{text}");
}

#[test]
fn a_refused_login_says_which_flags_to_change_when_the_values_came_from_flags() {
    // A password_command that prints the app password as Google shows it, in groups.
    let world = World::new(imap(APP_PASSWORD), smtp(false));
    let args = Args {
        password_command: Some("printf 'abcd efgh ijkl mnop'".into()),
        ..address()
    };
    let e = format!("{:#}", world.run(args, &mut Script::default()).unwrap_err());
    assert!(e.contains("came from --password-command"), "{e}");
    assert!(e.contains("without spaces"), "{e}");
    assert!(e.contains("came from --username"), "{e}");
    world.assert_nothing_written();
}

#[test]
fn no_check_skips_the_login_but_not_the_name_check() {
    let world = World::new(imap("another-password"), smtp(true));
    let args = || Args {
        no_check: true,
        ..address()
    };
    world.run(args(), &mut piped(APP_PASSWORD)).unwrap();
    assert!(world.imap.seen().is_empty());
    assert!(world.smtp.seen().is_empty());
    assert_eq!(world.stored("gmail").as_deref(), Some(APP_PASSWORD));

    let e = format!(
        "{:#}",
        world.run(args(), &mut piped(APP_PASSWORD)).unwrap_err()
    );
    assert!(e.starts_with("step 4 of 10 (Name the account): "), "{e}");
}

#[test]
fn name_names_the_account_and_its_keyring_entry() {
    let world = World::new(imap(APP_PASSWORD), smtp(false));
    let args = Args {
        name: Some("work".into()),
        ..address()
    };
    world.run(args, &mut piped(APP_PASSWORD)).unwrap();
    assert!(
        Config::load(&world.config())
            .unwrap()
            .find("work")
            .is_some()
    );
    assert_eq!(world.stored("work").as_deref(), Some(APP_PASSWORD));
}

#[test]
fn gmails_servers_are_written_as_google_lists_them() {
    // https://developers.google.com/workspace/gmail/imap/imap-smtp: imap.gmail.com 993 and
    // smtp.gmail.com 465, both SSL/TLS. No login is tried: --no-check, and a command for the
    // password so the keyring is not opened.
    let world = World::new(imap(APP_PASSWORD), smtp(false));
    let args = Args {
        password_command: Some("printf x".into()),
        no_check: true,
        ..address()
    };
    world
        .run_with(args, &mut Script::default(), Servers::gmail())
        .unwrap();
    let config = Config::load(&world.config()).unwrap();
    let account = config.find("gmail").unwrap();
    let imap = account.imap().unwrap();
    assert_eq!(
        (imap.host.as_str(), imap.port, imap.security),
        ("imap.gmail.com", 993, Security::Tls)
    );
    let smtp = account.smtp.as_ref().unwrap();
    assert_eq!(
        (smtp.host.as_str(), smtp.port, smtp.security, smtp.save_sent),
        ("smtp.gmail.com", 465, Security::Tls, false)
    );
}

#[test]
fn on_a_terminal_a_number_not_listed_is_refused_and_less_than_goes_back() {
    let world = World::new(imap(APP_PASSWORD), smtp(false));
    let mut term = Script {
        interactive: true,
        lines: ["0", "9", "1", "", "<", "", "", "", "you@example.com"].into(),
        secrets: [APP_PASSWORD].into(),
        ..Script::default()
    };
    world.run(Args::default(), &mut term).unwrap();
    assert_eq!(
        term.said.matches("is not one of the numbers above").count(),
        2,
        "{}",
        term.said
    );
    assert_eq!(
        term.said.matches("Step 2 of 10").count(),
        2,
        "{}",
        term.said
    );
    // Every prompt after the first step says how to go back, questions included.
    for want in ["Gmail address", "App password"] {
        let prompt = term.prompts.iter().find(|p| p.starts_with(want)).unwrap();
        assert!(prompt.contains("< to go back"), "{prompt}");
    }
    assert_eq!(world.stored("gmail").as_deref(), Some(APP_PASSWORD));
}

#[test]
fn on_a_terminal_an_empty_app_password_is_asked_again() {
    let world = World::new(imap(APP_PASSWORD), smtp(false));
    let mut term = Script {
        secrets: ["", APP_PASSWORD].into(),
        ..typed(&[])
    };
    world.run(Args::default(), &mut term).unwrap();
    assert_eq!(term.secrets_asked, 2);
    assert!(
        term.said.contains("The app password is empty."),
        "{}",
        term.said
    );
    assert_eq!(world.stored("gmail").as_deref(), Some(APP_PASSWORD));
}
