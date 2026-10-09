//! Concept 2, "scan & stamp": a light beam sweeps the row while jev reads; on
//! the result a coloured group stamp slams in with a little shake and the
//! confidence bar fills. Stamps land in place (flat view); the grouped view
//! moves rows in batched glides (see `glide`).
//!
//! The base UI already shows the real stamp, so the effect only decorates it:
//! nothing is held back, even at slow `--speed`.

use std::collections::HashMap;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
};
use tachyonfx::Interpolation;

use super::labels::{NARROW_TAG, WIDE_TAG, conf_bar, confidence_short, stamp_text, stamp_width};
use super::util::{bg_at, ease, mix, put_str, theme, tint_bg, tint_fg};
use super::{ASAP, AnimationStyle, Ctx, RowGeo};
use crate::groups::Group;
use crate::model::{Classification, MsgId};

/// One beam sweep while jev is still reading (real seconds).
const SWEEP: f32 = 0.9;
/// The beam needs fewer frames than the stamp.
const BEAM_FRAME: f32 = 1.0 / 30.0;
// Timeline after the result arrives (animation seconds).
const FINAL_SWEEP: f32 = 0.2;
const SLAM: f32 = 0.3;
const BAR: f32 = 0.34;
const RELEASE: f32 = 0.85;

struct Job {
    group: Group,
    conf: f32,
    /// Seconds since the result arrived.
    t: f32,
}

pub struct Stamp {
    jobs: HashMap<MsgId, Job>,
}

impl Stamp {
    pub fn new() -> Self {
        Stamp { jobs: HashMap::new() }
    }
}

/// Beam head at `p` (0..1) with a short tail. Tints are quantised to three
/// levels to keep colour changes (terminal output) low.
fn beam(buf: &mut Buffer, row: &RowGeo, p: f32, color: Color, strength: f32) {
    let head = (row.x as f32 + p * (row.width as f32 + 8.0) - 4.0).round() as i32;
    for dx in -7i32..=0 {
        let x = head + dx;
        if x < row.x as i32 || x >= (row.x + row.width) as i32 {
            continue;
        }
        let level = match -dx {
            0..=1 => 1.0,
            2..=4 => 0.6,
            _ => 0.3,
        } * strength;
        let r = Rect::new(x as u16, row.y, 1, 1);
        tint_bg(buf, r, color, 0.5 * level);
        tint_fg(buf, r, Color::White, 0.5 * level);
    }
}

impl AnimationStyle for Stamp {
    fn on_result(&mut self, c: &Classification, _ctx: &Ctx) {
        self.jobs.insert(c.id, Job { group: c.group, conf: c.confidence, t: 0.0 });
    }

    fn update(&mut self, dt: f32) {
        for job in self.jobs.values_mut() {
            job.t += dt;
        }
        self.jobs.retain(|_, j| j.t < RELEASE);
    }

    fn overlay(&mut self, buf: &mut Buffer, ctx: &Ctx) {
        for (id, age) in ctx.reading {
            if let Some(row) = ctx.geo.rows.get(id) {
                let p = (age % SWEEP) / SWEEP;
                beam(buf, row, ease(Interpolation::SineInOut, p), theme::JEV, 0.8);
            }
        }
        for (id, job) in &self.jobs {
            let Some(row) = ctx.geo.rows.get(id) else { continue };
            let (g, t) = (job.group, job.t);
            if t < FINAL_SWEEP {
                beam(buf, row, ease(Interpolation::QuadIn, t / FINAL_SWEEP), g.color(), 1.0);
                continue;
            }
            let since = t - FINAL_SWEEP;
            let fade = (1.0 - since / (RELEASE - FINAL_SWEEP)).max(0.0);
            if let Some(bin) = ctx.geo.bins[g.index()] {
                tint_bg(buf, bin, g.color(), 0.35 * fade);
            }
            if row.tag.1 < NARROW_TAG {
                // Only a glyph fits in the tag column: flash the row instead.
                tint_bg(buf, row.rect(), g.color(), 0.3 * fade);
                continue;
            }
            tint_bg(buf, row.rect(), g.color(), 0.22 * fade);
            let bg = bg_at(buf, row.tag.0.saturating_sub(1), row.y);
            let (tx, ty) = (row.tag.0 as i32, row.y as i32);
            let sw = stamp_width(row.tag.1);
            let shake = match (since * 60.0) as u32 {
                0..=1 => 2,
                2..=3 => -2,
                4..=5 => 1,
                6..=7 => -1,
                _ => 0,
            };
            let slam = t < SLAM;
            // Clear the tag cells the shake uncovers.
            put_str(buf, tx, ty, &" ".repeat(sw), Style::new().bg(bg), sw);
            let stamp_bg = mix(bg, g.color(), if slam { 0.8 } else { 0.2 + 0.45 * fade });
            let fg = if slam { Color::White } else { g.color() };
            let style = Style::new().fg(fg).bg(stamp_bg).add_modifier(Modifier::BOLD);
            put_str(buf, tx + shake, ty, &stamp_text(g, sw), style, sw);
            if t >= BAR {
                let fill = job.conf * ease(Interpolation::QuadOut, (t - BAR) / (RELEASE - BAR - 0.15));
                if row.tag.1 >= WIDE_TAG {
                    let bar_style = Style::new().fg(mix(g.color(), Color::White, 0.3)).bg(bg);
                    put_str(buf, tx + 15, ty, &conf_bar(fill, 4), bar_style, 4);
                }
                // Percentage over the time column while the bar fills.
                let tx2 = (row.x + row.width) as i32 - 6;
                let pct = format!("{} ", confidence_short(g, fill));
                put_str(buf, tx2, ty, &pct, Style::new().fg(g.color()).bg(bg), 5);
            }
        }
    }

    fn next_frame(&self, ctx: &Ctx) -> Option<f32> {
        if !self.jobs.is_empty() {
            Some(ASAP)
        } else if ctx.reading.iter().any(|(id, _)| ctx.geo.rows.contains_key(id)) {
            Some(BEAM_FRAME)
        } else {
            None
        }
    }

    fn backlog(&self) -> usize {
        self.jobs.len()
    }

    fn finish_all(&mut self) {
        self.jobs.clear();
    }
}
