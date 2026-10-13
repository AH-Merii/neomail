//! Inbox model and the classification event contract.
//!
//! Events are applied to the model immediately. The animation layer only
//! observes the same events; it never gates or delays model updates.

use crate::groups::{Account, Group};

pub type MsgId = u32;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum JevState {
    Pending,
    Thinking,
    Sorted { group: Group, confidence: f32 },
}

#[derive(Clone, Debug)]
pub struct Message {
    pub id: MsgId,
    pub account: Account,
    pub from: &'static str,
    pub subject: &'static str,
    pub time: String,
    pub unread: bool,
    pub state: JevState,
}

impl Message {
    pub fn group(&self) -> Option<Group> {
        match self.state {
            JevState::Sorted { group, .. } => Some(group),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Classification {
    pub id: MsgId,
    pub group: Group,
    pub confidence: f32,
    /// Probability per group, indexed by `Group::index` (unused by the
    /// prototype UI; kept so the event matches what the real jev reports).
    #[allow(dead_code)]
    pub probs: [f32; 11],
    pub latency_ms: u64,
}

/// Events emitted by jev (real or simulated).
#[derive(Clone, Debug)]
pub enum JevEvent {
    /// New mail synced into the unified inbox.
    Arrived(Message),
    /// jev started reading a message.
    Started(MsgId),
    /// jev finished: final group and confidence.
    Classified(Classification),
}

#[derive(Default)]
pub struct Model {
    /// Unified inbox, newest first.
    pub messages: Vec<Message>,
    pub counts: [u32; 11],
    pub sorted: usize,
    pub latency_total_ms: u64,
    pub last: Option<Classification>,
}

impl Model {
    pub fn new(initial: Vec<Message>) -> Self {
        Model { messages: initial, ..Default::default() }
    }

    pub fn apply(&mut self, ev: &JevEvent) {
        match ev {
            JevEvent::Arrived(msg) => self.messages.insert(0, msg.clone()),
            JevEvent::Started(id) => {
                if let Some(m) = self.get_mut(*id) {
                    if m.state == JevState::Pending {
                        m.state = JevState::Thinking;
                    }
                }
            }
            JevEvent::Classified(c) => {
                let Some(m) = self.messages.iter_mut().find(|m| m.id == c.id) else { return };
                let old = m.group();
                m.state = JevState::Sorted { group: c.group, confidence: c.confidence };
                if let Some(old) = old {
                    self.counts[old.index()] -= 1;
                    self.sorted -= 1;
                }
                self.counts[c.group.index()] += 1;
                self.sorted += 1;
                self.latency_total_ms += c.latency_ms;
                self.last = Some(c.clone());
            }
        }
    }

    pub fn get(&self, id: MsgId) -> Option<&Message> {
        self.messages.iter().find(|m| m.id == id)
    }

    fn get_mut(&mut self, id: MsgId) -> Option<&mut Message> {
        self.messages.iter_mut().find(|m| m.id == id)
    }

    pub fn unread(&self, account: Account) -> usize {
        self.messages.iter().filter(|m| m.account == account && m.unread).count()
    }

    pub fn avg_latency_ms(&self) -> u64 {
        if self.sorted == 0 { 0 } else { self.latency_total_ms / self.sorted as u64 }
    }
}
