//! A scripted terminal for the tests: answers by screen, or keys pressed one by one, and every
//! screen kept to read back. Nothing here touches a real terminal.

use std::cell::Cell;
use std::collections::VecDeque;
use std::rc::Rc;

use anyhow::{Result, bail};

use super::cli::{Ui, no_opener};
use super::remember::Memory;
use super::screen::{self, Look};
use super::term::{Frame, Key, Term};
use crate::style::{Paint, Symbols};

/// `lines` answer the screens in order: on a step to read, "" is Enter and "<" is Esc (back);
/// on a list, a digit points at that item and other text narrows it, then Enter; on a line,
/// the text replaces what is there, then Enter. `secrets` answer hidden lines, and off a
/// terminal the secrets read from standard input. The welcome, the save and every yes or no
/// are answered yes, unless `keys` holds keys, which are pressed first, one per read.
#[derive(Default)]
pub struct Script {
    pub interactive: bool,
    pub lines: VecDeque<&'static str>,
    pub secrets: VecDeque<&'static str>,
    pub keys: VecDeque<Key>,
    /// Keys that answer the questions whose default is no ("[y/N]"), in order; then yes.
    pub confirms: VecDeque<Key>,
    pub secrets_asked: usize,
    /// What was said, and each screen as plain text, in order.
    pub said: String,
    /// The prompt of each line asked, in order.
    pub prompts: Vec<String>,
    /// Every screen drawn, styled as drawn.
    pub frames: Vec<Frame>,
    pub size: (u16, u16),
    pub raw: bool,
    /// Raw mode was turned on this many times, and off this many.
    pub raw_ons: usize,
    pub raw_offs: usize,
    pub pending: VecDeque<Key>,
    /// Whether a line printed outside the seam, such as a save's or a sign-in's own output,
    /// would land in cooked mode below the last screen: raw mode is off, and the cursor left
    /// that screen as [`super::term::Tty`] leaves it (raw mode turned off, or something said).
    /// Shared, so a check or a write can read it while the Ui holds the Script.
    pub can_print: Rc<Cell<bool>>,
    /// A screen was drawn and the cursor is still on it.
    pub on_screen: bool,
    /// At a screen that waits for a command's result ([`Term::key_within`]) with no key
    /// scripted: how many waits pass with no key, each calling `on_wait` first (it may write the
    /// result), before the screen is answered from `lines`.
    pub waits: usize,
    pub on_wait: Option<Box<dyn FnMut()>>,
}

impl Script {
    pub fn terminal(lines: &[&'static str], secrets: &[&'static str]) -> Self {
        Self {
            interactive: true,
            lines: lines.iter().copied().collect(),
            secrets: secrets.iter().copied().collect(),
            size: (100, 40),
            ..Self::default()
        }
    }

    pub fn off_terminal(secrets: &[&'static str]) -> Self {
        Self {
            secrets: secrets.iter().copied().collect(),
            size: (100, 40),
            ..Self::default()
        }
    }

    /// Only these keys, pressed in order.
    pub fn keys(keys: &[Key]) -> Self {
        Self {
            interactive: true,
            keys: keys.iter().copied().collect(),
            size: (100, 40),
            ..Self::default()
        }
    }

    /// The last screen as plain text.
    pub fn screen(&self) -> String {
        self.frames
            .last()
            .map(|f| {
                f.lines
                    .iter()
                    .map(|l| screen::plain(l))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default()
    }

    fn typed(text: &str) -> impl Iterator<Item = Key> + '_ {
        std::iter::once(Key::ClearLine)
            .chain(text.chars().map(Key::Char))
            .chain(std::iter::once(Key::Enter))
    }

    /// The keys that answer the screen shown last.
    fn answer(&mut self) -> Result<()> {
        let shown = self.screen();
        let has = |s: &str| shown.contains(s);
        if has("[y/N]")
            && let Some(key) = self.confirms.pop_front()
        {
            self.pending.push_back(key);
            return Ok(());
        }
        if has("Ready to start?") || has("Save it now?") || has("[y/N]") {
            self.pending.push_back(Key::Char('y'));
            return Ok(());
        }
        if has("Typing is hidden") {
            self.secrets_asked += 1;
            self.prompts.push(prompt_of(&shown));
            let Some(secret) = self.secrets.pop_front() else {
                bail!("no secret for:\n{shown}\n--- said:\n{}", self.said);
            };
            self.pending.extend(Self::typed(secret));
            return Ok(());
        }
        let Some(line) = self.lines.pop_front() else {
            bail!("no answer for:\n{shown}\n--- said:\n{}", self.said);
        };
        if line == "<" {
            self.pending.push_back(Key::Esc);
        } else if has("Pick one, or type") || has("Narrowed by") {
            self.pending.extend(line.chars().map(Key::Char));
            self.pending.push_back(Key::Enter);
        } else if has("s skip") {
            self.pending.push_back(Key::Enter);
        } else {
            self.prompts.push(prompt_of(&shown));
            self.pending.extend(Self::typed(line));
        }
        Ok(())
    }
}

/// The prompt line of a screen: the one that starts with "? ".
fn prompt_of(shown: &str) -> String {
    shown
        .lines()
        .find_map(|l| l.trim_start().strip_prefix("? "))
        .unwrap_or_default()
        .to_owned()
}

impl Term for Script {
    fn interactive(&self) -> bool {
        self.interactive
    }

    fn size(&self) -> (u16, u16) {
        match self.size {
            (0, _) | (_, 0) => (100, 40),
            size => size,
        }
    }

    fn raw(&mut self, on: bool) -> Result<()> {
        if on {
            self.raw_ons += 1;
        } else if self.raw {
            self.raw_offs += 1;
            self.on_screen = false;
        }
        self.raw = on;
        self.can_print.set(!self.raw && !self.on_screen);
        Ok(())
    }

    fn key(&mut self) -> Result<Key> {
        assert!(self.raw, "a key read outside raw mode");
        if let Some(key) = self.keys.pop_front() {
            return Ok(key);
        }
        if self.pending.is_empty() {
            self.answer()?;
        }
        Ok(self.pending.pop_front().unwrap_or(Key::Enter))
    }

    fn key_within(&mut self, _wait: std::time::Duration) -> Result<Option<Key>> {
        assert!(self.raw, "a key read outside raw mode");
        if self.keys.is_empty() && self.pending.is_empty() && self.waits > 0 {
            self.waits -= 1;
            if let Some(on_wait) = self.on_wait.as_mut() {
                on_wait();
            }
            return Ok(None);
        }
        self.key().map(Some)
    }

    fn draw(&mut self, frame: &Frame) {
        // A new screen: what was pending answered the one before.
        if self.frames.last().map(|f| &f.lines) != Some(&frame.lines) {
            let plain: Vec<String> = frame.lines.iter().map(|l| screen::plain(l)).collect();
            self.said.push_str(&plain.join("\n"));
            self.said.push('\n');
        }
        self.frames.push(frame.clone());
        self.on_screen = true;
        self.can_print.set(false);
    }

    fn say(&mut self, text: &str) {
        self.said.push_str(text);
        self.said.push('\n');
        self.on_screen = false;
        self.can_print.set(!self.raw);
    }

    fn secret_line(&mut self, prompt: &str) -> Result<String> {
        self.secrets_asked += 1;
        self.prompts.push(prompt.to_owned());
        match self.secrets.pop_front() {
            Some(s) => Ok(s.to_owned()),
            None => bail!("no secret for {prompt:?}:\n{}", self.said),
        }
    }
}

/// A [`Ui`] on `term` without colour, with Unicode symbols, opening no page and keeping no
/// answers.
pub fn ui(term: &mut Script) -> Ui<'_> {
    let look = Look {
        paint: Paint::OFF,
        sym: Symbols::UNICODE,
        hyperlinks: false,
    };
    Ui::new(term, look, &no_opener, false, Memory::none())
}
