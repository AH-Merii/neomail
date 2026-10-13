//! Concept 3, "decode": while jev thinks, a short scramble window drifts
//! across the subject (the rest stays readable). On the result a decoder
//! cursor sweeps left to right over subject and tag, leaving text tinted with
//! the group colour; the tint fades back over `FADE` seconds, so the list does
//! not stay a rainbow and read mail keeps its dimming. The group counter
//! (already up to date) sparkles when the cursor reaches the tag.

use std::collections::HashMap;
use std::ops::Range;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier},
};
use tachyonfx::{EffectManager, Interpolation, fx};

use super::util::{ease, mix, put, read_row, theme, tint_fg};
use super::{ASAP, AnimationStyle, Ctx};
use crate::groups::Group;
use crate::model::{Classification, MsgId};

const GLITCH: &[char] = &[
    '░', '▒', '▓', '<', '>', '/', '\\', '|', '#', '%', '&', '*', '+', '=', '-', ':', ';', '0', '1', '?', '^', '{', '}',
];
/// Scrambled cells at once (reading window and decoder cursor).
const WINDOW: usize = 5;
/// Scramble refresh rate; frames in between would be identical.
const SCRAMBLE_HZ: f32 = 24.0;
/// Reading window drift period (real seconds).
const DRIFT: f32 = 1.4;
const FADE: f32 = 1.5;
const SPARKLE: f32 = 0.7;

struct Job {
    group: Group,
    t: f32,
    resolve: f32,
    sparkled: bool,
}

pub struct Decode {
    jobs: HashMap<MsgId, Job>,
    sparkles: [f32; 11],
    fx: EffectManager<u32>,
    pending_dt: f32,
    to_flash: Vec<Group>,
    seed: u64,
}

impl Decode {
    pub fn new(seed: u64) -> Self {
        Decode { jobs: HashMap::new(), sparkles: [0.0; 11], fx: EffectManager::default(), pending_dt: 0.0, to_flash: Vec::new(), seed }
    }

    fn glitch(&self, id: MsgId, i: usize, frame: u32) -> (char, f32) {
        let mut z = self.seed ^ ((id as u64) << 32) ^ ((i as u64) << 16) ^ frame as u64;
        z = (z ^ (z >> 33)).wrapping_mul(0xff51afd7ed558ccd);
        z = (z ^ (z >> 33)).wrapping_mul(0xc4ceb9fe1a85ec53);
        z ^= z >> 33;
        (GLITCH[(z >> 8) as usize % GLITCH.len()], 0.45 + ((z >> 20) % 100) as f32 / 250.0)
    }

    /// Scrambles cells `from..to` of a row span, skipping blanks.
    fn scramble(&self, buf: &mut Buffer, id: MsgId, (x0, y): (u16, u16), cells: &[char], range: Range<usize>, (frame, tint): (u32, Color)) {
        for (i, &ch) in cells.iter().enumerate().take(range.end.min(cells.len())).skip(range.start) {
            if ch == ' ' {
                continue;
            }
            let (g, shade) = self.glitch(id, i, frame);
            put(buf, x0 as i32 + i as i32, y as i32, g, mix(theme::BG, tint, shade), None, false);
        }
    }
}

impl AnimationStyle for Decode {
    fn on_result(&mut self, c: &Classification, ctx: &Ctx) {
        let len = ctx.model.get(c.id).map_or(30, |m| m.subject.chars().count());
        self.jobs.insert(c.id, Job { group: c.group, t: 0.0, resolve: 0.3 + len.min(60) as f32 * 0.008, sparkled: false });
    }

    fn update(&mut self, dt: f32) {
        self.pending_dt += dt;
        for job in self.jobs.values_mut() {
            job.t += dt;
            if job.t >= job.resolve && !job.sparkled {
                job.sparkled = true;
                self.sparkles[job.group.index()] = SPARKLE;
                self.to_flash.push(job.group);
            }
        }
        self.jobs.retain(|_, j| j.t < j.resolve + FADE);
        for s in self.sparkles.iter_mut() {
            *s = (*s - dt).max(0.0);
        }
    }

    fn overlay(&mut self, buf: &mut Buffer, ctx: &Ctx) {
        for (id, age) in ctx.reading {
            let Some(row) = ctx.geo.rows.get(id) else { continue };
            let (sx, sw) = row.subject;
            let cells = read_row(buf, sx, row.y, sw);
            let span = cells.len() + WINDOW;
            let head = ((age % DRIFT) / DRIFT * span as f32) as usize;
            let from = head.saturating_sub(WINDOW);
            self.scramble(buf, *id, (sx, row.y), &cells, from..head, ((age * SCRAMBLE_HZ) as u32, theme::JEV));
        }
        for (id, job) in &self.jobs {
            let Some(row) = ctx.geo.rows.get(id) else { continue };
            // One span from the subject through the end of the tag column.
            let x0 = row.subject.0;
            let w = (row.tag.0 + row.tag.1).saturating_sub(x0);
            let cells = read_row(buf, x0, row.y, w);
            let p = ease(Interpolation::Linear, job.t / job.resolve);
            let cursor = (p * (cells.len() + WINDOW) as f32) as usize;
            let fade = ((job.t - job.resolve) / FADE).clamp(0.0, 1.0);
            let tint = 0.75 * (1.0 - ease(Interpolation::QuadIn, fade));
            let done = Rect::new(x0, row.y, cursor.min(cells.len()) as u16, 1);
            tint_fg(buf, done, job.group.color(), tint);
            if cursor < cells.len() + WINDOW {
                let from = cursor.saturating_sub(WINDOW);
                let frame = (job.t * SCRAMBLE_HZ) as u32;
                self.scramble(buf, *id, (x0, row.y), &cells, from..cursor, (frame, job.group.color()));
                if let Some(&ch) = cells.get(cursor) {
                    put(buf, x0 as i32 + cursor as i32, row.y as i32, ch, Color::White, None, true);
                }
            }
        }

        // Counter sparkle: tachyonfx fade on the bin plus twinkling stars.
        let dt = std::time::Duration::from_secs_f32(self.pending_dt);
        self.pending_dt = 0.0;
        for g in self.to_flash.drain(..) {
            if let Some(bin) = ctx.geo.bins[g.index()] {
                let flash = fx::fade_from_fg(Color::White, (600, Interpolation::QuadOut));
                self.fx.add_effect(flash.with_area(bin));
            }
        }
        for g in Group::ALL {
            let s = self.sparkles[g.index()];
            let Some((cx, cy)) = ctx.geo.counters[g.index()] else { continue };
            if s <= 0.0 {
                continue;
            }
            let k = s / SPARKLE;
            let stars = ['✦', '✧', '*', '·'];
            let phase = ((1.0 - k) * 8.0) as usize;
            let c = mix(g.color(), Color::White, k);
            let (left, right) = if ctx.geo.compact { (cx as i32 - 2, cx as i32 + 2) } else { (cx as i32 - 1, cx as i32 + 3) };
            put(buf, left, cy as i32, stars[phase % 4], c, None, true);
            put(buf, right, cy as i32, stars[(phase + 2) % 4], c, None, true);
            if let Some(cell) = buf.cell_mut((cx + 2, cy)) {
                cell.modifier.insert(Modifier::BOLD);
            }
        }
        self.fx.process_effects(dt, buf, ctx.geo.area);
    }

    fn next_frame(&self, ctx: &Ctx) -> Option<f32> {
        let sweeping = self.jobs.values().any(|j| j.t < j.resolve);
        if sweeping || self.sparkles.iter().any(|&s| s > 0.0) || self.fx.is_running() {
            Some(ASAP)
        } else if !self.jobs.is_empty() {
            Some(1.0 / 20.0) // tint fade
        } else if ctx.reading.iter().any(|(id, _)| ctx.geo.rows.contains_key(id)) {
            Some(1.0 / SCRAMBLE_HZ)
        } else {
            None
        }
    }

    fn backlog(&self) -> usize {
        self.jobs.values().filter(|j| j.t < j.resolve).count()
    }

    fn finish_all(&mut self) {
        self.jobs.clear();
        self.sparkles = [0.0; 11];
        self.fx = EffectManager::default();
        self.to_flash.clear();
    }
}
