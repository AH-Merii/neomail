//! Terminal output: synchronized updates, an output byte counter and
//! adaptive frame pacing for slow terminals and SSH.

use std::io::{self, Stdout, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};
use crossterm::{execute, queue};
use ratatui::{Frame, Terminal, backend::CrosstermBackend};

/// Stdout wrapper that counts bytes written.
pub struct Counting {
    inner: Stdout,
    bytes: Arc<AtomicU64>,
}

impl Write for Counting {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.bytes.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

pub struct Term {
    pub terminal: Terminal<CrosstermBackend<Counting>>,
    bytes: Arc<AtomicU64>,
}

impl Term {
    /// Raw mode, alternate screen and panic hook come from `ratatui::init`;
    /// drawing goes through our own counting backend.
    pub fn init() -> io::Result<Term> {
        drop(ratatui::init());
        let bytes = Arc::new(AtomicU64::new(0));
        let backend = CrosstermBackend::new(Counting { inner: io::stdout(), bytes: bytes.clone() });
        Ok(Term { terminal: Terminal::new(backend)?, bytes })
    }

    /// Draws one frame inside a synchronized update (DEC mode 2026), so
    /// terminals that support it never show half-drawn frames. Returns the
    /// bytes written.
    pub fn draw(&mut self, render: impl FnOnce(&mut Frame)) -> io::Result<u64> {
        let before = self.bytes.load(Ordering::Relaxed);
        queue!(self.terminal.backend_mut(), BeginSynchronizedUpdate)?;
        self.terminal.draw(render)?;
        execute!(self.terminal.backend_mut(), EndSynchronizedUpdate)?;
        Ok(self.bytes.load(Ordering::Relaxed) - before)
    }
}

const MIN_FRAME: f32 = 1.0 / 60.0;
const MAX_FRAME: f32 = 1.0 / 12.0;
/// Output budget: above this the frame rate drops (slow links, SSH).
const BYTES_PER_SEC: f32 = 100_000.0;
/// Frames further apart than this do not count towards the fps readout.
const FPS_GAP: f32 = 0.25;

/// Measures draw cost, output size and frame rate.
pub struct Pacer {
    cost_ema: f32,
    bytes_ema: f32,
    last: Option<Instant>,
    fps: Option<f32>,
    kbps: f32,
}

impl Default for Pacer {
    fn default() -> Self {
        Pacer { cost_ema: 0.002, bytes_ema: 0.0, last: None, fps: None, kbps: 0.0 }
    }
}

impl Pacer {
    /// Shortest frame interval this terminal can sustain right now.
    pub fn budget(&self) -> f32 {
        let by_cost = self.cost_ema * 1.5;
        let by_bytes = self.bytes_ema / BYTES_PER_SEC;
        by_cost.max(by_bytes).clamp(MIN_FRAME, MAX_FRAME)
    }

    pub fn record(&mut self, start: Instant, cost: f32, bytes: u64) {
        self.cost_ema = self.cost_ema * 0.9 + cost * 0.1;
        self.bytes_ema = self.bytes_ema * 0.8 + bytes as f32 * 0.2;
        if let Some(prev) = self.last {
            let dt = start.duration_since(prev).as_secs_f32();
            if dt < FPS_GAP && dt > 0.0 {
                let inst = 1.0 / dt;
                self.fps = Some(self.fps.map_or(inst, |f| f * 0.85 + inst * 0.15));
                self.kbps = self.kbps * 0.85 + bytes as f32 / dt / 1000.0 * 0.15;
            }
        }
        self.last = Some(start);
    }

    /// Nothing animates: the readout switches to "idle".
    pub fn idle(&mut self) {
        self.fps = None;
        self.last = None;
    }

    pub fn fps(&self) -> Option<f32> {
        self.fps
    }

    pub fn kbps(&self) -> f32 {
        self.kbps
    }
}
