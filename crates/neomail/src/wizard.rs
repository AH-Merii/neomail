//! The setup wizard model: a provider's steps as data, and a state machine that walks them. It
//! does no I/O: a driver shows each [`Prompt`] and feeds back an [`Event`], so the CLI
//! ([`cli`]) and, later, the TUI's add-account screen show the same steps. Checks and the write
//! are the driver's, through the answers the session hands it (DEC-2026-10-13-wizard-steps).

pub mod cli;
pub mod gmail;
#[cfg(test)]
mod tests;

use crate::secret::Password;

/// One provider's setup.
#[derive(Debug)]
pub struct Wizard {
    pub id: &'static str,
    pub steps: &'static [Step],
    /// What a failed check means, and which step to change.
    pub mistakes: &'static [Mistake],
}

#[derive(Debug)]
pub struct Step {
    /// Names the step in [`Kind::Check`] reads and in [`Mistake`]s.
    pub id: &'static str,
    pub title: &'static str,
    /// Why the step exists, or what to do there.
    pub why: &'static str,
    /// Always printed; a provider's page or the help page a fact comes from.
    pub links: &'static [&'static str],
    pub kind: Kind,
}

#[derive(Debug)]
pub enum Kind {
    /// The user acts on a provider's page or reads advice. Sets nothing, so it has no flag and
    /// is skipped when nothing can be asked.
    Console,
    /// A [`Kind::Console`] step shown only when the earlier question `step` was answered with
    /// one of `is`: a step that applies to some setups only, such as a personal account's.
    ConsoleIf {
        step: &'static str,
        is: &'static [&'static str],
    },
    /// The user picks the case that fits: a case without advice goes on, the others stop with it.
    /// Sets nothing, so it has no flag and is skipped when nothing can be asked; a later check
    /// catches the same cases and its [`Mistake`] repeats the advice.
    Gate(&'static [Case]),
    /// A value the result depends on.
    Ask(Question),
    /// The driver checks the answers of the steps named in `reads`, which all come before it.
    Check { reads: &'static [&'static str] },
    /// The driver writes. Nothing is written before it, and there is no going back after it.
    Commit,
}

#[derive(Debug)]
pub struct Case {
    pub label: &'static str,
    /// `None` goes on; `Some` is the advice the setup stops with.
    pub stop: Option<&'static str>,
}

#[derive(Debug)]
pub struct Question {
    /// The flag that answers it; `None` only for a secret, which is never a flag.
    pub flag: Option<&'static str>,
    pub prompt: &'static str,
    /// Read without echo on a terminal, else one line of standard input.
    pub secret: bool,
    pub default: Option<&'static str>,
    /// Tidies an answer, or says what to change.
    pub clean: fn(&str) -> Result<Cleaned, &'static str>,
}

/// An answer as it will be used, with a warning that does not stop it.
#[derive(Debug, PartialEq, Eq)]
pub struct Cleaned {
    pub value: String,
    pub warning: Option<&'static str>,
}

impl Cleaned {
    pub fn ok(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            warning: None,
        }
    }
}

/// A check failure of `kind` from the check step `check` means: change step `step`, as `fix`
/// says.
#[derive(Debug)]
pub struct Mistake {
    pub check: &'static str,
    pub kind: &'static str,
    pub step: &'static str,
    pub fix: &'static str,
}

/// How a check failed: `kind` picks the [`Mistake`]; `detail` is the error itself.
#[derive(Debug)]
pub struct CheckError {
    pub kind: &'static str,
    pub detail: String,
}

/// What the driver tells the session.
#[derive(Debug)]
pub enum Event {
    /// Enter at a [`Kind::Console`] step.
    Continue,
    /// The index of the [`Case`] picked at a [`Kind::Gate`].
    Choose(usize),
    /// The text typed (or read) at a [`Kind::Ask`].
    Answer(String),
    /// Back to the step shown before this one.
    Back,
    /// The result of the check the session asked for.
    Checked(Result<(), CheckError>),
    /// The write the session asked for is done.
    Written,
}

/// What the driver does next.
#[derive(Debug)]
pub enum Prompt<'s> {
    /// Show the step and wait for an event (Console, Gate or Ask).
    Show(View<'s>),
    /// Run this check on `answers`, then send [`Event::Checked`].
    Check(View<'s>, Answers<'s>),
    /// Write, then send [`Event::Written`].
    Commit(Answers<'s>),
    /// The user picked a case with no way through: its advice. Nothing was written.
    Stopped(&'static str),
    /// A failure that cannot be fixed here: on a terminal a check of the step itself, else any.
    /// Nothing was written.
    Failed(View<'s>, String),
    Done,
}

/// Where the user is, why, and what is next.
#[derive(Debug)]
pub struct View<'s> {
    /// 1-based, of `total`.
    pub number: usize,
    pub total: usize,
    pub step: &'static Step,
    /// The step's links, with a `{id}` placeholder filled from question `id`'s answer.
    pub links: Vec<String>,
    /// The title of the next step that will be shown or run, if any.
    pub next: Option<&'static str>,
    /// Why this step is shown again: what to change.
    pub fix: Option<&'s str>,
    /// About the answer just given, which still goes on.
    pub warning: Option<&'static str>,
    pub can_go_back: bool,
}

/// An answer, kept as given.
#[derive(Debug)]
pub enum Answer {
    Text(String),
    Secret(Password),
}

/// The answers a check or the write may read.
#[derive(Debug)]
pub struct Answers<'s> {
    session: &'s Session,
    /// Step ids it may read; `None` reads all (the write).
    reads: Option<&'static [&'static str]>,
}

impl Answers<'_> {
    /// The answer of step `id`, if this check may read it and it was answered.
    pub fn get(&self, id: &str) -> Option<&Answer> {
        if self.reads.is_some_and(|reads| !reads.contains(&id)) {
            return None;
        }
        let i = self.session.wizard.position(id)?;
        self.session.answers[i].as_ref()
    }

    pub fn text(&self, id: &str) -> Option<&str> {
        match self.get(id)? {
            Answer::Text(t) => Some(t),
            Answer::Secret(_) => None,
        }
    }

    pub fn secret(&self, id: &str) -> Option<&Password> {
        match self.get(id)? {
            Answer::Secret(p) => Some(p),
            Answer::Text(_) => None,
        }
    }
}

/// Why a session cannot start.
#[derive(Debug, PartialEq, Eq)]
pub enum StartError {
    /// Nothing can be asked, and these flags are missing.
    Missing(Vec<&'static str>),
    /// A flag's value is unusable: what to change.
    Bad(&'static str, &'static str),
}

impl Wizard {
    pub fn position(&self, id: &str) -> Option<usize> {
        self.steps.iter().position(|s| s.id == id)
    }
}

/// One run through a wizard.
#[derive(Debug)]
pub struct Session {
    wizard: &'static Wizard,
    /// On a terminal and allowed to ask: Console and Gate steps show, failures ask again.
    interactive: bool,
    at: usize,
    /// The steps shown so far, for Back.
    shown: Vec<usize>,
    answers: Vec<Option<Answer>>,
    /// Answered by a flag (or a default when nothing can be asked): never shown.
    given: Vec<bool>,
    fix: Option<String>,
    warning: Option<&'static str>,
    /// Set by a failure that ends the run, or a stop.
    end: Option<End>,
}

#[derive(Debug)]
enum End {
    Stopped(&'static str),
    Failed(usize, String),
    Done,
}

impl Session {
    /// Starts `wizard`. `flag` gives each flag's value, if passed. Flags answer their steps;
    /// when nothing can be asked (`interactive` false), defaults answer the rest, and a
    /// question with neither is a missing flag. Secrets are never flags: they are read later.
    pub fn new(
        wizard: &'static Wizard,
        interactive: bool,
        flag: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, StartError> {
        let n = wizard.steps.len();
        let mut answers: Vec<Option<Answer>> = (0..n).map(|_| None).collect();
        let mut given = vec![false; n];
        let mut missing = Vec::new();
        for (i, step) in wizard.steps.iter().enumerate() {
            let Kind::Ask(q) = &step.kind else { continue };
            let Some(name) = q.flag else { continue };
            let value = flag(name).filter(|v| !v.trim().is_empty());
            let value = match (value, q.default) {
                (Some(v), _) => v,
                (None, Some(d)) if !interactive => d.to_owned(),
                (None, _) if !interactive => {
                    missing.push(name);
                    continue;
                }
                (None, _) => continue,
            };
            let cleaned = (q.clean)(&value).map_err(|fix| StartError::Bad(name, fix))?;
            answers[i] = Some(Answer::Text(cleaned.value));
            given[i] = true;
        }
        if !missing.is_empty() {
            return Err(StartError::Missing(missing));
        }
        let mut session = Self {
            wizard,
            interactive,
            at: 0,
            shown: Vec::new(),
            answers,
            given,
            fix: None,
            warning: None,
            end: None,
        };
        session.settle();
        Ok(session)
    }

    /// Marks step `id` as answered from elsewhere (a secret from `--password-command`), so it is
    /// never shown and its answer stays empty.
    pub fn provided(mut self, id: &str) -> Self {
        if let Some(i) = self.wizard.position(id) {
            self.given[i] = true;
            self.settle();
        }
        self
    }

    /// Moves past the steps that are not shown: answered (by a flag or earlier), or Console
    /// and Gate steps when nothing can be asked.
    fn settle(&mut self) {
        while self.at < self.wizard.steps.len() && self.skipped(self.at) {
            self.at += 1;
        }
    }

    /// Whether step `i` is passed without showing or running it.
    fn skipped(&self, i: usize) -> bool {
        match self.wizard.steps[i].kind {
            Kind::Console | Kind::Gate(_) => !self.interactive,
            Kind::ConsoleIf { step, is } => !self.interactive || !self.answered(step, is),
            // An answer kept from earlier: after a failed check asks an earlier step
            // again, the steps between keep theirs. Back still shows them.
            Kind::Ask(_) => self.given[i] || self.answers[i].is_some(),
            Kind::Check { .. } | Kind::Commit => false,
        }
    }

    /// Whether question `id` was answered with one of `values`.
    fn answered(&self, id: &str, values: &[&str]) -> bool {
        self.text(id).is_some_and(|t| values.contains(&t))
    }

    fn text(&self, id: &str) -> Option<&str> {
        match self.answers.get(self.wizard.position(id)?)? {
            Some(Answer::Text(t)) => Some(t),
            _ => None,
        }
    }

    /// `link` with a `{id}` placeholder filled with question `id`'s answer. Without one, the
    /// query parameter that holds it is dropped: it must be the link's last.
    fn link(&self, link: &str) -> String {
        let Some((head, rest)) = link.split_once('{') else {
            return link.to_owned();
        };
        let Some((id, tail)) = rest.split_once('}') else {
            return link.to_owned();
        };
        match self.text(id).filter(|t| !t.is_empty()) {
            Some(value) => format!("{head}{value}{tail}"),
            None => head[..head.rfind(['?', '&']).unwrap_or(head.len())].to_owned(),
        }
    }

    /// Goes on from the current step, which the user has passed.
    fn advance(&mut self) {
        self.shown.push(self.at);
        self.at += 1;
        self.fix = None;
        self.settle();
    }

    fn view(&self, at: usize) -> View<'_> {
        let steps = self.wizard.steps;
        View {
            number: at + 1,
            total: steps.len(),
            step: &steps[at],
            links: steps[at].links.iter().map(|l| self.link(l)).collect(),
            next: (at + 1..steps.len())
                .find(|&i| !self.skipped(i))
                .map(|i| steps[i].title),
            fix: self.fix.as_deref(),
            warning: self.warning,
            can_go_back: !self.shown.is_empty(),
        }
    }

    /// What to do now.
    pub fn prompt(&self) -> Prompt<'_> {
        match &self.end {
            Some(End::Stopped(advice)) => return Prompt::Stopped(advice),
            Some(End::Failed(at, fix)) => return Prompt::Failed(self.view(*at), fix.clone()),
            Some(End::Done) => return Prompt::Done,
            None => {}
        }
        let Some(step) = self.wizard.steps.get(self.at) else {
            return Prompt::Done;
        };
        match step.kind {
            Kind::Check { reads } => Prompt::Check(
                self.view(self.at),
                Answers {
                    session: self,
                    reads: Some(reads),
                },
            ),
            Kind::Commit => Prompt::Commit(Answers {
                session: self,
                reads: None,
            }),
            _ => Prompt::Show(self.view(self.at)),
        }
    }

    /// Takes the user's or the driver's `event` and says what to do next. An event that does
    /// not fit the current step changes nothing.
    pub fn next(&mut self, event: Event) -> Prompt<'_> {
        if self.end.is_none() {
            self.step(event);
        }
        self.prompt()
    }

    fn step(&mut self, event: Event) {
        let Some(step) = self.wizard.steps.get(self.at) else {
            return;
        };
        match (&step.kind, event) {
            (Kind::Commit, Event::Written) => self.end = Some(End::Done),
            (Kind::Commit, _) => {}
            (_, Event::Back) => {
                if let Some(back) = self.shown.pop() {
                    self.at = back;
                    self.fix = None;
                    self.warning = None;
                }
            }
            (Kind::Console | Kind::ConsoleIf { .. }, Event::Continue) => self.advance(),
            (Kind::Gate(cases), Event::Choose(i)) => match cases.get(i).map(|c| c.stop) {
                Some(None) => self.advance(),
                Some(Some(advice)) => self.end = Some(End::Stopped(advice)),
                None => {}
            },
            (Kind::Ask(q), Event::Answer(text)) => match (q.clean)(&text) {
                Ok(cleaned) => {
                    self.answers[self.at] = Some(if q.secret {
                        Answer::Secret(Password::new(cleaned.value))
                    } else {
                        Answer::Text(cleaned.value)
                    });
                    self.warning = cleaned.warning;
                    self.advance();
                }
                Err(fix) if self.interactive => self.fix = Some(fix.to_owned()),
                Err(fix) => self.end = Some(End::Failed(self.at, fix.to_owned())),
            },
            (Kind::Check { .. }, Event::Checked(Ok(()))) => {
                // Checks are not shown, so Back skips them and runs them again on the way.
                self.at += 1;
                self.settle();
            }
            (Kind::Check { .. }, Event::Checked(Err(e))) => self.failed(step.id, e),
            _ => {}
        }
    }

    /// A check failed: on a terminal, the step its mistake names is asked again, unless that is
    /// the check itself or a step a flag answered; otherwise the run ends.
    fn failed(&mut self, check: &str, e: CheckError) {
        let mistake = self
            .wizard
            .mistakes
            .iter()
            .find(|m| m.check == check && m.kind == e.kind);
        let (target, fix) = match mistake.and_then(|m| Some((self.wizard.position(m.step)?, m))) {
            Some((target, m)) => (target, format!("{}\n{}", e.detail, m.fix)),
            None => (self.at, e.detail),
        };
        let can_ask_again = self.interactive && target < self.at && !self.given[target];
        if can_ask_again {
            self.shown.retain(|&s| s < target);
            self.answers[target] = None;
            self.at = target;
            self.warning = None;
            self.fix = Some(fix);
        } else {
            self.end = Some(End::Failed(target, fix));
        }
    }
}
