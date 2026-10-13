//! Reduced motion: a brief, static highlight of the sorted row and its bin.
//! Nothing moves, fades or flickers; the highlight simply switches off, so
//! it costs two frames (on, off) and one timed wake-up per result.
//!
//! Full motion also uses the bin highlight in bulk mode (long syncs), where
//! only counters are animated.

use std::collections::HashMap;

use ratatui::buffer::Buffer;

use super::util::tint_bg;
use super::{AnimationStyle, Ctx, sooner};
use crate::groups::Group;
use crate::model::{Classification, MsgId};

/// Real seconds a highlight stays on.
const HOLD: f32 = 0.9;

#[derive(Default)]
pub struct Reduced {
    rows: HashMap<MsgId, (Group, f32)>,
    bins: [f32; 11],
}

impl Reduced {
    /// Counter-only highlight (bulk mode).
    pub fn pulse_bin(&mut self, g: Group) {
        self.bins[g.index()] = HOLD;
    }
}

impl AnimationStyle for Reduced {
    fn on_result(&mut self, c: &Classification, _ctx: &Ctx) {
        self.rows.insert(c.id, (c.group, HOLD));
        self.pulse_bin(c.group);
    }

    /// `dt` is real time here: speed does not stretch a static highlight.
    fn update(&mut self, dt: f32) {
        self.rows.retain(|_, (_, t)| {
            *t -= dt;
            *t > 0.0
        });
        for b in self.bins.iter_mut() {
            *b = (*b - dt).max(0.0);
        }
    }

    fn overlay(&mut self, buf: &mut Buffer, ctx: &Ctx) {
        for (id, (group, _)) in &self.rows {
            if let Some(row) = ctx.geo.rows.get(id) {
                tint_bg(buf, row.rect(), group.color(), 0.22);
            }
        }
        for g in Group::ALL {
            if self.bins[g.index()] > 0.0 {
                if let Some(rect) = ctx.geo.bins[g.index()] {
                    tint_bg(buf, rect, g.color(), 0.3);
                }
            }
        }
    }

    fn next_frame(&self, _ctx: &Ctx) -> Option<f32> {
        let rows = self.rows.values().map(|(_, t)| *t).reduce(f32::min);
        let bins = self.bins.iter().copied().filter(|&t| t > 0.0).reduce(f32::min);
        sooner(rows, bins)
    }

    fn backlog(&self) -> usize {
        0
    }

    fn finish_all(&mut self) {
        self.rows.clear();
        self.bins = [0.0; 11];
    }
}
