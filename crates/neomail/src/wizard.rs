//! The setup wizard model: a provider's steps as data, and a state machine that walks them. It
//! does no I/O: a driver shows each [`Prompt`] and feeds back an [`Event`], so the CLI
//! ([`cli`]) and, later, the TUI's add-account screen show the same steps. Checks and the write
//! are the driver's, through the answers the session hands it (DEC-2026-10-13-wizard-steps).

pub mod capture;
pub mod cli;
pub mod gmail;
pub mod imap;
pub mod proton;
pub mod remember;
pub mod screen;
#[cfg(test)]
pub mod script;
pub mod term;
#[cfg(test)]
mod tests;

use crate::secret::Password;

/// One provider's setup.
#[derive(Debug)]
pub struct Wizard {
    pub id: &'static str,
    /// What it sets up, for the welcome screen: "a Gmail account, with an app password".
    pub title: &'static str,
    pub steps: &'static [Step],
    /// What a failed check means, and which step to change.
    pub mistakes: &'static [Mistake],
}

#[derive(Debug)]
pub struct Step {
    /// Names the step in [`Kind::Check`] reads and in [`Mistake`]s.
    pub id: &'static str,
    pub title: &'static str,
    /// Why the step exists, or what to do there: paragraphs, hand-wrapped. A line that starts
    /// with `- ` is an action the user takes (its own lines after it are indented), one with
    /// `Why: ` or `Note: ` a side note, and one with `Warning: ` a warning; the lines after it,
    /// up to the next such line or a blank one, belong to it ([`Step::lines`]).
    pub why: &'static str,
    /// Always printed; a provider's page or the help page a fact comes from.
    pub links: &'static [&'static str],
    /// The one page of `links` the step sends the user to, opened for them where it can be.
    pub page: Option<&'static str>,
    pub kind: Kind,
}

#[derive(Debug)]
pub enum Kind {
    /// The user acts on a provider's page or reads advice. Sets nothing, so it has no flag and
    /// is skipped when nothing can be asked.
    Console,
    /// A [`Kind::Console`] step shown only when every [`When`] holds: a step that applies to
    /// some setups only, such as a personal account's.
    ConsoleIf(&'static [When]),
    /// The user picks the case that fits: a case without advice goes on, the others stop with it.
    /// Sets nothing, so it has no flag and is skipped when nothing can be asked; a later check
    /// catches the same cases and its [`Mistake`] repeats the advice.
    Gate(&'static [Case]),
    /// A value the result depends on.
    Ask(Question),
    /// A [`Kind::Ask`] asked only on a terminal and when every [`When`] holds; otherwise passed
    /// unanswered, so never a missing flag.
    AskIf(&'static [When], Question),
    /// The user runs a command the step shows, and its result answers a later question: see
    /// [`Run`]. Shown only on a terminal, when every [`When`] holds and no flag answered it.
    Run(Run),
    /// The driver checks the answers of the steps named in `reads`, which all come before it.
    Check { reads: &'static [&'static str] },
    /// The driver writes. Nothing is written before it, and there is no going back after it.
    Commit,
}

/// A condition on an earlier question: its answer (or the one expected) is one of `is`.
#[derive(Debug)]
pub struct When {
    pub step: &'static str,
    pub is: &'static [&'static str],
}

/// Reads an answer given so far, by step id.
pub type AnswerOf<'a> = dyn Fn(&str) -> Option<String> + 'a;

/// A step that hands the user a script to run and picks up its result itself
/// (DEC-2026-10-16-commands-route). The driver writes the script into a directory of its own
/// ([`capture::Inbox`]), puts on screen one short line that runs it, and sends the value the
/// script hands back there as [`Event::Answer`]; the user may type the value instead, for a
/// script run on another machine.
#[derive(Debug)]
pub struct Run {
    pub when: &'static [When],
    /// The question whose answer the result gives. It is not shown while this step is: this
    /// step asks it, and its `clean` checks the value.
    pub fills: &'static str,
    /// The script to run, in POSIX sh, from the answers so far (by step id). `{result}` stands
    /// for the directory the result goes to, quoted for sh: see [`capture`] for what to write.
    pub script: fn(&AnswerOf<'_>) -> String,
    /// The prompt of the line where the value can be typed instead.
    pub prompt: &'static str,
    /// What to say when the value typed there is not one `fills` takes: where the command
    /// shows it, as the question's own advice may point elsewhere.
    pub wrong: &'static str,
    /// What a failed run means, from the stage it failed at and what it said: what to do next.
    pub failure: fn(&str, &str) -> String,
}

/// What Next names at a [`Case`] the setup stops at.
pub const STOPS_HERE: &str = "the setup ends here, with what to do instead";

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
    /// The fixed answers, picked from a list on a terminal; empty for free text. `clean` still
    /// takes each one's value, so the flag accepts the same.
    pub choices: &'static [Choice],
    /// Tidies an answer, or says what to change.
    pub clean: fn(&str) -> Result<Cleaned, &'static str>,
}

/// One fixed answer of a [`Question`].
#[derive(Debug)]
pub struct Choice {
    pub value: &'static str,
    pub label: &'static str,
}

/// What a line of a step's text is, so each looks different.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line {
    /// An instruction or an explanation.
    Say,
    /// Something the user does.
    Action,
    /// A side note.
    Note,
    Warning,
}

/// `text` with its first letter capitalised, as the start of a line it now is.
pub(crate) fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// A side note opens with `Note: `, or with `Why` and up to three words before a colon
/// (`Why: `, `Why External: `).
fn is_note(line: &str) -> bool {
    if line.starts_with("Note: ") {
        return true;
    }
    let Some(rest) = line.strip_prefix("Why") else {
        return false;
    };
    rest.split_once(": ")
        .is_some_and(|(words, _)| words.split_whitespace().count() <= 3 && !words.contains('.'))
}

impl Kind {
    /// The question a [`Kind::Ask`] or [`Kind::AskIf`] asks.
    pub fn question(&self) -> Option<&Question> {
        match self {
            Kind::Ask(q) | Kind::AskIf(_, q) => Some(q),
            _ => None,
        }
    }
}

impl Step {
    /// The step's text as paragraphs, each with its kind and its lines joined: the markers of
    /// [`Step::why`] decide the kind, and `Warning: ` and `- ` are dropped, since the screen
    /// shows a sign for them.
    pub fn lines(&self) -> Vec<(Line, String)> {
        let mut out: Vec<(Line, String)> = Vec::new();
        let mut open = false;
        for raw in self.why.lines() {
            let line = raw.trim();
            if line.is_empty() {
                open = false;
                continue;
            }
            let marked = if let Some(rest) = line.strip_prefix("- ") {
                Some((Line::Action, rest))
            } else if let Some(rest) = line.strip_prefix("Warning: ") {
                Some((Line::Warning, rest))
            } else if is_note(line) {
                Some((Line::Note, line))
            } else {
                None
            };
            match (marked, out.last_mut()) {
                (Some((kind, text)), _) => out.push((kind, capitalised(text))),
                // An action goes on only on indented lines, as a list item would.
                (None, Some((kind, text)))
                    if open && (*kind != Line::Action || raw.starts_with("  ")) =>
                {
                    text.push(' ');
                    text.push_str(line);
                }
                (None, _) => out.push((Line::Say, line.to_owned())),
            }
            open = true;
        }
        out
    }
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
    /// [`Step::page`] with its placeholder filled, as in `links`.
    pub page: Option<String>,
    /// Where the step is among those the user sees (1-based), never counting checks.
    pub seen: usize,
    pub seen_total: usize,
    /// The steps the user sees, in order, and how far each is.
    pub outline: Vec<(&'static str, Place)>,
    /// At a [`Kind::Run`]: the script to write, with `{result}` still in it.
    pub script: Option<String>,
    /// At a [`Kind::Run`]: the answer its question has now, from an earlier result or typing.
    pub filled: Option<String>,
}

/// How far a step of the outline is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Done,
    Now,
    Left,
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
    /// The answers offered from the last run, for the path ([`Session::offer`]).
    offered: Vec<Option<String>>,
    /// Console steps passed before a failed check asked an earlier step again: not shown again
    /// on the way back to the check.
    passed: Vec<bool>,
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
            let Some(q) = step.kind.question() else {
                continue;
            };
            let Some(name) = q.flag else {
                // A question only a terminal asks, such as a choice of route: nothing can be
                // asked, so its default answers it, as when a flag is left out.
                if let (false, false, Some(d)) = (interactive, q.secret, q.default) {
                    answers[i] = Some(Answer::Text(d.to_owned()));
                    given[i] = true;
                }
                continue;
            };
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
            offered: vec![None; n],
            passed: vec![false; n],
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

    /// Moves past the steps that are not shown: answered (by a flag or earlier), Console and
    /// Gate steps when nothing can be asked, those passed before a failed check asked an earlier
    /// step again, and a [`Kind::ConsoleIf`] whose answer does not apply.
    fn settle(&mut self) {
        while self.at < self.wizard.steps.len() && self.skipped(self.at) {
            self.at += 1;
        }
    }

    /// Whether step `i` is passed without showing or running it, on the answers given so far.
    fn skipped(&self, i: usize) -> bool {
        match self.wizard.steps[i].kind {
            // An answer kept from earlier: after a failed check asks an earlier step
            // again, the steps between keep theirs. Back still shows them.
            Kind::Ask(_) => self.given[i] || self.answers[i].is_some(),
            Kind::AskIf(..) => self.given[i] || self.answers[i].is_some() || !self.shows(i, None),
            Kind::Run(_) => !self.shows(i, None),
            Kind::Check { .. } | Kind::Commit => false,
            _ => !self.shows(i, None) || self.passed[i],
        }
    }

    /// Whether step `i` is one the user sees when the current step is answered with `pending`:
    /// asked or shown, not answered by a flag, and, for a [`Kind::ConsoleIf`], one whose answer
    /// applies. Checks are never seen.
    fn shows(&self, i: usize, pending: Option<&str>) -> bool {
        match self.wizard.steps[i].kind {
            Kind::Console | Kind::Gate(_) => self.interactive,
            Kind::ConsoleIf(when) => self.holds(when, pending),
            Kind::Ask(_) => !self.given[i] && self.filler(i, pending).is_none(),
            Kind::AskIf(when, _) => !self.given[i] && self.holds(when, pending),
            Kind::Run(ref run) => {
                self.holds(run.when, pending)
                    && self
                        .wizard
                        .position(run.fills)
                        .is_some_and(|f| !self.given[f])
            }
            Kind::Check { .. } | Kind::Commit => false,
        }
    }

    /// Whether every condition in `when` holds, on a terminal: each question's answer, or the
    /// one expected with `pending` at the current step, is one it names.
    fn holds(&self, when: &[When], pending: Option<&str>) -> bool {
        self.interactive && self.matches(when, pending)
    }

    /// Whether the answers given or expected meet every condition in `when`.
    fn matches(&self, when: &[When], pending: Option<&str>) -> bool {
        when.iter().all(|w| {
            self.expected(w.step, pending)
                .is_some_and(|v| w.is.contains(&v.as_str()))
        })
    }

    /// Where a mistake naming step `i` sends the user on the route their answers take, on a
    /// terminal or not: the step, the [`Kind::Run`] that answers it, or `None` when it is not
    /// on that route.
    fn on_route(&self, i: usize) -> Option<usize> {
        let id = self.wizard.steps[i].id;
        let when = |kind: &Kind| match kind {
            Kind::ConsoleIf(when) | Kind::AskIf(when, _) | Kind::Run(Run { when, .. }) => {
                Some(*when)
            }
            _ => None,
        };
        let applies =
            |j: usize| when(&self.wizard.steps[j].kind).is_none_or(|w| self.matches(w, None));
        let filler = self.wizard.steps.iter().position(|s| match s.kind {
            Kind::Run(ref run) => run.fills == id,
            _ => false,
        });
        match filler {
            Some(r) if !self.given[i] && applies(r) => Some(r),
            _ => applies(i).then_some(i),
        }
    }

    /// The [`Kind::Run`] step the user sees that answers question `i`, if any.
    fn filler(&self, i: usize, pending: Option<&str>) -> Option<usize> {
        let id = self.wizard.steps[i].id;
        self.wizard
            .steps
            .iter()
            .position(|s| match s.kind {
                Kind::Run(ref run) => run.fills == id,
                _ => false,
            })
            .filter(|&r| self.shows(r, pending))
    }

    /// The answer question `id` has or is expected to get: `pending` at the current step, else
    /// the one given, else the one offered from the last run, its default, or a list's first
    /// item, which is what its screen starts with.
    fn expected(&self, id: &str, pending: Option<&str>) -> Option<String> {
        let i = self.wizard.position(id)?;
        let q = self.wizard.steps[i].kind.question()?;
        if i == self.at
            && let Some(p) = pending
        {
            return Some((q.clean)(p).map_or_else(|_| p.trim().to_owned(), |c| c.value));
        }
        if let Some(Answer::Text(t)) = &self.answers[i] {
            return Some(t.clone());
        }
        self.offered[i]
            .as_deref()
            .or(q.default)
            .or(q.choices.first().map(|c| c.value))
            .map(str::to_owned)
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

    /// The path the user will take from step `at` when it is answered with `pending`: the steps
    /// they see, in order, and how far each is. A later step that keeps its answer, or that was
    /// passed before a failed check asked an earlier one again, is done: it is not shown again. The header, the outline, Next and
    /// the welcome screen all come from it.
    pub fn path(&self, at: usize, pending: Option<&str>) -> Vec<(usize, Place)> {
        let stops = self.stops(at, pending);
        (0..self.wizard.steps.len())
            .filter(|&i| self.shows(i, pending) && !(stops && i > at))
            .map(|i| {
                let place = match i.cmp(&at) {
                    std::cmp::Ordering::Less => Place::Done,
                    std::cmp::Ordering::Equal => Place::Now,
                    // Passed on the way back to a failed check: kept, so not shown again.
                    _ if self.passed[i] || self.answers[i].is_some() => Place::Done,
                    std::cmp::Ordering::Greater => Place::Left,
                };
                (i, place)
            })
            .collect()
    }

    /// Whether step `at` is a [`Kind::Gate`] and `pending`, the index of a case, is one the setup
    /// stops at: no step follows it.
    fn stops(&self, at: usize, pending: Option<&str>) -> bool {
        let Some(Kind::Gate(cases)) = self.wizard.steps.get(at).map(|s| &s.kind) else {
            return false;
        };
        pending
            .and_then(|p| p.parse::<usize>().ok())
            .and_then(|i| cases.get(i))
            .is_some_and(|c| c.stop.is_some())
    }

    /// Whether the steps the user sees depend on an answer not given yet: the welcome screen
    /// then says its count is for the suggested answers.
    pub fn depends_on_answers(&self) -> bool {
        self.interactive
            && self.wizard.steps.iter().any(|s| match s.kind {
                Kind::ConsoleIf(when) | Kind::AskIf(when, _) | Kind::Run(Run { when, .. }) => {
                    when.iter().any(|w| self.text(w.step).is_none())
                }
                _ => false,
            })
    }

    /// Offers `value` as question `id`'s answer, as the screen does (an answer of the last
    /// run): the path is predicted with it until the question is answered.
    pub fn offer(&mut self, id: &str, value: &str) {
        if let Some(i) = self.wizard.position(id) {
            self.offered[i] = Some(value.to_owned());
        }
    }

    /// The answers typed so far that are no secret and came from no flag, by step id: what a
    /// second run offers again.
    pub fn typed(&self) -> Vec<(&'static str, &str)> {
        self.wizard
            .steps
            .iter()
            .zip(&self.answers)
            .zip(&self.given)
            .filter_map(|((step, answer), given)| match answer {
                Some(Answer::Text(t)) if !given => Some((step.id, t.as_str())),
                _ => None,
            })
            .collect()
    }

    pub fn wizard(&self) -> &'static Wizard {
        self.wizard
    }

    /// The current step's view when it is answered with `pending`, the item pointed at or the
    /// text typed so far, or at a [`Kind::Gate`] the index of the case pointed at: the outline,
    /// the count and Next follow that answer.
    pub fn preview(&self, pending: &str) -> View<'_> {
        self.view(self.at, Some(pending))
    }

    fn view(&self, at: usize, pending: Option<&str>) -> View<'_> {
        let steps = self.wizard.steps;
        let path = self.path(at, pending);
        let seen_total = path.len();
        let seen = path.iter().filter(|(i, _)| *i <= at).count().max(1);
        let stops = self.stops(at, pending);
        let next = path
            .iter()
            .find(|(i, place)| *i > at && *place == Place::Left)
            .map(|(i, _)| steps[*i].title)
            .or_else(|| match steps[at].kind {
                // A case the setup stops at leads to its advice, not to the save.
                _ if stops => Some(STOPS_HERE),
                // After the last step the user sees, the save's own screen comes.
                Kind::Commit => None,
                _ => steps
                    .iter()
                    .find(|s| matches!(s.kind, Kind::Commit))
                    .map(|s| s.title),
            });
        let (script, filled) = match steps[at].kind {
            Kind::Run(ref run) => (
                Some((run.script)(&|id| self.text(id).map(str::to_owned))),
                self.text(run.fills).map(str::to_owned),
            ),
            _ => (None, None),
        };
        View {
            script,
            filled,
            page: steps[at].page.map(|p| self.link(p)),
            seen,
            seen_total,
            outline: path
                .into_iter()
                .map(|(i, place)| (steps[i].title, place))
                .collect(),
            number: at + 1,
            total: steps.len(),
            step: &steps[at],
            links: steps[at].links.iter().map(|l| self.link(l)).collect(),
            next,
            fix: self.fix.as_deref(),
            warning: self.warning,
            can_go_back: !self.shown.is_empty(),
        }
    }

    /// What to do now.
    pub fn prompt(&self) -> Prompt<'_> {
        match &self.end {
            Some(End::Stopped(advice)) => return Prompt::Stopped(advice),
            Some(End::Failed(at, fix)) => return Prompt::Failed(self.view(*at, None), fix.clone()),
            Some(End::Done) => return Prompt::Done,
            None => {}
        }
        let Some(step) = self.wizard.steps.get(self.at) else {
            return Prompt::Done;
        };
        match step.kind {
            Kind::Check { reads } => Prompt::Check(
                self.view(self.at, None),
                Answers {
                    session: self,
                    reads: Some(reads),
                },
            ),
            Kind::Commit => Prompt::Commit(Answers {
                session: self,
                reads: None,
            }),
            _ => Prompt::Show(self.view(self.at, None)),
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
            (Kind::Console | Kind::ConsoleIf(_), Event::Continue) => self.advance(),
            (Kind::Gate(cases), Event::Choose(i)) => match cases.get(i).map(|c| c.stop) {
                Some(None) => self.advance(),
                Some(Some(advice)) => self.end = Some(End::Stopped(advice)),
                None => {}
            },
            (Kind::Run(run), Event::Answer(text)) => {
                let Some(f) = self.wizard.position(run.fills) else {
                    return;
                };
                let Some(q) = self.wizard.steps[f].kind.question() else {
                    return;
                };
                match (q.clean)(&text) {
                    Ok(cleaned) => {
                        self.answers[f] = Some(Answer::Text(cleaned.value));
                        self.warning = cleaned.warning;
                        self.advance();
                    }
                    Err(_) => self.fix = Some(run.wrong.to_owned()),
                }
            }
            (Kind::Ask(q) | Kind::AskIf(_, q), Event::Answer(text)) => match (q.clean)(&text) {
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
        // The first mistake of this kind whose step is on the user's path (a step of the route
        // taken, or the [`Kind::Run`] that answers a question for it), else the first.
        let mistakes = || {
            self.wizard
                .mistakes
                .iter()
                .filter(|m| m.check == check && m.kind == e.kind)
                .filter_map(|m| Some((self.wizard.position(m.step)?, m)))
        };
        let found = mistakes()
            .find_map(|(i, m)| Some((self.on_route(i)?, m)))
            .or_else(|| mistakes().next());
        let (target, fix) = match found {
            Some((target, m)) => (target, format!("{}\n{}", e.detail, m.fix)),
            None => (self.at, e.detail),
        };
        let can_ask_again = self.interactive && target < self.at && !self.given[target];
        if can_ask_again {
            for &s in self.shown.iter().filter(|&&s| s > target) {
                self.passed[s] = !matches!(
                    self.wizard.steps[s].kind,
                    Kind::Ask(_) | Kind::AskIf(..) | Kind::Run(_)
                );
            }
            self.shown.retain(|&s| s < target);
            // A command's step keeps the answer it gave, to offer again.
            if !matches!(self.wizard.steps[target].kind, Kind::Run(_)) {
                self.answers[target] = None;
            }
            self.at = target;
            self.warning = None;
            self.fix = Some(fix);
        } else {
            self.end = Some(End::Failed(target, fix));
        }
    }
}
