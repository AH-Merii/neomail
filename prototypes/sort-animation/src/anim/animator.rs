//! The animator: routes jev results to the active style and owns motion
//! modes, backlog handling, bulk mode and the frame schedule.

use std::collections::{HashMap, VecDeque};

use ratatui::buffer::Buffer;

use super::{AnimationStyle, Ctx, Geometry, MotionMode, StyleId, decode, glide, machine, reduced, sooner, stamp};
use crate::groups::Group;
use crate::model::{JevEvent, JevState, Model, MsgId};

/// Backlog above which animations speed up.
const SPEEDUP_FROM: usize = 3;
/// Backlog at which everything skips to its final state.
const SKIP_AT: usize = 12;
/// Results within `BULK_WINDOW` seconds that switch to counters-only mode
/// (long initial syncs), and the level that switches back.
const BULK_ENTER: usize = 16;
const BULK_EXIT: usize = 6;
const BULK_WINDOW: f32 = 2.0;
/// Rate of the base UI's "jev reading" spinner.
const SPINNER_HZ: f32 = 8.0;
/// How long the "skipped to final state" note stays up.
const SKIP_NOTE: f32 = 1.2;

pub struct Animator {
    pub style_id: StyleId,
    style: Box<dyn AnimationStyle>,
    reduced: reduced::Reduced,
    glide: glide::Glide,
    pub motion: MotionMode,
    /// Small terminals force reduced motion.
    pub tiny: bool,
    /// Grouped inbox view (a view setting, not tied to a style).
    pub grouped: bool,
    /// Motion mode the current animation state was built for.
    effective: MotionMode,
    pub speed: f32,
    pub skipped: usize,
    skip_note: f32,
    seed: u64,
    /// Real seconds since start.
    clock: f32,
    /// Reading start time per row, reconciled with the model every frame.
    reading: HashMap<MsgId, f32>,
    reading_ages: Vec<(MsgId, f32)>,
    results: VecDeque<f32>,
    pub bulk: bool,
}

fn make(id: StyleId, seed: u64) -> Box<dyn AnimationStyle> {
    match id {
        StyleId::Machine => Box::new(machine::Machine::new()),
        StyleId::Stamp => Box::new(stamp::Stamp::new()),
        StyleId::Decode => Box::new(decode::Decode::new(seed)),
    }
}

impl Animator {
    pub fn new(style_id: StyleId, motion: MotionMode, speed: f32, seed: u64, grouped: bool) -> Self {
        Animator {
            style_id,
            style: make(style_id, seed),
            reduced: reduced::Reduced::default(),
            glide: glide::Glide::default(),
            motion,
            tiny: false,
            grouped,
            effective: motion,
            speed,
            skipped: 0,
            skip_note: 0.0,
            seed,
            clock: 0.0,
            reading: HashMap::new(),
            reading_ages: Vec::new(),
            results: VecDeque::new(),
            bulk: false,
        }
    }

    pub fn effective_motion(&self) -> MotionMode {
        if self.tiny && self.motion == MotionMode::Full { MotionMode::Reduced } else { self.motion }
    }

    /// Call on every resize (and at start) before drawing.
    pub fn set_viewport(&mut self, width: u16, height: u16) {
        self.tiny = width < 60 || height < 16;
        self.resync();
    }

    pub fn set_style(&mut self, id: StyleId) {
        self.style_id = id;
        self.style = make(id, self.seed);
    }

    pub fn cycle_motion(&mut self) {
        self.motion = self.motion.next();
        self.resync();
    }

    pub fn toggle_grouped(&mut self) {
        self.grouped = !self.grouped;
        self.glide.reset();
    }

    /// Any change of the effective motion mode starts from a clean slate:
    /// result animations are dropped (the model already shows the results)
    /// and reading effects are rebuilt from the model on the next frame.
    fn resync(&mut self) {
        let eff = self.effective_motion();
        if eff != self.effective {
            self.style = make(self.style_id, self.seed);
            self.reduced = reduced::Reduced::default();
            self.glide.reset();
            self.skip_note = 0.0;
            self.effective = eff;
        }
    }

    /// Reconciles presentation state with the model. Call once per frame.
    pub fn sync(&mut self, model: &Model) {
        self.resync();
        let clock = self.clock;
        let thinking: Vec<MsgId> =
            model.messages.iter().filter(|m| m.state == JevState::Thinking).map(|m| m.id).collect();
        self.reading.retain(|id, _| thinking.contains(id));
        for id in thinking {
            self.reading.entry(id).or_insert(clock);
        }
        self.reading_ages = self.reading.iter().map(|(id, t)| (*id, clock - t)).collect();
        let immediate = self.effective != MotionMode::Full;
        self.glide.refresh_sections(model, clock, immediate);
    }

    pub fn on_event(&mut self, ev: &JevEvent, model: &Model, geo: &Geometry) {
        let c = match ev {
            JevEvent::Arrived(_) => return,
            JevEvent::Started(id) => {
                self.reading.entry(*id).or_insert(self.clock);
                return;
            }
            JevEvent::Classified(c) => c,
        };
        self.reading.remove(&c.id);
        self.reading_ages.retain(|(id, _)| *id != c.id);
        self.results.push_back(self.clock);
        while self.results.front().is_some_and(|t| self.clock - t > BULK_WINDOW) {
            self.results.pop_front();
        }
        let n = self.results.len();
        self.bulk = if self.bulk { n > BULK_EXIT } else { n >= BULK_ENTER };
        if self.bulk {
            self.style.finish_all();
        }
        let ctx = Ctx { geo, model, reading: &self.reading_ages };
        match self.effective {
            MotionMode::Off => {}
            MotionMode::Reduced => self.reduced.on_result(c, &ctx),
            MotionMode::Full if self.bulk => self.reduced.pulse_bin(c.group),
            MotionMode::Full => {
                if self.style.backlog() >= SKIP_AT {
                    self.style.finish_all();
                    self.skipped += 1;
                    self.skip_note = SKIP_NOTE;
                }
                self.style.on_result(c, &ctx);
            }
        }
    }

    /// Speed multiplier including backlog catch-up.
    pub fn time_scale(&self) -> f32 {
        let backlog = self.style.backlog();
        let catch_up = if backlog > SPEEDUP_FROM {
            (1.0 + 0.35 * (backlog - SPEEDUP_FROM) as f32).min(3.5)
        } else {
            1.0
        };
        self.speed * catch_up
    }

    /// Advances by `real_dt` wall-clock seconds.
    pub fn update(&mut self, real_dt: f32) {
        self.clock += real_dt;
        self.skip_note = (self.skip_note - real_dt).max(0.0);
        if self.effective == MotionMode::Full {
            let dt = real_dt * self.time_scale();
            self.style.update(dt);
        }
        self.reduced.update(real_dt);
        self.glide.update(real_dt);
    }

    /// Seconds until the next frame is needed; `None` when nothing moves.
    pub fn next_frame(&self, geo: &Geometry, model: &Model) -> Option<f32> {
        let ctx = Ctx { geo, model, reading: &self.reading_ages };
        let mut next = None;
        if !self.reading.is_empty() && self.effective != MotionMode::Off {
            let phase = (self.clock * SPINNER_HZ).fract();
            next = Some((1.0 - phase) / SPINNER_HZ);
        }
        if self.skip_note > 0.0 {
            next = sooner(next, Some(self.skip_note));
        }
        if self.effective == MotionMode::Full {
            next = sooner(next, self.style.next_frame(&ctx));
            next = sooner(next, self.glide.next_frame());
        }
        sooner(next, self.reduced.next_frame(&ctx))
    }

    pub fn backlog(&self) -> usize {
        self.style.backlog()
    }

    pub fn skip_note(&self) -> bool {
        self.skip_note > 0.0
    }

    /// Section a row is presented in (grouped view). Section changes are
    /// committed in batches so rows glide together instead of constantly.
    pub fn section(&self, id: MsgId) -> Option<Group> {
        self.glide.section(id)
    }

    /// Adjusts body-relative row positions (grouped view glides).
    pub fn place_rows(&mut self, rows: &mut [(MsgId, f32)]) {
        if self.effective == MotionMode::Full && self.grouped {
            self.glide.place(rows);
        }
    }

    pub fn overlay(&mut self, buf: &mut Buffer, geo: &Geometry, model: &Model) {
        let ctx = Ctx { geo, model, reading: &self.reading_ages };
        if self.effective == MotionMode::Full {
            self.style.overlay(buf, &ctx);
        }
        if self.effective != MotionMode::Off {
            self.reduced.overlay(buf, &ctx);
        }
    }
}
