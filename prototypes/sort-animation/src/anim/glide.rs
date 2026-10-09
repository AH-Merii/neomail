//! Grouped-view presentation: batched section changes and row glides.
//!
//! Section membership is committed in batches (at most every `BATCH`
//! seconds), so a burst of results moves rows once, together, instead of
//! reflowing the list on every result. Rows then glide to their new places.

use std::collections::HashMap;

use super::ASAP;
use crate::groups::Group;
use crate::model::{Model, MsgId};

/// Section changes are committed at most this often (seconds).
const BATCH: f32 = 0.6;
/// Glide smoothing rate (per second).
const GLIDE_RATE: f32 = 10.0;

#[derive(Default)]
pub struct Glide {
    sections: HashMap<MsgId, Option<Group>>,
    last_commit: Option<f32>,
    /// Seconds until a waiting batch is committed.
    wait: Option<f32>,
    ys: HashMap<MsgId, f32>,
    pending_dt: f32,
    moving: bool,
}

impl Glide {
    pub fn reset(&mut self) {
        self.ys.clear();
        self.moving = false;
        self.last_commit = None;
    }

    pub fn refresh_sections(&mut self, model: &Model, clock: f32, immediate: bool) {
        let changed = self.sections.len() != model.messages.len()
            || model.messages.iter().any(|m| self.sections.get(&m.id) != Some(&m.group()));
        if !changed {
            self.wait = None;
            return;
        }
        let since = self.last_commit.map_or(f32::INFINITY, |t| clock - t);
        if immediate || since >= BATCH {
            self.sections = model.messages.iter().map(|m| (m.id, m.group())).collect();
            self.last_commit = Some(clock);
            self.wait = None;
        } else {
            self.wait = Some(BATCH - since);
        }
    }

    /// Presented section (`None` = "jev is sorting").
    pub fn section(&self, id: MsgId) -> Option<Group> {
        self.sections.get(&id).copied().flatten()
    }

    pub fn update(&mut self, real_dt: f32) {
        self.pending_dt += real_dt;
    }

    pub fn next_frame(&self) -> Option<f32> {
        if self.moving { Some(ASAP) } else { self.wait }
    }

    /// Eases each row from its previous presented y towards its target.
    pub fn place(&mut self, rows: &mut [(MsgId, f32)]) {
        let k = 1.0 - (-GLIDE_RATE * self.pending_dt).exp();
        self.pending_dt = 0.0;
        let mut next = HashMap::with_capacity(rows.len());
        self.moving = false;
        for (id, y) in rows.iter_mut() {
            let shown = match self.ys.get(id) {
                Some(&prev) if (prev - *y).abs() > 0.05 => {
                    self.moving = true;
                    prev + (*y - prev) * k
                }
                _ => *y,
            };
            next.insert(*id, shown);
            *y = shown;
        }
        self.ys = next;
    }
}
