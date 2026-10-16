//! The one seam between the wizards and the terminal: keys in, screens out. The real terminal
//! ([`Tty`]) uses crossterm for raw mode, keys and the size; the tests script a [`Term`] of
//! their own, press keys and read the screens.

use std::io::{self, Write};

use anyhow::{Context, Result};

use crate::secret::{self, Input};

/// A key the wizard acts on. Anything else is [`Key::Other`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Esc,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    Tab,
    PageUp,
    PageDown,
    /// Ctrl-C: quit.
    Interrupt,
    /// Ctrl-U: clear what was typed.
    ClearLine,
    /// The window changed size: draw again.
    Resize,
    Other,
}

/// One screen to draw: whole lines, styled, and where the cursor goes (row and column, from 0),
/// if it shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    pub lines: Vec<String>,
    pub cursor: Option<(usize, usize)>,
}

/// Where the wizard talks to the user.
pub trait Term {
    /// Whether a person is at a terminal and may be asked: keys can be read and screens drawn.
    /// Off a terminal only secrets are read, one line of standard input each.
    fn interactive(&self) -> bool;
    /// Columns and rows.
    fn size(&self) -> (u16, u16);
    /// Raw mode on (keys one by one, no echo) or off. Off is always safe to ask for again.
    fn raw(&mut self, on: bool) -> Result<()>;
    /// The next key, in raw mode.
    fn key(&mut self) -> Result<Key>;
    /// Replaces what is on the screen with `frame`.
    fn draw(&mut self, frame: &Frame);
    /// Text below what is on the screen, outside raw mode: progress, results, the summary, and
    /// everything off a terminal.
    fn say(&mut self, text: &str);
    /// Off a terminal: a secret as one line of standard input. `what` names it in errors.
    fn secret_line(&mut self, prompt: &str) -> Result<String>;
}

/// Standard error, standard input and the controlling terminal.
pub struct Tty {
    input: Input,
    /// What a secret read off a terminal is called in errors: "password", "client secret".
    what: &'static str,
    raw: bool,
    /// The rows of the last screen drawn, until something is said below it.
    drawn: Option<usize>,
}

impl Tty {
    pub fn new(input: Input, what: &'static str) -> Self {
        if input.can_prompt() {
            restore_on_panic();
        }
        Self {
            input,
            what,
            raw: false,
            drawn: None,
        }
    }

    /// Puts the cursor on a line of its own under the last screen, so what follows (a check's
    /// own output, an error, the shell's prompt) never overwrites it.
    fn below(&mut self) {
        if let Some(rows) = self.drawn.take() {
            let _ = write!(io::stderr(), "\x1b[{rows};1H\r\n\x1b[?25h");
            let _ = io::stderr().flush();
        }
    }
}

/// A panic leaves raw mode before its message prints, even where it does not unwind.
fn restore_on_panic() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = crossterm::terminal::disable_raw_mode();
            let _ = io::stderr().write_all(b"\x1b[?25h\r\n");
            previous(info);
        }));
    });
}

impl Term for Tty {
    fn interactive(&self) -> bool {
        self.input.can_prompt() && io::IsTerminal::is_terminal(&io::stderr())
    }

    fn size(&self) -> (u16, u16) {
        crossterm::terminal::size().unwrap_or((80, 24))
    }

    fn raw(&mut self, on: bool) -> Result<()> {
        if on == self.raw {
            return Ok(());
        }
        if on {
            crossterm::terminal::enable_raw_mode().context("cannot read keys from the terminal")?;
        } else {
            crossterm::terminal::disable_raw_mode().context("cannot restore the terminal")?;
            self.below();
            let _ = io::stderr().write_all(b"\x1b[?25h");
        }
        self.raw = on;
        Ok(())
    }

    fn key(&mut self) -> Result<Key> {
        use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
        loop {
            match event::read().context("cannot read a key")? {
                Event::Resize(..) => return Ok(Key::Resize),
                Event::Key(k) if k.kind != KeyEventKind::Release => {
                    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
                    return Ok(match k.code {
                        KeyCode::Char('c' | 'd') if ctrl => Key::Interrupt,
                        KeyCode::Char('u') if ctrl => Key::ClearLine,
                        KeyCode::Char('n') if ctrl => Key::Down,
                        KeyCode::Char('p') if ctrl => Key::Up,
                        KeyCode::Char(_) if ctrl => Key::Other,
                        KeyCode::Char(c) => Key::Char(c),
                        KeyCode::Enter => Key::Enter,
                        KeyCode::Esc => Key::Esc,
                        KeyCode::Backspace => Key::Backspace,
                        KeyCode::Delete => Key::Delete,
                        KeyCode::Left => Key::Left,
                        KeyCode::Right => Key::Right,
                        KeyCode::Up => Key::Up,
                        KeyCode::Down => Key::Down,
                        KeyCode::Home => Key::Home,
                        KeyCode::End => Key::End,
                        KeyCode::Tab => Key::Tab,
                        KeyCode::PageUp => Key::PageUp,
                        KeyCode::PageDown => Key::PageDown,
                        _ => Key::Other,
                    });
                }
                _ => {}
            }
        }
    }

    fn draw(&mut self, frame: &Frame) {
        let _ = io::stderr().write_all(render(frame).as_bytes());
        let _ = io::stderr().flush();
        let cols = usize::from(self.size().0);
        self.drawn = Some(super::screen::rows(&frame.lines, cols));
    }

    fn say(&mut self, text: &str) {
        self.below();
        if self.raw {
            let _ = write!(io::stderr(), "{}\r\n", text.replace('\n', "\r\n"));
        } else {
            eprintln!("{text}");
        }
    }

    fn secret_line(&mut self, prompt: &str) -> Result<String> {
        secret::read_secret(prompt, self.what, self.input)
    }
}

/// The bytes that draw `frame` over the screen: home, each line cleared to its end, the rest of
/// the screen cleared, then the cursor placed or hidden. One write, so nothing flickers.
pub fn render(frame: &Frame) -> String {
    let mut out = String::from("\x1b[?25l\x1b[H");
    for (i, line) in frame.lines.iter().enumerate() {
        if i > 0 {
            out.push_str("\r\n");
        }
        out.push_str(line);
        out.push_str("\x1b[K");
    }
    out.push_str("\x1b[J");
    if let Some((row, col)) = frame.cursor {
        out.push_str(&format!("\x1b[{};{}H\x1b[?25h", row + 1, col + 1));
    }
    out
}
