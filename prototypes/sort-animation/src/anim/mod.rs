//! Animation layer: pluggable "animation styles" fed by jev events.
//!
//! The layer is presentation-only and never changes what the base UI shows:
//! the app applies every `JevEvent` to the model first, renders the real
//! model state, and then lets the `Animator` draw effects on top.
//!
//! Integration contract for the real TUI:
//! 1. forward every jev event to `Animator::on_event` after the model,
//! 2. render the base UI from the model and publish a `Geometry`
//!    (where rows and group bins are on screen),
//! 3. call `Animator::overlay` on the frame buffer,
//! 4. sleep until `Animator::next_frame` (None = idle, no wake-ups).
//!
//! "jev is reading" effects are derived from the model on every frame, so a
//! style can never get stuck showing a row as reading. Styles only keep
//! short-lived state for result animations, and the animator drops all of it
//! whenever the effective motion mode changes.

pub mod animator;
pub mod decode;
pub mod glide;
pub mod labels;
pub mod machine;
pub mod reduced;
pub mod stamp;
pub mod util;

use std::collections::HashMap;

use ratatui::{buffer::Buffer, layout::Rect};

pub use animator::Animator;

use crate::model::{Classification, Model, MsgId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StyleId {
    Machine,
    Stamp,
    Decode,
}

impl StyleId {
    pub fn name(self) -> &'static str {
        match self {
            StyleId::Machine => "machine",
            StyleId::Stamp => "stamp",
            StyleId::Decode => "decode",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MotionMode {
    Full,
    Reduced,
    Off,
}

impl MotionMode {
    pub fn name(self) -> &'static str {
        match self {
            MotionMode::Full => "full",
            MotionMode::Reduced => "reduced",
            MotionMode::Off => "off",
        }
    }

    pub fn next(self) -> Self {
        match self {
            MotionMode::Full => MotionMode::Reduced,
            MotionMode::Reduced => MotionMode::Off,
            MotionMode::Off => MotionMode::Full,
        }
    }
}

/// Screen positions of one message row.
#[derive(Clone, Copy, Debug, Default)]
pub struct RowGeo {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    /// Subject column: (x, width).
    pub subject: (u16, u16),
    /// Group tag column: (x, width).
    pub tag: (u16, u16),
}

impl RowGeo {
    pub fn rect(&self) -> Rect {
        Rect::new(self.x, self.y, self.width, 1)
    }
}

/// Where things are on screen this frame (published by the UI).
#[derive(Clone, Debug, Default)]
pub struct Geometry {
    pub area: Rect,
    /// List body (rows only).
    pub list: Rect,
    /// Region effects may travel through (list body plus sidebar/tabs).
    /// Flights must stay inside it so they never cross the title bar.
    pub stage: Rect,
    /// Visible rows only.
    pub rows: HashMap<MsgId, RowGeo>,
    /// Sidebar bin (or compact tab) per group.
    pub bins: [Option<Rect>; 11],
    /// Position of each bin's counter text.
    pub counters: [Option<(u16, u16)>; 11],
    /// Compact layout: group tabs instead of a sidebar.
    pub compact: bool,
}

/// Everything a style may look at while reacting or drawing.
pub struct Ctx<'a> {
    pub geo: &'a Geometry,
    pub model: &'a Model,
    /// Rows jev is reading right now (from the model) with the real seconds
    /// since reading started.
    pub reading: &'a [(MsgId, f32)],
}

/// Request "as fast as the frame budget allows".
pub const ASAP: f32 = 0.0;

/// A pluggable animation style.
pub trait AnimationStyle {
    /// React to a classification result (already applied to the model).
    fn on_result(&mut self, c: &Classification, ctx: &Ctx);
    /// Advance by `dt` seconds of (speed-scaled) animation time.
    fn update(&mut self, dt: f32);
    /// Draw effects on top of the rendered base UI.
    fn overlay(&mut self, buf: &mut Buffer, ctx: &Ctx);
    /// Seconds until the style next needs a frame; `None` when idle.
    fn next_frame(&self, ctx: &Ctx) -> Option<f32>;
    /// Result animations still playing (drives speed-up and skipping).
    fn backlog(&self) -> usize;
    /// Jump every running animation to its final state.
    fn finish_all(&mut self);
}

/// Earliest of two optional deadlines.
pub fn sooner(a: Option<f32>, b: Option<f32>) -> Option<f32> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (x, None) => x,
        (None, y) => y,
    }
}
