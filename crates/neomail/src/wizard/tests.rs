use clap::CommandFactory;

use super::gmail::GMAIL_APP_PASSWORD;
use super::*;

/// Every wizard `neomail account add` runs.
const ALL: &[&Wizard] = &[
    &GMAIL_APP_PASSWORD,
    &crate::oauth::steps::GMAIL,
    &crate::oauth::steps::OUTLOOK,
    &super::proton::PROTON,
];

/// The long flags of `neomail account add`.
fn add_flags() -> Vec<String> {
    let mut cli = crate::Cli::command();
    let add = cli
        .find_subcommand_mut("account")
        .and_then(|a| a.find_subcommand_mut("add"))
        .expect("neomail account add")
        .clone();
    add.get_arguments()
        .filter_map(|a| a.get_long().map(|l| format!("--{l}")))
        .collect()
}

#[test]
fn every_question_but_a_secret_has_a_flag_of_account_add() {
    let flags = add_flags();
    assert!(flags.contains(&"--username".to_owned()), "{flags:?}");
    let mut questions = 0;
    for wizard in ALL {
        for step in wizard.steps {
            let Kind::Ask(q) = &step.kind else { continue };
            questions += 1;
            match q.flag {
                Some(flag) => {
                    assert!(!q.secret, "{}: a secret is never a flag", step.id);
                    assert!(flags.iter().any(|f| f == flag), "{}: no {flag}", step.id);
                }
                None => assert!(q.secret, "{}/{} has no flag", wizard.id, step.id),
            }
        }
    }
    assert!(questions >= 3, "{questions}");
}

#[test]
fn checks_read_only_earlier_questions_and_mistakes_name_earlier_steps() {
    for wizard in ALL {
        let at = |id: &str| wizard.position(id);
        for (i, step) in wizard.steps.iter().enumerate() {
            assert_eq!(at(step.id), Some(i), "{}: ids are unique", step.id);
            if let Kind::Check { reads } = step.kind {
                assert!(!reads.is_empty(), "{}", step.id);
                for read in reads {
                    let r = at(read).unwrap_or_else(|| panic!("{}: no step {read}", step.id));
                    assert!(r < i, "{} reads {read}, which comes later", step.id);
                    assert!(matches!(wizard.steps[r].kind, Kind::Ask(_)), "{read}");
                }
            }
        }
        let last = wizard.steps.last().unwrap();
        assert!(
            matches!(last.kind, Kind::Commit),
            "{}: writes last",
            wizard.id
        );
        assert!(!wizard.mistakes.is_empty(), "{}", wizard.id);
        for m in wizard.mistakes {
            let check = at(m.check).unwrap_or_else(|| panic!("no check {}", m.check));
            assert!(matches!(wizard.steps[check].kind, Kind::Check { .. }));
            let step = at(m.step).unwrap_or_else(|| panic!("no step {}", m.step));
            assert!(
                step < check,
                "{} sends the user forward to {}",
                m.check,
                m.step
            );
        }
    }
}

/// The titles of the steps shown, in order, answering each the plain way, until the first
/// check; `None` when it reached none.
fn shown_before_first_check(mut session: Session) -> (Vec<&'static str>, Option<&'static str>) {
    let mut shown = Vec::new();
    for _ in 0..20 {
        let event = match session.prompt() {
            Prompt::Show(view) => {
                shown.push(view.step.id);
                match view.step.kind {
                    Kind::Gate(_) => Event::Choose(0),
                    Kind::Ask(_) => Event::Answer("you@example.com".into()),
                    _ => Event::Continue,
                }
            }
            Prompt::Check(view, _) => return (shown, Some(view.step.id)),
            _ => return (shown, None),
        };
        session.next(event);
    }
    panic!("no end: {shown:?}")
}

#[test]
fn on_a_terminal_every_question_is_asked_before_the_check_that_reads_it() {
    // A name default "gmail" must be offered to the user before the name is checked.
    let session = Session::new(&GMAIL_APP_PASSWORD, true, |_| None).unwrap();
    let (shown, check) = shown_before_first_check(session);
    assert_eq!(shown, ["eligible", "two-step", "create", "name"]);
    assert_eq!(check, Some("name-free"));
}

#[test]
fn off_a_terminal_flags_and_defaults_answer_and_advice_steps_are_skipped() {
    let flags = |f: &str| (f == "--username").then(|| "you@example.com".to_owned());
    let session = Session::new(&GMAIL_APP_PASSWORD, false, flags).unwrap();
    let (shown, check) = shown_before_first_check(session);
    assert_eq!((shown.len(), check), (0, Some("name-free")), "{shown:?}");

    let missing = Session::new(&GMAIL_APP_PASSWORD, false, |_| None).unwrap_err();
    assert_eq!(missing, StartError::Missing(vec!["--username"]));
    let bad = Session::new(&GMAIL_APP_PASSWORD, false, |_| Some("you".into())).unwrap_err();
    assert!(matches!(bad, StartError::Bad("--username", _)), "{bad:?}");
}

#[test]
fn back_returns_to_the_step_shown_before_and_is_not_offered_first() {
    let mut s = Session::new(&GMAIL_APP_PASSWORD, true, |_| None).unwrap();
    let Prompt::Show(first) = s.prompt() else {
        panic!()
    };
    assert!(!first.can_go_back);
    assert_eq!((first.number, first.total), (1, 10));
    s.next(Event::Back);
    s.next(Event::Choose(0));
    s.next(Event::Continue);
    let Prompt::Show(view) = s.next(Event::Back) else {
        panic!()
    };
    assert_eq!(
        (view.step.id, view.number, view.next),
        ("two-step", 2, Some("Create an app password"))
    );
}

#[test]
fn a_failed_check_asks_its_step_again_on_a_terminal_and_ends_off_one() {
    for interactive in [true, false] {
        let flags = |f: &str| (f == "--username").then(|| "you@example.com".to_owned());
        let mut s = Session::new(&GMAIL_APP_PASSWORD, interactive, flags).unwrap();
        let mut steps = 0;
        while !matches!(s.prompt(), Prompt::Check(ref v, _) if v.step.id == "login") {
            let event = match s.prompt() {
                Prompt::Check(..) => Event::Checked(Ok(())),
                Prompt::Show(v) if matches!(v.step.kind, Kind::Gate(_)) => Event::Choose(0),
                Prompt::Show(v) if matches!(v.step.kind, Kind::Ask(_)) => {
                    Event::Answer("gmail".into())
                }
                _ => Event::Continue,
            };
            s.next(event);
            steps += 1;
            assert!(steps < 20);
        }
        let refused = CheckError {
            kind: "refused",
            detail: "login as you@example.com failed".into(),
        };
        match s.next(Event::Checked(Err(refused))) {
            Prompt::Show(v) if interactive => {
                assert_eq!(v.step.id, "password");
                assert!(
                    v.fix.unwrap().contains("Advanced Protection"),
                    "{:?}",
                    v.fix
                );
            }
            Prompt::Failed(v, fix) if !interactive => {
                assert_eq!((v.step.id, v.number), ("password", 8));
                assert!(fix.contains("login as you@example.com failed"), "{fix}");
            }
            other => panic!("{interactive}: {other:?}"),
        }
    }
}

#[test]
fn a_check_reads_only_the_answers_it_declares() {
    let mut s = Session::new(&GMAIL_APP_PASSWORD, true, |_| None).unwrap();
    for _ in 0..20 {
        let event = match s.prompt() {
            Prompt::Check(view, answers) if view.step.id == "login" => {
                // "login" declares address and password; the name was answered but is not its.
                assert_eq!(answers.text("address"), Some("you@example.com"));
                assert!(answers.secret("password").is_some());
                assert!(answers.get("name").is_none());
                return;
            }
            Prompt::Check(..) => Event::Checked(Ok(())),
            Prompt::Show(v) => match v.step.kind {
                Kind::Gate(_) => Event::Choose(0),
                Kind::Ask(_) => Event::Answer("you@example.com".into()),
                _ => Event::Continue,
            },
            other => panic!("{other:?}"),
        };
        s.next(event);
    }
    panic!("never reached the login check");
}
