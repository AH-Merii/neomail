//! Concept 1, "sorting machine": a small envelope chip lifts out of the row
//! and flies along a shallow curve into its group's bin in the sidebar, where
//! the (already updated) counter pulses with "+n".
//!
//! - The curve stays inside the list and sidebar (`Geometry::stage`) with a
//!   lift of at most 3 rows, so it never crosses the title bar.
//! - At most `MAX_FLIGHTS` chips fly at once. Further results join a flight
//!   to the same bin or land straight away as a batched pulse.
//! - Without a sidebar (compact layout) there is no flight: the tab flashes.

use std::collections::HashMap;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
};
use tachyonfx::{EffectManager, Interpolation, fx};

use super::util::{bezier, bg_at, ease, mix, put, put_str, theme, tint_bg};
use super::{ASAP, AnimationStyle, Ctx};
use crate::groups::Group;
use crate::model::{Classification, MsgId};

/// A flight may still pick up passengers until this fraction of its path.
const BOARDING: f32 = 0.35;
const MAX_FLIGHTS: usize = 3;
const MAX_LIFT: f32 = 3.0;
/// Trail length as a fraction of the path.
const TRAIL: f32 = 0.15;
const LIFT: f32 = 0.35;
const PULSE: f32 = 0.6;

struct Flight {
    group: Group,
    from: (f32, f32),
    ctrl: (f32, f32),
    to: (f32, f32),
    t: f32,
    dur: f32,
    count: u32,
}

impl Flight {
    fn progress(&self) -> f32 {
        (self.t / self.dur).clamp(0.0, 1.0)
    }

    fn at(&self, p: f32) -> (i32, i32) {
        let (x, y) = bezier(self.from, self.ctrl, self.to, ease(Interpolation::CubicInOut, p));
        (x.round() as i32, y.round() as i32)
    }
}

pub struct Machine {
    flights: Vec<Flight>,
    lifts: HashMap<MsgId, (Group, f32)>,
    pulses: [(f32, u32); 11],
    to_flash: Vec<Group>,
    fx: EffectManager<u32>,
    pending_dt: f32,
}

impl Machine {
    pub fn new() -> Self {
        Machine {
            flights: Vec::new(),
            lifts: HashMap::new(),
            pulses: [(0.0, 0); 11],
            to_flash: Vec::new(),
            fx: EffectManager::default(),
            pending_dt: 0.0,
        }
    }

    fn land(&mut self, g: Group, count: u32) {
        let p = &mut self.pulses[g.index()];
        *p = (PULSE, p.1 + count);
        self.to_flash.push(g);
    }
}

fn superscript(n: u32) -> char {
    match n {
        2 => '²',
        3 => '³',
        4..=9 => ['⁴', '⁵', '⁶', '⁷', '⁸', '⁹'][n as usize - 4],
        _ => '⁺',
    }
}

impl AnimationStyle for Machine {
    fn on_result(&mut self, c: &Classification, ctx: &Ctx) {
        let geo = ctx.geo;
        let Some(bin) = geo.bins[c.group.index()] else { return };
        let row = geo.rows.get(&c.id);
        if row.is_some() {
            self.lifts.insert(c.id, (c.group, LIFT));
        }
        if let Some(f) = self.flights.iter_mut().find(|f| f.group == c.group && f.progress() < BOARDING) {
            f.count += 1;
            return;
        }
        if geo.compact || self.flights.len() >= MAX_FLIGHTS {
            self.land(c.group, 1);
            return;
        }
        let from = match row {
            Some(r) => (r.tag.0 as f32, r.y as f32),
            None => (geo.list.x as f32 + geo.list.width as f32 / 2.0, geo.list.bottom() as f32 - 1.0),
        };
        let to = (bin.x as f32 + 1.0, bin.y as f32);
        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
        let dist = (dx * dx + 4.0 * dy * dy).sqrt();
        // The curve lies inside the hull of its three points, so clamping the
        // control point to the stage keeps the whole flight on stage.
        let lift = (1.0 + dist * 0.03).min(MAX_LIFT);
        let ctrl = (from.0 + dx * 0.45, (from.1.min(to.1) - lift).max(geo.stage.y as f32));
        let dur = 0.55 + dist / 260.0;
        self.flights.push(Flight { group: c.group, from, ctrl, to, t: 0.0, dur, count: 1 });
    }

    fn update(&mut self, dt: f32) {
        self.pending_dt += dt;
        for f in self.flights.iter_mut() {
            f.t += dt;
        }
        let (done, flying): (Vec<_>, Vec<_>) = self.flights.drain(..).partition(|f| f.t >= f.dur);
        self.flights = flying;
        for f in done {
            self.land(f.group, f.count);
        }
        self.lifts.retain(|_, (_, t)| {
            *t -= dt;
            *t > 0.0
        });
        for p in self.pulses.iter_mut() {
            p.0 = (p.0 - dt).max(0.0);
            if p.0 == 0.0 {
                p.1 = 0;
            }
        }
    }

    fn overlay(&mut self, buf: &mut Buffer, ctx: &Ctx) {
        let geo = ctx.geo;
        for (id, (g, t)) in &self.lifts {
            if let Some(row) = geo.rows.get(id) {
                tint_bg(buf, row.rect(), g.color(), 0.25 * (t / LIFT));
            }
        }

        // Bin flash (tachyonfx) and "+n" counter pop.
        let dt = std::time::Duration::from_secs_f32(self.pending_dt);
        self.pending_dt = 0.0;
        for g in self.to_flash.drain(..) {
            if let Some(bin) = geo.bins[g.index()] {
                let flash = fx::fade_from(Color::White, mix(theme::PANEL, g.color(), 0.65), (450, Interpolation::QuadOut));
                self.fx.add_effect(flash.with_area(bin));
            }
        }
        self.fx.process_effects(dt, buf, geo.area);
        for g in Group::ALL {
            let (t, n) = self.pulses[g.index()];
            let (Some((cx, cy)), Some(bin)) = (geo.counters[g.index()], geo.bins[g.index()]) else { continue };
            if t <= 0.0 {
                continue;
            }
            let pop = mix(g.color(), Color::White, t / PULSE);
            if !geo.compact {
                put_str(buf, cx as i32 - 4, cy as i32, &format!("{:>3}", format!("+{n}")), Style::new().fg(pop), 3);
            }
            put(buf, bin.x as i32 + 1, bin.y as i32, g.glyph(), pop, None, true);
        }

        // Chips with a continuous trail that fades towards its tail.
        for f in &self.flights {
            let p = f.progress();
            let mut cells: Vec<(i32, i32, f32)> = Vec::new();
            let steps = 96;
            for i in (1..=steps).rev() {
                let back = TRAIL * i as f32 / steps as f32;
                if p <= back {
                    continue;
                }
                let (x, y) = f.at(p - back);
                let k = 1.0 - back / TRAIL;
                match cells.iter_mut().find(|c| c.0 == x && c.1 == y) {
                    Some(c) => c.2 = c.2.max(k),
                    None => cells.push((x, y, k)),
                }
            }
            for (x, y, k) in cells {
                if x >= 0 && y >= 0 {
                    let level = (k * 3.0).ceil() / 3.0; // three shades: fewer colour changes
                    tint_bg(buf, Rect::new(x as u16, y as u16, 1, 1), f.group.color(), 0.45 * level);
                }
            }
            let (x, y) = f.at(p);
            let chip = f.group.color();
            let under = bg_at(buf, x.max(0) as u16, y.max(0) as u16);
            put(buf, x - 1, y, '▐', chip, Some(under), false);
            put(buf, x, y, f.group.glyph(), Color::White, Some(chip), true);
            put(buf, x + 1, y, '▌', chip, Some(under), false);
            if f.count > 1 {
                put(buf, x + 2, y, superscript(f.count), Color::White, Some(under), true);
            }
        }
    }

    fn next_frame(&self, _ctx: &Ctx) -> Option<f32> {
        let busy = !self.flights.is_empty()
            || !self.lifts.is_empty()
            || !self.to_flash.is_empty()
            || self.pulses.iter().any(|p| p.0 > 0.0)
            || self.fx.is_running();
        busy.then_some(ASAP)
    }

    fn backlog(&self) -> usize {
        self.flights.len()
    }

    fn finish_all(&mut self) {
        self.flights.clear();
        self.lifts.clear();
        self.pulses = [(0.0, 0); 11];
        self.to_flash.clear();
        self.fx = EffectManager::default();
    }
}
