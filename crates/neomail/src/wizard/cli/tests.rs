//! The terminal interface through the scripted seam: keys in, screens read back. The wizard is
//! the app-password one, whose checks and write are recorded here instead of run.

use std::cell::RefCell;

use super::*;
use crate::wizard::gmail::GMAIL_APP_PASSWORD;
use crate::wizard::script::Script;
use crate::wizard::term::Key::{self, *};

const SECRET: &str = "s3cret-typed-here";

fn look(paint: Paint, sym: Symbols) -> Look {
    Look {
        paint,
        sym,
        hyperlinks: false,
    }
}

fn plain() -> Look {
    look(Paint::OFF, Symbols::UNICODE)
}

fn typed(text: &str) -> Vec<Key> {
    text.chars().map(Char).chain([Enter]).collect()
}

/// The keys from the welcome screen to the address step: start, the first case, step 2 done,
/// step 3 skipped, the name's default kept.
fn to_address() -> Vec<Key> {
    vec![Enter, Enter, Enter, Char('s'), Enter]
}

/// What the checks and the write saw, in order.
#[derive(Default)]
struct Seen {
    events: RefCell<Vec<String>>,
}

impl Seen {
    fn has(&self, what: &str) -> bool {
        self.events.borrow().iter().any(|e| e == what)
    }
}

/// Runs the app-password wizard on `script`; checks fail with `refused` when `fails` names
/// them.
fn run(
    script: &mut Script,
    look: Look,
    open_pages: bool,
    opener: &dyn Fn(&str) -> Result<String>,
    memory: Memory,
    seen: &Seen,
    fails: &[&str],
) -> Result<()> {
    let mut ui = Ui::new(script, look, opener, open_pages, memory);
    let interactive = ui.interactive();
    let mut session = Session::new(&GMAIL_APP_PASSWORD, interactive, |_| None).unwrap();
    let check = |id: &str, answers: &Answers<'_>, _: &mut Ask<'_, '_>| {
        seen.events.borrow_mut().push(id.to_owned());
        if let Some(p) = answers.secret("password") {
            seen.events
                .borrow_mut()
                .push(format!("password={}", p.expose()));
        }
        match fails.contains(&id) {
            true => Err(CheckError {
                kind: "refused",
                detail: "the server said no".into(),
            }),
            false => Ok(String::new()),
        }
    };
    let write = |answers: &Answers<'_>| {
        let address = answers.text("address").unwrap_or_default();
        seen.events.borrow_mut().push(format!("write {address}"));
        Ok(Saved::default())
    };
    drive(&mut session, &mut ui, check, write)
}

fn headers(script: &Script) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for frame in &script.frames {
        let header = screen::plain(&frame.lines[0]);
        if out.last() != Some(&header) {
            out.push(header);
        }
    }
    out
}

#[test]
fn a_list_is_picked_with_the_arrows_j_and_k_a_digit_or_by_typing_to_narrow() {
    let items = [
        ("gmail", "Gmail"),
        ("outlook", "Outlook"),
        ("starttls", "STARTTLS"),
        ("tls", "TLS from the first byte"),
        ("proton", "Proton Mail"),
    ];
    let mut prot = typed("prot");
    prot.insert(0, Down);
    let cases: Vec<(Vec<Key>, Option<usize>)> = vec![
        (vec![Enter], Some(0)),
        (vec![Down, Down, Up, Enter], Some(1)),
        (vec![Char('j'), Char('j'), Char('k'), Enter], Some(1)),
        (vec![Char('4'), Enter], Some(3)),
        (prot, Some(4)),
        // Narrowing ranks an exact name first, though "starttls" also holds it.
        (typed("tls"), Some(3)),
        // Once text is typed, j narrows like any letter: "tj" matches nothing, so Enter picks
        // nothing until it is erased, and "t" ranks the name that starts with it first.
        (vec![Char('t'), Char('j'), Enter, Backspace, Enter], Some(3)),
        (vec![Char('z'), Esc, Enter], Some(0)),
        (vec![Down, Interrupt], None),
    ];
    for (keys, want) in cases {
        let mut script = Script::keys(&keys);
        let mut ui = Ui::new(&mut script, plain(), &no_opener, false, Memory::none());
        let got = choose(&mut ui, "Which kind of account?", &items, 0).unwrap();
        assert_eq!(got, want, "{keys:?}");
    }
}

#[test]
fn going_back_skipping_and_quitting_are_keys_each_screen_names() {
    let mut keys = vec![Enter, Enter, Esc, Enter, Char('s'), Char('q')];
    let mut script = Script::keys(&keys);
    let seen = Seen::default();
    let e = run(
        &mut script,
        plain(),
        false,
        &no_opener,
        Memory::none(),
        &seen,
        &[],
    )
    .unwrap_err();
    assert!(format!("{e}").contains("stopped at step 3"), "{e}");
    assert!(format!("{e}").contains("nothing was written"), "{e}");
    assert!(!seen.has("write you@example.com") && seen.events.borrow().is_empty());
    let steps: Vec<String> = headers(&script)
        .into_iter()
        .filter(|h| h.contains("Step"))
        .collect();
    let want = ["Step 1/6", "Step 2/6", "Step 1/6", "Step 2/6", "Step 3/6"];
    assert_eq!(steps.len(), want.len(), "{steps:?}");
    for (h, w) in steps.iter().zip(want) {
        assert!(h.starts_with(&format!("▸ {w} · ")), "{h}");
    }
    let step_2 = script
        .frames
        .iter()
        .map(|f| f.lines.join("\n"))
        .find(|s| s.contains("Step 2/6"));
    let step_2 = step_2.unwrap();
    for key in ["Enter done", "s skip, already done", "Esc back", "q quit"] {
        assert!(step_2.contains(key), "{key}: {step_2}");
    }
    // The first step has nowhere to go back to, and says so by not offering it.
    let step_1 = script
        .frames
        .iter()
        .map(|f| f.lines.join("\n"))
        .find(|s| s.contains("Step 1/6"));
    assert!(!step_1.unwrap().contains("Esc back"));
    // Ctrl-C quits from a question too.
    keys = to_address();
    keys.push(Interrupt);
    let mut script = Script::keys(&keys);
    let e = run(
        &mut script,
        plain(),
        false,
        &no_opener,
        Memory::none(),
        &seen,
        &[],
    )
    .unwrap_err();
    assert!(format!("{e}").contains("stopped at step 5"), "{e}");
}

#[test]
fn a_wrong_answer_is_shown_under_it_and_fixed_without_starting_over() {
    let mut keys = to_address();
    keys.extend(typed("you"));
    keys.extend(typed("@example.com"));
    keys.push(Interrupt);
    let mut script = Script::keys(&keys);
    let seen = Seen::default();
    run(
        &mut script,
        plain(),
        false,
        &no_opener,
        Memory::none(),
        &seen,
        &[],
    )
    .unwrap_err();
    let wrong = script
        .frames
        .iter()
        .map(|f| f.lines.iter().map(|l| screen::plain(l)).collect::<Vec<_>>())
        .find(|lines| lines.iter().any(|l| l.contains("✗ That is not an address")))
        .expect("the error is shown");
    let prompt = wrong
        .iter()
        .position(|l| l.contains("? Gmail address: you"))
        .unwrap();
    assert!(
        wrong[prompt + 1].contains("✗ That is not an address"),
        "{wrong:#?}"
    );
    // The text typed stayed, so the fix only added to it, and the next step came.
    assert!(
        headers(&script)
            .last()
            .unwrap()
            .contains("Step 6/6 · Paste the app password")
    );
    assert!(seen.has("ready") && !seen.has("login"));
}

/// Every screen and everything said, escapes included.
fn everything(script: &Script) -> String {
    let frames: Vec<String> = script.frames.iter().map(|f| f.lines.join("\n")).collect();
    format!("{}\n{}", frames.join("\n"), script.said)
}

#[test]
fn a_secret_is_never_shown_nor_kept_and_a_second_run_offers_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let id = GMAIL_APP_PASSWORD.id;
    let seen = Seen::default();
    // The first run gets to the save and says no.
    let mut keys = to_address();
    keys.extend(typed("you@example.com"));
    keys.extend(typed(SECRET));
    keys.push(Char('n'));
    let mut script = Script::keys(&keys);
    let memory = Memory::load(dir.path(), id);
    let e = run(&mut script, plain(), false, &no_opener, memory, &seen, &[]).unwrap_err();
    assert!(format!("{e}").contains("nothing was written"), "{e}");
    assert!(
        seen.has(&format!("password={SECRET}")),
        "the login check got it"
    );
    assert!(!seen.has("write you@example.com"));
    assert!(!everything(&script).contains(SECRET));
    let file = dir.path().join(format!("wizard-{id}.json"));
    let kept = std::fs::read_to_string(&file).unwrap();
    assert!(
        kept.contains("you@example.com") && kept.contains("gmail"),
        "{kept}"
    );
    assert!(!kept.contains(SECRET), "{kept}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    // The second run offers the address again: Enter keeps it; the save removes the file.
    let mut keys = to_address();
    keys.push(Enter);
    keys.extend(typed(SECRET));
    keys.push(Char('y'));
    let mut script = Script::keys(&keys);
    let memory = Memory::load(dir.path(), id);
    run(&mut script, plain(), false, &no_opener, memory, &seen, &[]).unwrap();
    let all = everything(&script);
    assert!(all.contains("2 answers from your last run"), "{all}");
    assert!(all.contains("Enter keeps you@example.com"), "{all}");
    assert!(seen.has("write you@example.com"));
    assert!(!file.exists());
}

#[test]
fn without_colour_nothing_is_styled_and_outside_utf_8_every_symbol_is_ascii() {
    let mut keys = to_address();
    keys.extend(typed("you"));
    keys.push(Interrupt);
    for (paint, sym) in [(Paint::OFF, Symbols::ASCII), (Paint::ON, Symbols::UNICODE)] {
        let mut script = Script::keys(&keys);
        let seen = Seen::default();
        run(
            &mut script,
            look(paint, sym),
            false,
            &no_opener,
            Memory::none(),
            &seen,
            &[],
        )
        .unwrap_err();
        let lines: Vec<&String> = script.frames.iter().flat_map(|f| &f.lines).collect();
        assert!(lines.len() > 20);
        let styled = lines.iter().any(|l| l.contains('\x1b'));
        let ascii = lines.iter().all(|l| l.is_ascii());
        assert_eq!(
            (styled, ascii),
            (paint.is_on(), sym == Symbols::ASCII),
            "{lines:#?}"
        );
        if sym == Symbols::ASCII {
            // The words and signs still say what colour would: the error, the steps done.
            let all = everything(&script);
            assert!(all.contains("x That is not an address") && all.contains("+ Can this"));
        }
    }
}

#[test]
fn pages_open_only_on_a_terminal_without_no_browser_nor_ssh_without_a_display() {
    let env = |vars: &'static [(&'static str, &'static str)]| {
        move |name: &str| {
            vars.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_owned())
        }
    };
    let ssh: &[(&str, &str)] = &[("SSH_CONNECTION", "10.0.0.2 5000 10.0.0.1 22")];
    let ssh_x: &[(&str, &str)] = &[("SSH_CONNECTION", "x"), ("DISPLAY", "localhost:10.0")];
    assert!(may_open_pages(true, false, env(&[])));
    assert!(may_open_pages(true, false, env(ssh_x)));
    assert!(!may_open_pages(true, true, env(&[])));
    assert!(!may_open_pages(true, false, env(ssh)));
    assert!(!may_open_pages(false, false, env(&[])));

    let keys = [Enter, Enter, Enter, Esc, Enter, Enter, Interrupt];
    let opened = RefCell::new(Vec::new());
    let opener = |url: &str| {
        opened.borrow_mut().push(url.to_owned());
        Ok("stub".to_owned())
    };
    for open in [true, false] {
        opened.borrow_mut().clear();
        let mut script = Script::keys(&keys);
        let seen = Seen::default();
        run(
            &mut script,
            plain(),
            open,
            &opener,
            Memory::none(),
            &seen,
            &[],
        )
        .unwrap_err();
        let all = everything(&script);
        if open {
            // Each step's page once, though step 2 showed twice.
            let want = [
                "https://myaccount.google.com/signinoptions/two-step-verification",
                "https://myaccount.google.com/apppasswords",
            ];
            assert_eq!(*opened.borrow(), want);
            assert!(all.contains("↗ opening https://myaccount.google.com/apppasswords"));
        } else {
            assert!(opened.borrow().is_empty());
            assert!(all.contains("Open it yourself: https://myaccount.google.com/apppasswords"));
        }
    }
    let mut off = Script::off_terminal(&[]);
    let ui = Ui::new(&mut off, plain(), &opener, true, Memory::none());
    assert!(!ui.open_pages, "off a terminal");
}

#[test]
fn raw_mode_is_left_after_a_quit_an_error_and_a_panic() {
    let seen = Seen::default();
    // A quit.
    let mut script = Script::keys(&[Enter, Interrupt]);
    run(
        &mut script,
        plain(),
        false,
        &no_opener,
        Memory::none(),
        &seen,
        &[],
    )
    .unwrap_err();
    assert!(script.raw_ons > 0 && !script.raw);
    // An error from a key read: the keys ran out.
    let mut script = Script::keys(&[Enter, Enter]);
    run(
        &mut script,
        plain(),
        false,
        &no_opener,
        Memory::none(),
        &seen,
        &[],
    )
    .unwrap_err();
    assert!(script.raw_ons > 0 && !script.raw);
    // A panic in a check, while it unwinds.
    let mut keys = to_address();
    keys.extend(typed("you@example.com"));
    let mut script = Script::keys(&keys);
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut ui = Ui::new(&mut script, plain(), &no_opener, false, Memory::none());
        let mut session = Session::new(&GMAIL_APP_PASSWORD, true, |_| None).unwrap();
        let check = |id: &str, _: &Answers<'_>, _: &mut Ask<'_, '_>| match id {
            "ready" => panic!("a bug in a check"),
            _ => Ok(String::new()),
        };
        let _ = drive(&mut session, &mut ui, check, |_| Ok(Saved::default()));
    }));
    assert!(caught.is_err());
    assert!(script.raw_ons > 0 && !script.raw);
}

#[test]
fn nothing_is_written_until_every_check_has_passed() {
    let seen = Seen::default();
    let mut keys = to_address();
    keys.extend(typed("you@example.com"));
    keys.extend(typed(SECRET));
    // The login is refused: the password is asked again, and the user gives up.
    keys.extend(typed(SECRET));
    keys.push(Interrupt);
    let mut script = Script::keys(&keys);
    run(
        &mut script,
        plain(),
        false,
        &no_opener,
        Memory::none(),
        &seen,
        &["login"],
    )
    .unwrap_err();
    let events = seen.events.borrow().clone();
    assert_eq!(
        events.iter().filter(|e| *e == "login").count(),
        2,
        "{events:?}"
    );
    assert!(!events.iter().any(|e| e.starts_with("write")), "{events:?}");
    assert!(everything(&script).contains("✗ the server said no"));
    // When they all pass, the write comes after the last one, and only after a yes.
    let seen = Seen::default();
    let mut keys = to_address();
    keys.extend(typed("you@example.com"));
    keys.extend(typed(SECRET));
    keys.push(Enter);
    let mut script = Script::keys(&keys);
    run(
        &mut script,
        plain(),
        false,
        &no_opener,
        Memory::none(),
        &seen,
        &[],
    )
    .unwrap();
    let events = seen.events.borrow().clone();
    let checks = ["name-free", "ready", "login", "write you@example.com"];
    let order: Vec<&str> = events
        .iter()
        .map(String::as_str)
        .filter(|e| checks.contains(e))
        .collect();
    assert_eq!(order, checks);
}

/// What a check or the save prints itself (a sign-in's code, "Added ...") must land in cooked
/// mode below the last screen: raw mode off, so Ctrl-C works and lines start at column 1, and
/// the cursor off that screen, so the summary said after it is not drawn over it.
#[test]
fn checks_a_confirm_and_the_save_run_in_cooked_mode_below_the_screen() {
    let mut keys = to_address();
    keys.extend(typed("you@example.com"));
    keys.extend(typed(SECRET));
    keys.extend([Char('y'), Enter]);
    let mut script = Script::keys(&keys);
    let can_print = script.can_print.clone();
    let seen = Seen::default();
    let mut ui = Ui::new(&mut script, plain(), &no_opener, false, Memory::none());
    let mut session = Session::new(&GMAIL_APP_PASSWORD, true, |_| None).unwrap();
    let check = |id: &str, _: &Answers<'_>, ask: &mut Ask<'_, '_>| {
        seen.events
            .borrow_mut()
            .push(format!("{id} {}", can_print.get()));
        if id == "login" {
            assert_eq!(
                ask.confirm("Replace it", "An old one is there.", "Replace it?")
                    .unwrap(),
                Some(true)
            );
            seen.events
                .borrow_mut()
                .push(format!("confirmed {}", can_print.get()));
        }
        Ok(String::new())
    };
    let write = |_: &Answers<'_>| {
        seen.events
            .borrow_mut()
            .push(format!("write {}", can_print.get()));
        Ok(Saved::default())
    };
    drive(&mut session, &mut ui, check, write).unwrap();
    let events = seen.events.borrow().clone();
    let want = [
        "name-free true",
        "ready true",
        "login true",
        "confirmed true",
        "write true",
    ];
    assert_eq!(events, want);
}
