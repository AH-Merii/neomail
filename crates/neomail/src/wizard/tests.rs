use clap::CommandFactory;

use super::gmail::GMAIL_APP_PASSWORD;
use super::*;

/// Every wizard `neomail account add` runs.
const ALL: &[&Wizard] = &[
    &GMAIL_APP_PASSWORD,
    &crate::oauth::steps::GMAIL,
    &crate::oauth::steps::OUTLOOK,
    &super::proton::PROTON,
    &super::imap::IMAP,
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
            let Some(q) = step.kind.question() else {
                continue;
            };
            questions += 1;
            match q.flag {
                Some(flag) => {
                    assert!(!q.secret, "{}: a secret is never a flag", step.id);
                    assert!(flags.iter().any(|f| f == flag), "{}: no {flag}", step.id);
                }
                // Read from standard input, answered by its default, or asked only on a
                // terminal: a script never needs it.
                None => assert!(
                    q.secret || q.default.is_some() || matches!(step.kind, Kind::AskIf(..)),
                    "{}/{} has no flag",
                    wizard.id,
                    step.id
                ),
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
    assert_eq!(shown, ["eligible", "two-step", "name"]);
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
        ("two-step", 2, Some("Name the account"))
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

#[test]
fn a_page_is_one_of_its_steps_links_and_fixed_answers_pass_their_own_check() {
    let mut pages = 0;
    let mut choices = 0;
    for wizard in ALL {
        for step in wizard.steps {
            if let Some(page) = step.page {
                pages += 1;
                assert!(step.links.contains(&page), "{}: {page}", step.id);
            }
            if let Kind::Ask(q) = &step.kind {
                for c in q.choices {
                    choices += 1;
                    assert_eq!((q.clean)(c.value).map(|c| c.value), Ok(c.value.to_owned()));
                }
                if let (Some(d), false) = (q.default, q.choices.is_empty()) {
                    assert!(q.choices.iter().any(|c| c.value == d), "{}", step.id);
                }
            }
        }
    }
    assert!(pages >= 5 && choices >= 6, "{pages} {choices}");
}

#[test]
fn a_steps_text_marks_its_actions_notes_and_warnings() {
    let step = Step {
        id: "x",
        title: "X",
        why: "\
Some explanation that goes
on here.
- Click Enable, then
  Save.
This line explains, after the action.
Why External: a note
that continues.
Warning: mind the key.pem.",
        links: &[],
        page: None,
        kind: Kind::Console,
    };
    assert_eq!(
        step.lines(),
        [
            (Line::Say, "Some explanation that goes on here.".to_owned()),
            (Line::Action, "Click Enable, then Save.".to_owned()),
            (
                Line::Say,
                "This line explains, after the action.".to_owned()
            ),
            (
                Line::Note,
                "Why External: a note that continues.".to_owned()
            ),
            (Line::Warning, "Mind the key.pem.".to_owned()),
        ]
    );
}

/// The welcome's step count may grow only where it says so: while a step shown only for some
/// answers waits on an answer not given yet.
#[test]
fn the_step_count_grows_only_while_an_answer_it_depends_on_is_missing() {
    use crate::oauth::steps::{DEVICE_CODE, OUTLOOK, PORTAL, WORK};
    let open = Session::new(&OUTLOOK, true, |_| None).unwrap();
    assert!(open.depends_on_answers());
    let flags = |f: &str| match f {
        "--account-type" => Some(WORK.to_owned()),
        "--device-code" => Some(DEVICE_CODE.to_owned()),
        _ => None,
    };
    let mut decided = Session::new(&OUTLOOK, true, flags).unwrap();
    // The route has no flag: it is the first question left.
    assert!(decided.depends_on_answers());
    decided.next(Event::Answer(PORTAL.to_owned()));
    assert!(!decided.depends_on_answers());
    assert!(
        !Session::new(&GMAIL_APP_PASSWORD, true, |_| None)
            .unwrap()
            .depends_on_answers()
    );
}

fn yes_no(t: &str) -> Result<Cleaned, &'static str> {
    Ok(Cleaned::ok(t.trim()))
}

/// A wizard where answering "yes" brings one more step: the count the user sees follows it.
static GROWS: Wizard = Wizard {
    id: "grows",
    title: "a test",
    steps: &[
        Step {
            id: "more",
            title: "More?",
            why: "",
            links: &[],
            page: None,
            kind: Kind::Ask(Question {
                flag: None,
                prompt: "more? ",
                secret: false,
                default: Some("no"),
                choices: &[
                    Choice {
                        value: "no",
                        label: "no",
                    },
                    Choice {
                        value: "yes",
                        label: "yes",
                    },
                ],
                clean: yes_no,
            }),
        },
        Step {
            id: "extra",
            title: "The extra step",
            why: "",
            links: &[],
            page: None,
            kind: Kind::ConsoleIf(&[When {
                step: "more",
                is: &["yes"],
            }]),
        },
        Step {
            id: "last",
            title: "The last step",
            why: "",
            links: &[],
            page: None,
            kind: Kind::Console,
        },
        Step {
            id: "write",
            title: "Save it",
            why: "",
            links: &[],
            page: None,
            kind: Kind::Commit,
        },
    ],
    mistakes: &[],
};

/// The count, the outline and Next follow the answer about to be given, before it is given;
/// with none pending they follow the default, and an offered answer of the last run beats it.
#[test]
fn the_path_follows_the_answer_about_to_be_given() {
    let mut s = Session::new(&GROWS, true, |_| None).unwrap();
    let titles = |v: &View<'_>| v.outline.iter().map(|(t, _)| *t).collect::<Vec<_>>();
    let Prompt::Show(now) = s.prompt() else {
        panic!()
    };
    assert_eq!(
        (now.seen, now.seen_total, now.next),
        (1, 2, Some("The last step"))
    );
    let yes = s.preview("yes");
    assert_eq!(
        (yes.seen, yes.seen_total, yes.next),
        (1, 3, Some("The extra step"))
    );
    assert_eq!(titles(&yes), ["More?", "The extra step", "The last step"]);
    let no = s.preview("no");
    assert_eq!(titles(&no), ["More?", "The last step"]);
    s.offer("more", "yes");
    let Prompt::Show(offered) = s.prompt() else {
        panic!()
    };
    assert_eq!(offered.seen_total, 3);
    // Given, it holds: the extra step is step 2 of 3, and the last one names the save as next.
    let Prompt::Show(extra) = s.next(Event::Answer("yes".into())) else {
        panic!()
    };
    assert_eq!(
        (extra.step.id, extra.seen, extra.seen_total),
        ("extra", 2, 3)
    );
    let Prompt::Show(last) = s.next(Event::Continue) else {
        panic!()
    };
    assert_eq!(
        (last.seen, last.seen_total, last.next),
        (3, 3, Some("Save it"))
    );
}

/// Pointing at a case the setup stops at shows that no step follows it; pointing at one that
/// goes on shows the steps it leads to.
#[test]
fn at_a_gate_the_path_follows_the_case_pointed_at() {
    let s = Session::new(&GMAIL_APP_PASSWORD, true, |_| None).unwrap();
    let Kind::Gate(cases) = &s.wizard().steps[0].kind else {
        panic!("the first step is the gate")
    };
    let Prompt::Show(now) = s.prompt() else {
        panic!()
    };
    for (i, case) in cases.iter().enumerate() {
        let v = s.preview(&i.to_string());
        if case.stop.is_some() {
            assert_eq!((v.seen_total, v.next), (1, Some(STOPS_HERE)), "case {i}");
            assert!(v.outline.iter().all(|(_, p)| *p != Place::Left), "case {i}");
        } else {
            assert_eq!(
                (v.seen_total, v.next),
                (now.seen_total, now.next),
                "case {i}"
            );
            assert!(v.seen_total > 1 && v.next != Some(STOPS_HERE), "case {i}");
        }
    }
    assert!(cases.iter().any(|c| c.stop.is_some()) && cases.iter().any(|c| c.stop.is_none()));
}

/// After a failed check asks an earlier step again, the advice steps already passed between it
/// and the check are not shown again, and the outline counts them as done.
#[test]
fn a_step_asked_again_after_a_check_leads_back_to_the_check_without_repeating_advice() {
    use crate::oauth::steps::{OUTLOOK, UNKNOWN_ID};
    let mut s = Session::new(&OUTLOOK, true, |_| None).unwrap();
    let mut steps = 0;
    while !matches!(s.prompt(), Prompt::Check(ref v, _) if v.step.id == "client-check") {
        let event = match s.prompt() {
            Prompt::Check(..) => Event::Checked(Ok(())),
            Prompt::Show(v) if v.step.id == "client-id" => {
                Event::Answer("00000000-0000-0000-0000-000000000000".into())
            }
            Prompt::Show(v) if v.step.id == "email" => Event::Answer("you@example.com".into()),
            Prompt::Show(v) if v.step.id == "account-type" => Event::Answer("personal".into()),
            Prompt::Show(v) if v.step.id == "route" => Event::Answer("portal".into()),
            Prompt::Show(v) if v.step.id == "sign-in-method" => Event::Answer("browser".into()),
            Prompt::Show(v) if v.step.id == "name" => Event::Answer("outlook".into()),
            _ => Event::Continue,
        };
        s.next(event);
        steps += 1;
        assert!(steps < 30);
    }
    let unknown = CheckError {
        kind: UNKNOWN_ID,
        detail: "Microsoft does not know it".into(),
    };
    let Prompt::Show(again) = s.next(Event::Checked(Err(unknown))) else {
        panic!()
    };
    assert_eq!(
        (again.step.id, again.seen, again.seen_total),
        ("client-id", 6, 9)
    );
    assert!(
        again.outline[6..]
            .iter()
            .all(|(_, place)| *place == Place::Done),
        "{:?}",
        again.outline
    );
    let after = s.next(Event::Answer("11111111-1111-1111-1111-111111111111".into()));
    assert!(
        matches!(&after, Prompt::Check(v, _) if v.step.id == "ready"),
        "{after:?}"
    );
}

/// On the commands route, a client ID Microsoft does not know, or an app set up wrong, sends
/// the user back to the commands step, which the route shows, not to the browser steps.
#[test]
fn on_the_commands_route_a_failed_check_goes_back_to_the_commands() {
    use crate::oauth::steps::{NOT_MULTITENANT, OUTLOOK, UNKNOWN_ID};
    for kind in [UNKNOWN_ID, NOT_MULTITENANT] {
        let mut s = Session::new(&OUTLOOK, true, |_| None).unwrap();
        let mut steps = 0;
        while !matches!(s.prompt(), Prompt::Check(ref v, _) if v.step.id == "client-check") {
            let event = match s.prompt() {
                Prompt::Check(..) => Event::Checked(Ok(())),
                Prompt::Show(v) => match v.step.id {
                    "account-type" => Event::Answer("work".into()),
                    "route" => Event::Answer("commands".into()),
                    "sign-in-method" => Event::Answer("browser".into()),
                    "directory" => Event::Answer("contoso.com".into()),
                    "app" => Event::Answer(String::new()),
                    "commands" => Event::Answer("00000000-0000-0000-0000-000000000000".into()),
                    "email" => Event::Answer("you@example.com".into()),
                    "name" => Event::Answer("outlook".into()),
                    _ => Event::Continue,
                },
                other => panic!("{other:?}"),
            };
            s.next(event);
            steps += 1;
            assert!(steps < 30);
        }
        let error = CheckError {
            kind,
            detail: "Microsoft said no".into(),
        };
        let Prompt::Show(again) = s.next(Event::Checked(Err(error))) else {
            panic!()
        };
        assert_eq!(again.step.id, "commands", "{kind}");
        assert!(
            again
                .fix
                .is_some_and(|f| f.contains("Run the commands again")),
            "{kind}"
        );
    }
}
