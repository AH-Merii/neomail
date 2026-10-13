//! Shows a [`Session`] on the command line: each step on standard error, answers from the
//! terminal. Checks and the write are the caller's.

use anyhow::{Context, Result, bail};

use super::{Answers, CheckError, Event, Kind, Prompt, Session, View};
use crate::secret::{self, Input};

/// Where the wizard talks to the user. One real one ([`Stdio`]), scripted ones in tests.
pub trait Term {
    /// Whether steps are shown and answers typed; off a terminal only secrets are read.
    fn interactive(&self) -> bool;
    /// Text for the user, on standard error.
    fn say(&mut self, text: &str);
    /// One line typed after `prompt`, trimmed.
    fn line(&mut self, prompt: &str) -> Result<String>;
    /// A secret: without echo on a terminal, else one line of standard input.
    fn secret(&mut self, prompt: &str) -> Result<String>;
}

/// Standard error and standard input.
pub struct Stdio(pub Input);

impl Term for Stdio {
    fn interactive(&self) -> bool {
        self.0.can_prompt()
    }

    fn say(&mut self, text: &str) {
        eprintln!("{text}");
    }

    fn line(&mut self, prompt: &str) -> Result<String> {
        secret::prompt_line(prompt)
    }

    fn secret(&mut self, prompt: &str) -> Result<String> {
        if self.0.can_prompt() {
            // Empty included: the session's own check asks again.
            return rpassword::prompt_password(prompt).context("cannot read the password");
        }
        secret::read_secret(prompt, "password", self.0)
    }
}

/// What typing this at a prompt does: go back a step.
pub const BACK: &str = "<";

/// Runs `session` to its end: shows each step, runs `check` for each check step and `write` at
/// the end; what they return is said to the user. Fails, with nothing written, when the user stops or a check fails for good.
pub fn drive(
    session: &mut Session,
    term: &mut dyn Term,
    mut check: impl FnMut(&str, &Answers<'_>) -> Result<String, CheckError>,
    mut write: impl FnMut(&Answers<'_>) -> Result<String>,
) -> Result<()> {
    loop {
        let event = match session.prompt() {
            Prompt::Done => return Ok(()),
            Prompt::Stopped(advice) => bail!("{advice}\nNothing was written."),
            Prompt::Failed(view, fix) => bail!(
                "step {} of {} ({}): {fix}\nNothing was written.",
                view.number,
                view.total,
                view.step.title
            ),
            Prompt::Check(view, answers) => {
                if let Some(warning) = view.warning {
                    term.say(&format!("Warning: {warning}"));
                }
                let result = check(view.step.id, &answers);
                if let Ok(said) = &result
                    && !said.is_empty()
                {
                    term.say(said);
                }
                Event::Checked(result.map(drop))
            }
            Prompt::Commit(answers) => {
                let said = write(&answers)?;
                if !said.is_empty() {
                    term.say(&said);
                }
                Event::Written
            }
            Prompt::Show(view) => show(term, &view)?,
        };
        session.next(event);
    }
}

fn heading(view: &View<'_>) -> String {
    format!(
        "Step {} of {}: {}",
        view.number, view.total, view.step.title
    )
}

/// Shows a Console, Gate or Ask step and returns what the user did there.
fn show(term: &mut dyn Term, view: &View<'_>) -> Result<Event> {
    let step = view.step;
    if !term.interactive() {
        // Off a terminal only a secret is left to read: Console and Gate steps are skipped.
        return match &step.kind {
            Kind::Ask(q) => Ok(Event::Answer(term.secret(q.prompt)?)),
            _ => bail!("step {:?} cannot be shown off a terminal", step.id),
        };
    }
    let mut text = format!("\n{}\n{}", heading(view), indent(step.why));
    for link in &view.links {
        text.push_str(&format!("\n  {link}"));
    }
    if let Kind::Gate(cases) = &step.kind {
        for (i, case) in cases.iter().enumerate() {
            text.push_str(&format!("\n  {}. {}", i + 1, case.label));
        }
    }
    if let Some(next) = view.next {
        text.push_str(&format!("\nNext: {next}"));
    }
    if let Some(fix) = view.fix {
        text.push_str(&format!("\n{}", indent(fix)));
    }
    term.say(&text);
    let back = if view.can_go_back {
        format!(", {BACK} to go back")
    } else {
        String::new()
    };
    loop {
        let typed = match &step.kind {
            Kind::Console | Kind::ConsoleIf { .. } => {
                term.line(&format!("Enter when done{back}: "))?
            }
            Kind::Gate(cases) => term.line(&format!("Which one? [1-{}{back}] ", cases.len()))?,
            Kind::Ask(q) if q.secret => term.secret(&ask_prompt(q.prompt, view.can_go_back))?,
            Kind::Ask(q) => {
                let prompt = ask_prompt(q.prompt, view.can_go_back);
                match q.default {
                    Some(d) => term.line(&format!("{prompt}[{d}] "))?,
                    None => term.line(&prompt)?,
                }
            }
            Kind::Check { .. } | Kind::Commit => unreachable!("checks and the write are run"),
        };
        if view.can_go_back && typed.trim() == BACK {
            return Ok(Event::Back);
        }
        match &step.kind {
            Kind::Console | Kind::ConsoleIf { .. } => return Ok(Event::Continue),
            Kind::Gate(cases) => match typed.trim().parse::<usize>() {
                Ok(n) if (1..=cases.len()).contains(&n) => return Ok(Event::Choose(n - 1)),
                _ => term.say(&format!("{typed:?} is not one of the numbers above")),
            },
            Kind::Ask(q) => {
                let typed = match q.default {
                    Some(d) if typed.trim().is_empty() => d.to_owned(),
                    _ => typed,
                };
                return Ok(Event::Answer(typed));
            }
            Kind::Check { .. } | Kind::Commit => unreachable!(),
        }
    }
}

/// `prompt` ("Gmail address: ") with the way back in it: "Gmail address (< to go back): ".
fn ask_prompt(prompt: &str, can_go_back: bool) -> String {
    if !can_go_back {
        return prompt.to_owned();
    }
    let head = prompt.trim_end().trim_end_matches(':');
    match head.strip_suffix(')') {
        Some(open) => format!("{open}, {BACK} to go back): "),
        None => format!("{head} ({BACK} to go back): "),
    }
}

fn indent(text: &str) -> String {
    text.lines()
        .map(|l| format!("  {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}
