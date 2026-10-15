//! Shows a [`Session`] on a terminal: a welcome screen, then one step per screen with a header
//! and the steps done and left, lists picked with the arrow keys or by typing, answers checked
//! on the spot, each check as it runs, a yes or no before the save, and a closing summary. Off a
//! terminal it prints only what the checks and the save say, and reads secrets from standard
//! input, as before. Everything goes through [`Term`], so the tests press keys and read the
//! screens; checks and the write are the caller's (DEC-2026-10-15-wizard-ui).

use anyhow::{Result, bail};

use super::remember::Memory;
use super::screen::{self, Look};
use super::term::{Frame, Key, Term, Tty};
use super::{Answers, CheckError, Event, Kind, Line, Place, Prompt, Session, View};
use crate::style::{self, Paint, Symbols};

/// What the save did: shown in the closing summary on a terminal.
#[derive(Debug, Default)]
pub struct Saved {
    /// What went where: "the app password: GNOME Keyring (service \"neomail\", ...)".
    pub saved: Vec<String>,
    /// The command to run next, such as `neomail sync --account work`.
    pub next: Option<String>,
    /// What is left to do or know; said off a terminal too.
    pub said: String,
    /// What is left to do by hand, in the summary on a terminal only.
    pub todo: Vec<String>,
}

/// What a check returns when a flag (`--no-check`) skipped it: shown as skipped, never as
/// passed, and said nowhere off a terminal.
pub const SKIPPED: &str = "\u{0}skipped";

/// The [`CheckError`] kind of a check whose question the user quit (Ctrl-C or Esc at
/// [`Ask::confirm`]): the run stops there, as a quit at a step does.
pub const QUIT: &str = "quit";

/// The wizard's side of the terminal: the seam, how screens look, and the pages it may open.
pub struct Ui<'a> {
    pub term: &'a mut dyn Term,
    pub look: Look,
    /// Opens a page in the browser and returns the program's name ([`crate::browser::open`]).
    pub opener: &'a dyn Fn(&str) -> Result<String>,
    /// On a terminal, without `--no-browser`, and not over SSH without a display.
    pub open_pages: bool,
    pub memory: Memory,
    /// What the checks said since the last screen, shown on the next one.
    recent: Vec<String>,
    /// Every check passed, for the summary.
    passed: Vec<String>,
    /// The steps whose page was opened or offered.
    opened: Vec<&'static str>,
    /// The last text typed at each step, kept for editing when it is asked again.
    kept: Vec<(&'static str, String)>,
    /// The header of a list on its own screen ([`choose`]), in place of a step's.
    menu_header: Option<String>,
}

/// Opens nothing: for a [`Ui`] that may not open pages.
#[cfg(test)]
pub fn no_opener(_: &str) -> Result<String> {
    bail!("pages are not opened here")
}

/// Whether pages may open in a browser: on a terminal, without `--no-browser`, and not over SSH
/// without a display, where the browser would not reach the user. `env` reads a variable.
pub fn may_open_pages(
    interactive: bool,
    no_browser: bool,
    env: impl Fn(&str) -> Option<String>,
) -> bool {
    interactive && !no_browser && !crate::browser::ssh_without_display(env)
}

impl<'a> Ui<'a> {
    pub fn new(
        term: &'a mut dyn Term,
        look: Look,
        opener: &'a dyn Fn(&str) -> Result<String>,
        open_pages: bool,
        memory: Memory,
    ) -> Self {
        let open_pages = open_pages && term.interactive();
        Self {
            term,
            look,
            opener,
            open_pages,
            memory,
            recent: Vec::new(),
            passed: Vec::new(),
            opened: Vec::new(),
            kept: Vec::new(),
            menu_header: None,
        }
    }

    /// The real terminal for wizard `id`: colour as main.rs decided for standard error, the
    /// locale's symbols, pages opened unless `no_browser` or over SSH without a display, and the
    /// answers of an unfinished run.
    pub fn real(tty: &'a mut Tty, paint: Paint, no_browser: bool, id: &str) -> Self {
        let interactive = tty.interactive();
        let look = Look {
            paint,
            sym: Symbols::for_locale(style::env_var),
            hyperlinks: interactive,
        };
        let open = may_open_pages(interactive, no_browser, style::env_var);
        let memory = match crate::config::data_dir() {
            Ok(dir) if interactive => Memory::load(&dir, id),
            _ => Memory::none(),
        };
        Self::new(tty, look, &crate::browser::open, open, memory)
    }

    pub fn interactive(&self) -> bool {
        self.term.interactive()
    }

    fn sym(&self) -> Symbols {
        self.look.sym
    }

    fn paint(&self) -> Paint {
        self.look.paint
    }

    /// `text` wrapped and styled as a paragraph after `first` (a sign, or the indent), with the
    /// lines after it under the text.
    fn paragraph(
        &self,
        first: &str,
        text: &str,
        cols: usize,
        role: fn(Paint, &str) -> String,
    ) -> Vec<String> {
        let indent = screen::width(first);
        let lines = screen::wrap(text, screen::measure(cols, indent));
        let styled = screen::style(&self.look, &lines);
        styled
            .into_iter()
            .enumerate()
            .map(|(i, l)| {
                let lead = if i == 0 {
                    first.to_owned()
                } else {
                    " ".repeat(indent)
                };
                format!("{lead}{}", role(self.paint(), &l))
            })
            .collect()
    }

    /// The keys a screen takes, named in its footer: ("Enter", "continue"), ...
    fn footer(&self, keys: &[(&str, &str)]) -> String {
        let dot = format!(" {} ", self.sym().dot);
        let parts: Vec<String> = keys
            .iter()
            .map(|(key, what)| format!("{} {what}", self.paint().literal(key)))
            .collect();
        format!("  {}", parts.join(&dot))
    }
}

impl Ui<'_> {
    /// A list item after `lead`: a label with a name column ("gmail   Gmail, through ...")
    /// keeps the column, and its description wraps under itself.
    fn item(
        &self,
        lead: &str,
        label: &str,
        cols: usize,
        role: fn(Paint, &str) -> String,
    ) -> Vec<String> {
        let Some((name, rest)) = label.split_once("  ") else {
            return self.paragraph(lead, label, cols, role);
        };
        let about = rest.trim_start();
        let pad = 2 + rest.len() - about.len();
        let first = format!("{lead}{}{}", role(self.paint(), name), " ".repeat(pad));
        let mut lines = self.paragraph(&first, about, cols, role);
        let indent = " ".repeat(screen::width(&first));
        for line in lines.iter_mut().skip(1) {
            if let Some(tail) = line.strip_prefix(&indent) {
                *line = format!("{indent}{tail}");
            }
        }
        lines
    }
}

/// Raw mode is left whatever happens: success, an error, a quit, or a panic that unwinds.
impl Drop for Ui<'_> {
    fn drop(&mut self) {
        let _ = self.term.raw(false);
    }
}

/// Keeps a role's text as it is: for plain paragraphs.
fn as_is(_: Paint, text: &str) -> String {
    text.to_owned()
}

/// What a step screen waits for, under the step's text.
enum Input<'i> {
    /// Enter, at a step that shows a page or advice.
    Keys,
    /// A list: the items (label, and what narrowing also matches), the one pointed at (an index
    /// into the narrowed list) and the text typed to narrow it.
    Pick(&'i [(String, String)], usize, &'i str),
    /// A line: its prompt, the text so far and the cursor, whether it is hidden, and what Enter
    /// keeps when nothing is typed.
    Line {
        prompt: &'i str,
        text: &'i [char],
        cursor: usize,
        hidden: bool,
        keeps: Option<&'i str>,
    },
}

/// The items of `items` that `filter` matches, best first: exact, then by prefix, then
/// anywhere, each in list order. Case is ignored.
pub fn narrow(items: &[(String, String)], filter: &str) -> Vec<usize> {
    let f = filter.trim().to_lowercase();
    if f.is_empty() {
        return (0..items.len()).collect();
    }
    let mut ranked: Vec<(u8, usize)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, (label, also))| {
            let label = label.to_lowercase();
            let also = also.to_lowercase();
            let rank = if also == f || label == f {
                0
            } else if also.starts_with(&f) || label.starts_with(&f) {
                1
            } else if also.contains(&f) || label.contains(&f) {
                2
            } else {
                return None;
            };
            Some((rank, i))
        })
        .collect();
    ranked.sort();
    ranked.into_iter().map(|(_, i)| i).collect()
}

/// The rows a screen keeps for its essentials besides the input: the header, a blank, the
/// footer's blank and the footer.
const FRAME_ROWS: usize = 4;

impl Ui<'_> {
    /// The header: where the user is, "▸ Step 2/9 · Name the account".
    fn header(&self, view: &View<'_>) -> String {
        if let Some(header) = &self.menu_header {
            return header.clone();
        }
        let s = self.sym();
        self.paint().heading(&format!(
            "{} Step {}/{} {} {}",
            s.arrow, view.seen, view.seen_total, s.dot, view.step.title
        ))
    }

    /// The steps done, the current one and those left: one per line when `roomy`, else one line.
    fn outline(&self, view: &View<'_>, roomy: bool, cols: usize) -> Vec<String> {
        let s = self.sym();
        let p = self.paint();
        if view.outline.is_empty() {
            return Vec::new();
        }
        if roomy {
            return view
                .outline
                .iter()
                .map(|(title, place)| match place {
                    Place::Done => p.note(&format!("  {} {title}", s.check)),
                    Place::Now => p.heading(&format!("  {} {title}", s.arrow)),
                    Place::Left => format!("  {} {title}", s.dot),
                })
                .collect();
        }
        let done = view
            .outline
            .iter()
            .filter(|(_, at)| *at == Place::Done)
            .count();
        let left = view
            .outline
            .iter()
            .filter(|(_, at)| *at == Place::Left)
            .count();
        let next = view
            .next
            .map(|t| format!(": next, {t}"))
            .unwrap_or_default();
        let line = format!("  {} {done} done  {} {left} left{next}", s.check, s.dot);
        let line = if screen::width(&line) > cols {
            format!("  {} {done} done  {} {left} left", s.check, s.dot)
        } else {
            line
        };
        vec![p.note(&line)]
    }

    /// The step's own text: paragraphs by kind, then its links. `notes` false leaves out the side
    /// notes and the extra links, for a short screen.
    fn body(&self, view: &View<'_>, cols: usize, notes: bool, page_line: &[String]) -> Vec<String> {
        let s = self.sym();
        let mut out = Vec::new();
        for (kind, text) in view.step.lines() {
            let lines = match kind {
                Line::Say => self.paragraph("  ", &text, cols, as_is),
                Line::Action => {
                    let bullet = format!("  {} ", self.paint().literal(s.bullet));
                    self.paragraph(&bullet, &text, cols, Paint::action)
                }
                Line::Note if notes => self.paragraph("  ", &text, cols, Paint::note),
                Line::Note => continue,
                Line::Warning => {
                    let sign = format!("  {} ", self.paint().warning(s.warning));
                    self.paragraph(&sign, &text, cols, Paint::warning)
                }
            };
            out.extend(lines);
        }
        out.extend(page_line.iter().cloned());
        if notes {
            let others = view.links.iter().filter(|l| Some(*l) != view.page.as_ref());
            for link in others {
                out.push(format!(
                    "  {} {}",
                    self.paint().note("See"),
                    screen::link(&self.look, link)
                ));
            }
        }
        out
    }

    /// The input lines, and the cursor's row in them and column, if it shows. `rows` is how many
    /// a list may take.
    fn input(
        &self,
        input: &Input<'_>,
        cols: usize,
        rows: usize,
    ) -> (Vec<String>, Option<(usize, usize)>) {
        let s = self.sym();
        let p = self.paint();
        match input {
            Input::Keys => (Vec::new(), None),
            Input::Line {
                prompt,
                text,
                cursor,
                hidden,
                keeps,
            } => {
                let prompt = prompt.trim_end();
                let head = format!("  {} {prompt} ", p.literal("?"));
                let shown: String = if *hidden {
                    String::new()
                } else {
                    text.iter().collect()
                };
                let mut lines = vec![format!("{head}{shown}")];
                let col = screen::width(&head)
                    + if *hidden {
                        0
                    } else {
                        screen::width(&text[..*cursor].iter().collect::<String>())
                    };
                let hint = match keeps {
                    _ if *hidden => {
                        Some("Typing is hidden: nothing shows, and Enter ends it.".to_owned())
                    }
                    Some("") if text.is_empty() => {
                        Some("Enter with nothing typed skips this.".to_owned())
                    }
                    Some(v) if text.is_empty() => {
                        Some(format!("Enter keeps {v}; Tab fills it in to edit."))
                    }
                    _ => None,
                };
                if let Some(hint) = hint {
                    lines.extend(self.paragraph("    ", &hint, cols, Paint::note));
                }
                (lines, Some((0, col)))
            }
            Input::Pick(items, at, filter) => {
                let matches = narrow(items, filter);
                let mut lines = vec![match filter.is_empty() {
                    true => format!(
                        "  {} {}",
                        p.literal("?"),
                        p.note("Pick one, or type to narrow the list:")
                    ),
                    false => format!("  {} {} {filter}", p.literal("?"), p.note("Narrowed by:")),
                }];
                if matches.is_empty() {
                    lines.push(format!(
                        "    {}",
                        p.failure(&format!(
                            "{} Nothing matches: Backspace to change it.",
                            s.cross
                        ))
                    ));
                }
                let mut rendered: Vec<(usize, Vec<String>)> = Vec::new();
                for (n, &i) in matches.iter().enumerate() {
                    let here = n == *at;
                    let lead = if here {
                        format!("  {} ", p.literal(s.pointer))
                    } else {
                        "    ".to_owned()
                    };
                    let role = if here { Paint::literal } else { as_is };
                    rendered.push((n, self.item(&lead, &items[i].0, cols, role)));
                }
                // A window of items around the one pointed at, when they do not all fit.
                let room = rows.saturating_sub(lines.len()).max(3);
                let total: usize = rendered.iter().map(|(_, l)| l.len()).sum();
                let (mut first, mut last) = (0, rendered.len());
                if total > room {
                    first = *at;
                    last = at + 1;
                    let mut used = rendered.get(*at).map_or(0, |(_, l)| l.len());
                    while used < room.saturating_sub(2) {
                        let grew_down =
                            last < rendered.len() && used + rendered[last].1.len() <= room - 2;
                        if grew_down {
                            used += rendered[last].1.len();
                            last += 1;
                        }
                        let grew_up = first > 0 && used + rendered[first - 1].1.len() <= room - 2;
                        if grew_up {
                            first -= 1;
                            used += rendered[first].1.len();
                        }
                        if !grew_down && !grew_up {
                            break;
                        }
                    }
                }
                if first > 0 {
                    lines.push(p.note(&format!("    {} {first} more", s.up)));
                }
                for (_, l) in &rendered[first..last] {
                    lines.extend(l.iter().cloned());
                }
                if last < rendered.len() {
                    lines.push(p.note(&format!("    {} {} more", s.down, rendered.len() - last)));
                }
                (lines, None)
            }
        }
    }
}

/// What the user did at a screen.
enum Did {
    Event(Event),
    Quit,
}

impl Ui<'_> {
    /// A step's screen at the terminal's size: the header, the outline, what the checks said,
    /// the step's text, the input, a wrong answer next to it, and the keys. When it does not
    /// fit, the outline goes on one line, then the side notes go, then the text is cut, so the
    /// header, the action, the prompt and the footer stay on screen.
    fn step_frame(
        &self,
        view: &View<'_>,
        input: &Input<'_>,
        error: &[String],
        page_line: &[String],
        keys: &[(&str, &str)],
    ) -> Frame {
        let (cols, rows) = self.term.size();
        let (cols, rows) = (usize::from(cols).max(20), usize::from(rows).max(8));
        let header = self.header(view);
        let footer = self.footer(keys);
        let mut fixed = FRAME_ROWS + self.recent.len() + error.len() + 1;
        let (list, _) = self.input(input, cols, rows);
        let mut layout = None;
        for (roomy, notes) in [(true, true), (false, true), (false, false)] {
            let outline = self.outline(view, roomy, cols);
            let body = self.body(view, cols, notes, page_line);
            let need = fixed + outline.len() + screen::rows(&body, cols) + list.len();
            let last = !notes;
            if need <= rows || last {
                layout = Some((outline, body));
                break;
            }
        }
        let (outline, mut body) = layout.unwrap_or_default();
        fixed += outline.len();
        let avail = rows.saturating_sub(fixed);
        let list_rows = match input {
            Input::Pick(..) => list.len().min(avail.saturating_sub(body.len()).max(6)),
            _ => list.len(),
        };
        let (input_lines, cursor) = self.input(input, cols, list_rows);
        let room = avail.saturating_sub(input_lines.len());
        if screen::rows(&body, cols) > room {
            let mut keep = body.len();
            while keep > 0 && screen::rows(&body[..keep], cols) + 1 > room {
                keep -= 1;
            }
            let cut = body.len() - keep;
            body.truncate(keep);
            body.push(self.paint().note(&format!(
                "  {} {cut} more lines: a taller window shows them",
                self.sym().down
            )));
        }
        let mut lines = vec![header];
        lines.extend(outline);
        lines.push(String::new());
        let said = !self.recent.is_empty() || !body.is_empty();
        lines.extend(self.recent.iter().cloned());
        lines.extend(body);
        if said {
            lines.push(String::new());
        }
        let at = screen::rows(&lines, cols);
        let asks = !input_lines.is_empty() || !error.is_empty();
        lines.extend(input_lines);
        lines.extend(error.iter().cloned());
        if asks || !said {
            lines.push(String::new());
        }
        lines.push(footer);
        Frame {
            lines,
            cursor: cursor.map(|(row, col)| (at + row, col)),
        }
    }

    /// The line that says what happened to the step's page: opened now, or to open yourself.
    fn page_line(&mut self, view: &View<'_>, cols: usize) -> Vec<String> {
        let Some(url) = view.page.clone() else {
            return Vec::new();
        };
        let s = self.sym();
        let first = !self.opened.contains(&view.step.id);
        if first {
            self.opened.push(view.step.id);
        }
        let link = screen::link(&self.look, &url);
        if !self.open_pages {
            return vec![format!(
                "  {} {link}",
                self.paint().action("Open it yourself:")
            )];
        }
        if !first {
            return vec![format!(
                "  {} {} {link}",
                self.paint().success(s.open),
                "opened"
            )];
        }
        match (self.opener)(&url) {
            Ok(_) => vec![format!(
                "  {} {} {link}",
                self.paint().success(s.open),
                "opening"
            )],
            Err(e) => {
                let mut out = self.paragraph(
                    &format!("  {} ", self.paint().warning(s.warning)),
                    &format!("No browser opened ({e:#})."),
                    cols,
                    Paint::warning,
                );
                out.push(format!(
                    "  {} {link}",
                    self.paint().action("Open it yourself:")
                ));
                out
            }
        }
    }

    /// A wrong answer or a failed check, next to the input: a cross and what to change.
    fn error_lines(&self, fix: Option<&str>, cols: usize) -> Vec<String> {
        let Some(fix) = fix else { return Vec::new() };
        // A line that ends mid-sentence was wrapped by hand: it goes on in the next.
        let mut parts: Vec<String> = Vec::new();
        for line in fix.lines().map(str::trim).filter(|l| !l.is_empty()) {
            match parts.last_mut() {
                Some(last) if !last.ends_with(['.', ':', '?', '!', ')']) => {
                    last.push(' ');
                    last.push_str(line);
                }
                _ => parts.push(line.to_owned()),
            }
        }
        let mut out = Vec::new();
        for (i, part) in parts.iter().enumerate() {
            let lead = if i == 0 {
                format!("    {} ", self.paint().failure(self.sym().cross))
            } else {
                "      ".to_owned()
            };
            out.extend(self.paragraph(&lead, part, cols, Paint::failure));
        }
        out
    }

    /// Shows a Console, Gate or Ask step and waits for what the user does there.
    fn show(&mut self, view: &View<'_>) -> Result<Did> {
        if !self.interactive() {
            // Off a terminal only a secret is left to read: Console and Gate steps are skipped.
            return match &view.step.kind {
                Kind::Ask(q) => Ok(Did::Event(Event::Answer(self.term.secret_line(q.prompt)?))),
                _ => bail!("step {:?} cannot be shown off a terminal", view.step.id),
            };
        }
        let (cols, _) = self.term.size();
        let cols = usize::from(cols);
        let page = self.page_line(view, cols);
        let error = self.error_lines(view.fix, cols);
        if let Some(w) = view.warning {
            let sign = format!("  {} ", self.paint().warning(self.sym().warning));
            self.recent
                .extend(self.paragraph(&sign, &super::capitalised(w), cols, Paint::warning));
        }
        let back = view.can_go_back;
        let did = match &view.step.kind {
            Kind::Console | Kind::ConsoleIf { .. } => {
                let mut keys = vec![("Enter", "done"), ("s", "skip, already done")];
                if back {
                    keys.push(("Esc", "back"));
                }
                keys.push(("q", "quit"));
                let frame = self.step_frame(view, &Input::Keys, &error, &page, &keys);
                self.wait_keys(&frame, back)?
            }
            Kind::Gate(cases) => {
                let items: Vec<(String, String)> = cases
                    .iter()
                    .enumerate()
                    .map(|(i, c)| (format!("{}. {}", i + 1, c.label), String::new()))
                    .collect();
                match self.pick(view, &items, 0, &error, &page, back)? {
                    Some(Ok(i)) => Did::Event(Event::Choose(i)),
                    Some(Err(())) => Did::Event(Event::Back),
                    None => Did::Quit,
                }
            }
            Kind::Ask(q) if !q.choices.is_empty() => {
                let w = q
                    .choices
                    .iter()
                    .map(|c| screen::width(c.value))
                    .max()
                    .unwrap_or(0);
                let items: Vec<(String, String)> = q
                    .choices
                    .iter()
                    .map(|c| (format!("{:<w$}  {}", c.value, c.label), c.value.to_owned()))
                    .collect();
                let preferred = self
                    .memory
                    .get(view.step.id)
                    .map(str::to_owned)
                    .or(q.default.map(str::to_owned));
                let start = preferred
                    .and_then(|v| q.choices.iter().position(|c| c.value == v))
                    .unwrap_or(0);
                match self.pick(view, &items, start, &error, &page, back)? {
                    Some(Ok(i)) => Did::Event(Event::Answer(q.choices[i].value.to_owned())),
                    Some(Err(())) => Did::Event(Event::Back),
                    None => Did::Quit,
                }
            }
            Kind::Ask(q) => {
                let keeps = match self.memory.get(view.step.id) {
                    Some(v) if !q.secret => Some(v.to_owned()),
                    _ => q.default.map(str::to_owned),
                };
                let kept = match self.kept.iter().find(|(id, _)| *id == view.step.id) {
                    Some((_, text)) if view.fix.is_some() && !q.secret => text.clone(),
                    _ => String::new(),
                };
                self.line(
                    view,
                    q.prompt,
                    q.secret,
                    keeps.as_deref(),
                    &kept,
                    &error,
                    &page,
                    back,
                )?
            }
            Kind::Check { .. } | Kind::Commit => unreachable!("checks and the write are run"),
        };
        self.recent.clear();
        Ok(did)
    }
}

impl Ui<'_> {
    /// Draws `frame` and reads one key in raw mode. Raw mode stays on between keys, so keys
    /// typed ahead never echo; it goes off for a check, at the end, and when the Ui drops.
    fn key(&mut self, frame: &Frame) -> Result<Key> {
        self.term.raw(true)?;
        self.term.draw(frame);
        self.term.key()
    }

    /// A step with nothing to type: Enter or s goes on, Esc goes back, q or Ctrl-C quits.
    fn wait_keys(&mut self, frame: &Frame, back: bool) -> Result<Did> {
        loop {
            match self.key(frame)? {
                Key::Enter | Key::Char('s' | 'S') => return Ok(Did::Event(Event::Continue)),
                Key::Esc | Key::Char('b') if back => return Ok(Did::Event(Event::Back)),
                Key::Char('q' | 'Q') | Key::Interrupt => return Ok(Did::Quit),
                _ => {}
            }
        }
    }

    /// A list: arrows, j and k (before anything is typed), Ctrl-N and Ctrl-P move; a digit
    /// before anything is typed points at that item; other text narrows the list; Enter picks.
    /// `Some(Ok(index))` is the item of `items` picked, `Some(Err(()))` back, `None` quit.
    #[allow(clippy::too_many_arguments)]
    fn pick(
        &mut self,
        view: &View<'_>,
        items: &[(String, String)],
        start: usize,
        error: &[String],
        page: &[String],
        back: bool,
    ) -> Result<Option<Result<usize, ()>>> {
        let mut filter = String::new();
        let mut at = start.min(items.len().saturating_sub(1));
        let arrows = if self.sym().arrow.is_ascii() {
            "Up/Down"
        } else {
            "↑↓"
        };
        let mut keys = vec![(arrows, "move"), ("type", "narrow"), ("Enter", "pick")];
        if back {
            keys.push(("Esc", "back"));
        }
        keys.push(("Ctrl-C", "quit"));
        loop {
            let matches = narrow(items, &filter);
            at = at.min(matches.len().saturating_sub(1));
            let frame = self.step_frame(view, &Input::Pick(items, at, &filter), error, page, &keys);
            match self.key(&frame)? {
                Key::Enter => {
                    if let Some(&i) = matches.get(at) {
                        return Ok(Some(Ok(i)));
                    }
                }
                // j and k move only while nothing is typed; after that they narrow.
                Key::Char('k') if filter.is_empty() => at = at.saturating_sub(1),
                Key::Char('j') if filter.is_empty() => {
                    at = (at + 1).min(matches.len().saturating_sub(1));
                }
                Key::Up => at = at.saturating_sub(1),
                Key::Down => at = (at + 1).min(matches.len().saturating_sub(1)),
                Key::Home | Key::PageUp => at = 0,
                Key::End | Key::PageDown => at = matches.len().saturating_sub(1),
                Key::Char(c) if filter.is_empty() && c.is_ascii_digit() => {
                    let n = c.to_digit(10).unwrap_or(0) as usize;
                    if (1..=items.len()).contains(&n) {
                        at = n - 1;
                    }
                }
                Key::Char(c) => {
                    filter.push(c);
                    at = 0;
                }
                Key::Backspace => {
                    filter.pop();
                    at = 0;
                }
                Key::ClearLine => filter.clear(),
                Key::Esc if !filter.is_empty() => filter.clear(),
                Key::Esc if back => return Ok(Some(Err(()))),
                Key::Interrupt => return Ok(None),
                _ => {}
            }
        }
    }
}

impl Ui<'_> {
    /// A line of text: typed in place, hidden for a secret. Enter with nothing typed takes
    /// `keeps`; Tab fills it in to edit. `kept` is the text typed before a wrong answer.
    #[allow(clippy::too_many_arguments)]
    fn line(
        &mut self,
        view: &View<'_>,
        prompt: &str,
        hidden: bool,
        keeps: Option<&str>,
        kept: &str,
        error: &[String],
        page: &[String],
        back: bool,
    ) -> Result<Did> {
        let mut text: Vec<char> = kept.chars().collect();
        let mut cursor = text.len();
        let mut keys = vec![("Enter", "continue")];
        if keeps.is_some_and(|k| !k.is_empty()) && !hidden {
            keys.push(("Tab", "fill in"));
        }
        if back {
            keys.push(("Esc", "back"));
        }
        keys.push(("Ctrl-C", "quit"));
        loop {
            let input = Input::Line {
                prompt,
                text: &text,
                cursor,
                hidden,
                keeps,
            };
            let frame = self.step_frame(view, &input, error, page, &keys);
            match self.key(&frame)? {
                Key::Enter => {
                    let typed: String = text.iter().collect();
                    let answer = match keeps {
                        Some(k) if typed.trim().is_empty() => k.to_owned(),
                        _ => typed,
                    };
                    if !hidden {
                        self.kept.retain(|(id, _)| *id != view.step.id);
                        self.kept.push((view.step.id, answer.clone()));
                    }
                    return Ok(Did::Event(Event::Answer(answer)));
                }
                Key::Esc if back => return Ok(Did::Event(Event::Back)),
                Key::Interrupt => return Ok(Did::Quit),
                Key::Char(c) if !c.is_control() => {
                    text.insert(cursor, c);
                    cursor += 1;
                }
                Key::Tab => {
                    if let (Some(k), false) = (keeps, hidden) {
                        text = k.chars().collect();
                        cursor = text.len();
                    }
                }
                Key::Backspace if cursor > 0 => {
                    cursor -= 1;
                    text.remove(cursor);
                }
                Key::Delete if cursor < text.len() => {
                    text.remove(cursor);
                }
                Key::Left => cursor = cursor.saturating_sub(1),
                Key::Right => cursor = (cursor + 1).min(text.len()),
                Key::Home => cursor = 0,
                Key::End => cursor = text.len(),
                Key::ClearLine => {
                    text.clear();
                    cursor = 0;
                }
                _ => {}
            }
        }
    }

    /// A screen that asks yes or no: `Some(answer)`, or `None` to quit. Enter takes `yes`.
    fn yes_no(
        &mut self,
        title: &str,
        lines: &[String],
        question: &str,
        yes: bool,
    ) -> Result<Option<bool>> {
        let p = self.paint();
        let hint = if yes { "[Y/n]" } else { "[y/N]" };
        let mut all = vec![
            p.heading(&format!("{} {title}", self.sym().arrow)),
            String::new(),
        ];
        all.extend(self.recent.iter().cloned());
        all.extend(lines.iter().cloned());
        all.push(String::new());
        let ask = format!("  {} {question} {}", p.literal("?"), p.note(hint));
        let at = all.len();
        let col = screen::width(&ask) + 1;
        all.push(ask);
        all.push(String::new());
        all.push(self.footer(&[("y", "yes"), ("n", "no"), ("Ctrl-C", "quit")]));
        let frame = Frame {
            lines: all,
            cursor: Some((at, col)),
        };
        loop {
            match self.key(&frame)? {
                Key::Char('y' | 'Y') => return Ok(Some(true)),
                Key::Char('n' | 'N') => return Ok(Some(false)),
                Key::Enter => return Ok(Some(yes)),
                Key::Interrupt | Key::Esc => return Ok(None),
                _ => {}
            }
        }
    }

    /// The first screen: what will be set up, in how many steps, that quitting keeps the typed
    /// answers, then "Ready to start?". `false` to quit.
    fn welcome(&mut self, session: &Session, view: &View<'_>) -> Result<bool> {
        let (cols, rows) = self.term.size();
        let (cols, rows) = (usize::from(cols), usize::from(rows));
        let wizard = session.wizard();
        let p = self.paint();
        let mut lines = self.paragraph(
            "  ",
            &format!(
                "This sets up {}, in {} steps{}. Nothing is saved until the last one, after every \
                 check has passed.",
                wizard.title,
                view.seen_total,
                match session.may_grow() {
                    true => " or a few more, depending on your answers",
                    false => "",
                }
            ),
            cols,
            as_is,
        );
        if view.outline.len() + 12 <= rows {
            lines.push(String::new());
            for (n, (title, _)) in view.outline.iter().enumerate() {
                lines.push(format!("  {} {title}", p.note(&format!("{:>2}.", n + 1))));
            }
        }
        lines.push(String::new());
        lines.extend(self.paragraph(
            "  ",
            "Quit at any time with Ctrl-C. The answers you typed are kept for the next run, never a \
             password or a secret, and Enter takes each one again.",
            cols,
            Paint::note,
        ));
        if !self.memory.answers.is_empty() {
            let sign = format!("  {} ", p.success(self.sym().check));
            let text = format!(
                "{} answers from your last run are offered again: Enter keeps each one.",
                self.memory.answers.len()
            );
            lines.extend(self.paragraph(&sign, &text, cols, Paint::success));
        }
        let title = format!(
            "neomail account add {} {} {}",
            wizard.id,
            self.sym().dot,
            "Welcome"
        );
        Ok(self.yes_no(&title, &lines, "Ready to start?", true)? == Some(true))
    }
}

/// What a check may ask the user: a yes or no before something that cannot be undone.
pub struct Ask<'u, 'a>(&'u mut Ui<'a>);

impl Ask<'_, '_> {
    /// Asks `question` after `what` explains it; Enter says no. Off a terminal the flags have
    /// decided, so it is yes. `None` is quit.
    pub fn confirm(&mut self, title: &str, what: &str, question: &str) -> Result<Option<bool>> {
        if !self.0.interactive() {
            return Ok(Some(true));
        }
        let (cols, _) = self.0.term.size();
        let sign = format!("  {} ", self.0.paint().warning(self.0.sym().warning));
        let lines = self
            .0
            .paragraph(&sign, what, usize::from(cols), Paint::warning);
        let answer = self.0.yes_no(title, &lines, question, false);
        // The check goes on in cooked mode, as it started: Ctrl-C stops a sign-in that waits,
        // and what it prints lands below this screen.
        let _ = self.0.term.raw(false);
        answer
    }
}

/// Runs `session` to its end: shows each step, runs `check` for each check step and `write` at
/// the end, and says what they return. Fails, with nothing written, when the user stops or
/// quits, or a check fails for good.
pub fn drive(
    session: &mut Session,
    ui: &mut Ui<'_>,
    mut check: impl FnMut(&str, &Answers<'_>, &mut Ask<'_, '_>) -> Result<String, CheckError>,
    mut write: impl FnMut(&Answers<'_>) -> Result<Saved>,
) -> Result<()> {
    let interactive = ui.interactive();
    if interactive && let Prompt::Show(view) = session.prompt() {
        let ready = ui.welcome(session, &view)?;
        if !ready {
            return quit(ui, session, None);
        }
    }
    let mut last_seen: Option<(usize, usize, &'static str)> = None;
    loop {
        let event = match session.prompt() {
            Prompt::Done => return Ok(()),
            Prompt::Stopped(advice) => {
                ui.memory.keep(session).ok();
                bail!("{advice}\nNothing was written.")
            }
            Prompt::Failed(view, fix) => bail!(
                "step {} of {} ({}): {fix}\nNothing was written.",
                view.number,
                view.total,
                view.step.title
            ),
            Prompt::Check(view, answers) => {
                let title = view.step.title;
                if !interactive && let Some(warning) = view.warning {
                    ui.term.say(&format!("Warning: {warning}"));
                }
                if interactive {
                    ui.checking(last_seen, title);
                }
                let result = check(view.step.id, &answers, &mut Ask(ui));
                if matches!(&result, Err(e) if e.kind == QUIT) {
                    return quit(ui, session, Some(&format!("\"{title}\"")));
                }
                ui.checked(title, &result);
                Event::Checked(result.map(drop))
            }
            Prompt::Commit(answers) => {
                if interactive {
                    let step = session.wizard().steps.last().map_or("", |s| s.why);
                    let mut lines = ui.paragraph("  ", step, usize::from(ui.term.size().0), as_is);
                    lines.extend(answer_lines(ui, session));
                    match ui.yes_no("Save the account", &lines, "Save it now?", true)? {
                        Some(true) => {}
                        Some(false) | None => return quit(ui, session, Some("the save")),
                    }
                    ui.term.draw(&Frame {
                        lines: vec![
                            ui.paint().heading(&format!("{} Saving", ui.sym().arrow)),
                            String::new(),
                        ],
                        cursor: Some((2, 0)),
                    });
                    // The save prints its own lines: in cooked mode, below this screen, so the
                    // summary said after them follows them instead of being drawn over them.
                    let _ = ui.term.raw(false);
                }
                let saved = write(&answers)?;
                ui.memory.forget();
                ui.summary(session.wizard().title, &saved);
                Event::Written
            }
            Prompt::Show(view) => {
                last_seen = Some((view.seen, view.seen_total, view.step.title));
                match ui.show(&view)? {
                    Did::Event(e) => e,
                    Did::Quit => {
                        let at = format!("step {} ({})", view.seen, view.step.title);
                        return quit(ui, session, Some(&at));
                    }
                }
            }
        };
        let answered = matches!(event, Event::Answer(_));
        session.next(event);
        if answered && interactive {
            ui.memory.keep(session).ok();
        }
    }
}

/// The typed answers, for the screen before the save.
fn answer_lines(ui: &Ui<'_>, session: &Session) -> Vec<String> {
    let steps = session.wizard().steps;
    session
        .typed()
        .into_iter()
        .filter(|(_, v)| !v.is_empty())
        .filter_map(|(id, v)| {
            let step = steps.iter().find(|s| s.id == id)?;
            Some(format!(
                "  {} {}: {}",
                ui.sym().dot,
                step.title,
                ui.paint().literal(v)
            ))
        })
        .collect()
}

/// The user quit at `at`: the answers typed so far are kept, and nothing was written.
fn quit(ui: &mut Ui<'_>, session: &Session, at: Option<&str>) -> Result<()> {
    let _ = ui.term.raw(false);
    let kept = ui.memory.keep(session).is_ok() && !ui.memory.answers.is_empty();
    let mut text = match at {
        Some(at) => format!("stopped at {at}; nothing was written."),
        None => "stopped; nothing was written.".to_owned(),
    };
    if kept && let Some(path) = ui.memory.path() {
        text.push_str(&format!(
            " The answers you typed are kept in {} for the next run.",
            path.display()
        ));
    }
    bail!(text)
}

impl Ui<'_> {
    /// A check starts: its screen says so under the step it follows, and what the check itself
    /// prints (a sign-in's page or code) shows below.
    fn checking(&mut self, step: Option<(usize, usize, &'static str)>, title: &str) {
        let _ = self.term.raw(false);
        let s = self.sym();
        let p = self.paint();
        let header = match step {
            Some((n, total, step)) => format!("{} Step {n}/{total} {} {step}", s.arrow, s.dot),
            None => format!("{} Checking", s.arrow),
        };
        let mut lines = vec![p.heading(&header), String::new()];
        lines.extend(self.recent.iter().cloned());
        lines.push(format!("  {} {}", p.note(s.running), title));
        let row = lines.len();
        self.term.draw(&Frame {
            lines,
            cursor: Some((row, 0)),
        });
        self.term.say("");
    }

    /// A check ended: ✓ and what it said, or ✗; on a terminal on the next screen, else said as
    /// before.
    fn checked(&mut self, title: &str, result: &Result<String, CheckError>) {
        let skipped = matches!(result, Ok(said) if said == SKIPPED);
        let said = match result {
            Ok(said) if !skipped => said.as_str(),
            _ => "",
        };
        if !self.interactive() {
            if !said.is_empty() {
                self.term.say(said);
            }
            return;
        }
        let (cols, _) = self.term.size();
        let cols = usize::from(cols);
        let s = self.sym();
        let p = self.paint();
        match result {
            Ok(_) if skipped => {
                let line = format!("  {} {title}: skipped (--no-check)", s.dot);
                self.recent.push(p.note(&line));
            }
            Ok(_) => {
                self.recent
                    .push(format!("  {} {}", p.success(s.check), p.success(title)));
                self.passed.push(title.to_owned());
                for line in said.lines().filter(|l| !l.trim().is_empty()) {
                    self.recent
                        .extend(self.paragraph("    ", line, cols, Paint::note));
                }
            }
            Err(_) => self
                .recent
                .push(format!("  {} {}", p.failure(s.cross), p.failure(title))),
        }
    }

    /// The end: what was set up, what was saved where, and what is left to do by hand. Off a
    /// terminal only what the save said.
    fn summary(&mut self, title: &str, saved: &Saved) {
        let _ = self.term.raw(false);
        if !self.interactive() {
            if !saved.said.is_empty() {
                self.term.say(&saved.said);
            }
            return;
        }
        let (cols, _) = self.term.size();
        let cols = usize::from(cols);
        let s = self.sym();
        let p = self.paint();
        let mut lines = vec![
            String::new(),
            p.heading(&format!("{} Set up {title}", s.check)),
        ];
        for check in &self.passed {
            lines.push(format!("  {} {}", p.success(s.check), check));
        }
        for item in &saved.saved {
            let sign = format!("  {} ", p.success(s.check));
            lines.extend(self.paragraph(&sign, &format!("Saved {item}"), cols, as_is));
        }
        if saved.next.is_some() || !saved.said.is_empty() || !saved.todo.is_empty() {
            lines.push(String::new());
            lines.push(p.heading("Left for you"));
        }
        if let Some(next) = &saved.next {
            let bullet = format!("  {} ", p.literal(s.bullet));
            lines.extend(self.paragraph(
                &bullet,
                &format!("Run `{next}` to fetch your mail."),
                cols,
                as_is,
            ));
        }
        let said = saved.said.lines().map(str::to_owned);
        for said in saved
            .todo
            .iter()
            .cloned()
            .chain(said)
            .filter(|l| !l.trim().is_empty())
        {
            let bullet = format!("  {} ", p.literal(s.bullet));
            lines.extend(self.paragraph(&bullet, &said, cols, as_is));
        }
        self.term.say(&lines.join("\n"));
    }
}

/// A list on its own screen, such as the provider menu: `items` are (name, description).
/// Returns the index picked, or `None` when the user quit.
pub fn choose(
    ui: &mut Ui<'_>,
    title: &str,
    items: &[(&str, &str)],
    start: usize,
) -> Result<Option<usize>> {
    let width = items
        .iter()
        .map(|(n, _)| screen::width(n))
        .max()
        .unwrap_or(0);
    let list: Vec<(String, String)> = items
        .iter()
        .map(|(name, about)| (format!("{name:<width$}  {about}"), (*name).to_owned()))
        .collect();
    static MENU: super::Step = super::Step {
        id: "menu",
        title: "",
        why: "",
        links: &[],
        page: None,
        kind: Kind::Console,
    };
    let view = View {
        number: 1,
        total: 1,
        step: &MENU,
        links: Vec::new(),
        next: None,
        fix: None,
        warning: None,
        can_go_back: false,
        page: None,
        seen: 0,
        seen_total: 0,
        outline: Vec::new(),
    };
    let header = ui.paint().heading(&format!("{} {title}", ui.sym().arrow));
    ui.menu_header = Some(header);
    let picked = ui.pick(&view, &list, start, &[], &[], false);
    ui.menu_header = None;
    Ok(picked?.and_then(Result::ok))
}

#[cfg(test)]
mod tests;
