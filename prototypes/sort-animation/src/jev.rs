//! Simulated jev classifier stream.
//!
//! The whole timeline (arrivals, starts, results) is precomputed from a seed so
//! a run is reproducible. Latencies are 80-600 ms per message, with a small
//! worker pool that widens when a backlog builds up (sync bursts).

use std::collections::VecDeque;

use crate::fixtures::{ARRIVALS, Fixture, INITIAL};
use crate::groups::{Group, Rng};
use crate::model::{Classification, JevEvent, JevState, Message, Model, MsgId};

/// jev waits this long after launch before it starts reading.
const WARMUP_MS: u64 = 900;
/// Results with lower confidence fall back to `other`.
const MIN_CONFIDENCE: f32 = 0.55;

pub struct JevSim {
    events: Vec<(u64, JevEvent)>,
    cursor: usize,
    pub total: usize,
}

fn message(id: MsgId, f: &Fixture, time: String, unread: bool) -> Message {
    Message {
        id,
        account: f.account,
        from: f.from,
        subject: f.subject,
        time,
        unread,
        state: JevState::Pending,
    }
}

fn classify(rng: &mut Rng, id: MsgId, f: &Fixture, latency_ms: u64) -> Classification {
    let mut logits = [0f32; 11];
    for l in logits.iter_mut() {
        *l = rng.f32();
    }
    logits[f.hint.index()] += f.signal * 1.6;
    let max = logits.iter().cloned().fold(f32::MIN, f32::max);
    let mut probs = logits.map(|l| (l - max).exp());
    let sum: f32 = probs.iter().sum();
    probs.iter_mut().for_each(|p| *p /= sum);
    let (best, conf) = probs
        .iter()
        .enumerate()
        .fold((0, 0f32), |acc, (i, &p)| if p > acc.1 { (i, p) } else { acc });
    let group = if conf < MIN_CONFIDENCE { Group::Other } else { Group::ALL[best] };
    Classification { id, group, confidence: conf, probs, latency_ms }
}

impl JevSim {
    /// Builds the initial inbox and the precomputed event timeline.
    pub fn new(seed: u64) -> (Model, JevSim) {
        let mut rng = Rng::new(seed);
        let mut fixtures: Vec<&Fixture> = Vec::new();
        let mut initial = Vec::new();
        let mut minute = 9 * 60 + 58;
        for f in INITIAL {
            let id = fixtures.len() as MsgId;
            fixtures.push(f);
            let time = format!("{:02}:{:02}", minute / 60, minute % 60);
            initial.push(message(id, f, time, rng.f32() < 0.55));
            minute -= rng.range(3, 17) as i32;
        }

        // Discrete-event simulation of the worker pool.
        let mut events: Vec<(u64, JevEvent)> = Vec::new();
        let mut queue: VecDeque<MsgId> = (0..INITIAL.len() as MsgId).collect();
        let mut workers: Vec<(u64, MsgId)> = Vec::new(); // (finish time, id)
        let mut next_arrival = 0usize;
        let mut t = WARMUP_MS;
        loop {
            // Arrivals due now.
            while next_arrival < ARRIVALS.len() && ARRIVALS[next_arrival].0 <= t {
                let (delay, f) = &ARRIVALS[next_arrival];
                let id = fixtures.len() as MsgId;
                fixtures.push(f);
                let time = format!("10:{:02}", 1 + next_arrival / 2);
                events.push((*delay, JevEvent::Arrived(message(id, f, time, true))));
                queue.push_back(id);
                next_arrival += 1;
            }
            // Finished jobs.
            workers.retain(|&(done, id)| {
                if done <= t {
                    let c = classify(&mut rng, id, fixtures[id as usize], 0);
                    events.push((done, JevEvent::Classified(c)));
                    false
                } else {
                    true
                }
            });
            // Hand out work: 2 workers normally, up to 6 under a backlog.
            let limit = if queue.len() > 4 { 6 } else { 2 };
            while workers.len() < limit {
                let Some(id) = queue.pop_front() else { break };
                let latency = rng.range(80, 600);
                events.push((t, JevEvent::Started(id)));
                workers.push((t + latency, id));
            }
            let next_done = workers.iter().map(|w| w.0).min();
            let next_arr = ARRIVALS.get(next_arrival).map(|a| a.0.max(t));
            t = match (next_done, next_arr) {
                (None, None) => break,
                (Some(a), None) | (None, Some(a)) => a,
                (Some(a), Some(b)) => a.min(b),
            };
        }
        // Fill in latencies (start -> result) and order the timeline.
        let mut started = vec![0u64; fixtures.len()];
        for (at, ev) in events.iter_mut() {
            match ev {
                JevEvent::Started(id) => started[*id as usize] = *at,
                JevEvent::Classified(c) => c.latency_ms = *at - started[c.id as usize],
                _ => {}
            }
        }
        events.sort_by_key(|(at, ev)| (*at, !matches!(ev, JevEvent::Arrived(_))));
        let total = fixtures.len();
        (Model::new(initial), JevSim { events, cursor: 0, total })
    }

    /// Returns all events due at `elapsed_ms` (simulation time).
    pub fn poll(&mut self, elapsed_ms: u64) -> Vec<JevEvent> {
        let start = self.cursor;
        while self.cursor < self.events.len() && self.events[self.cursor].0 <= elapsed_ms {
            self.cursor += 1;
        }
        self.events[start..self.cursor].iter().map(|(_, e)| e.clone()).collect()
    }

    pub fn next_due_ms(&self) -> Option<u64> {
        self.events.get(self.cursor).map(|(at, _)| *at)
    }

    pub fn finished(&self) -> bool {
        self.cursor >= self.events.len()
    }
}
