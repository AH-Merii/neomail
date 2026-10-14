//! The groups classification sorts mail into (the decisions table under "Classification" in
//! `docs/decisions.md`), what the classifier may choose, and the one rule that turns a stored
//! choice into the group a message is listed under: [`GroupRules::effective`].

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, TimeDelta, Utc};

/// A group a message is listed under. Each message belongs to exactly one, or to none until it
/// is classified.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Group {
    Main,
    FollowUp,
    FollowUpOld,
    Recruitment,
    ReceiptsInvoices,
    OrdersShipping,
    Newsletters,
    Promotions,
    Notifications,
    Spam,
    Other,
}

impl Group {
    /// Every group, in the decisions table's order.
    pub const ALL: [Group; 11] = [
        Group::Main,
        Group::FollowUp,
        Group::FollowUpOld,
        Group::Recruitment,
        Group::ReceiptsInvoices,
        Group::OrdersShipping,
        Group::Newsletters,
        Group::Promotions,
        Group::Notifications,
        Group::Spam,
        Group::Other,
    ];

    /// The group's stable name, as printed, filtered on and used in JSON output.
    pub fn name(self) -> &'static str {
        match self {
            Group::Main => "main",
            Group::FollowUp => "follow-up",
            Group::FollowUpOld => "follow-up-old",
            Group::Recruitment => "recruitment",
            Group::ReceiptsInvoices => "receipts-invoices",
            Group::OrdersShipping => "orders-shipping",
            Group::Newsletters => "newsletters",
            Group::Promotions => "promotions",
            Group::Notifications => "notifications",
            Group::Spam => "spam",
            Group::Other => "other",
        }
    }
}

impl fmt::Display for Group {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A name that is not a group's.
#[derive(Debug, thiserror::Error)]
#[error("unknown group {0:?}")]
pub struct UnknownGroup(pub String);

impl FromStr for Group {
    type Err = UnknownGroup;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Group::ALL
            .into_iter()
            .find(|g| g.name() == s)
            .ok_or_else(|| UnknownGroup(s.to_owned()))
    }
}

/// What the classifier may answer: every group but `follow-up-old`, which comes from age.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Choice {
    Main,
    FollowUp,
    Recruitment,
    ReceiptsInvoices,
    OrdersShipping,
    Newsletters,
    Promotions,
    Notifications,
    Spam,
    Other,
}

impl Choice {
    /// Every choice, in the fixed order the request offers them (`main` first).
    pub const ALL: [Choice; 10] = [
        Choice::Main,
        Choice::FollowUp,
        Choice::Recruitment,
        Choice::ReceiptsInvoices,
        Choice::OrdersShipping,
        Choice::Newsletters,
        Choice::Promotions,
        Choice::Notifications,
        Choice::Spam,
        Choice::Other,
    ];

    /// The group of the same name.
    pub fn group(self) -> Group {
        match self {
            Choice::Main => Group::Main,
            Choice::FollowUp => Group::FollowUp,
            Choice::Recruitment => Group::Recruitment,
            Choice::ReceiptsInvoices => Group::ReceiptsInvoices,
            Choice::OrdersShipping => Group::OrdersShipping,
            Choice::Newsletters => Group::Newsletters,
            Choice::Promotions => Group::Promotions,
            Choice::Notifications => Group::Notifications,
            Choice::Spam => Group::Spam,
            Choice::Other => Group::Other,
        }
    }

    pub fn name(self) -> &'static str {
        self.group().name()
    }

    /// The choice named `name`, or `None`.
    pub fn from_name(name: &str) -> Option<Self> {
        Choice::ALL.into_iter().find(|c| c.name() == name)
    }
}

impl fmt::Display for Choice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The thresholds that turn a stored choice into a [`Group`]; the config's `[classify]` table.
/// Changing them needs no new answer from the classifier.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroupRules {
    /// Below this confidence a choice other than `spam` lists as `other`.
    pub other_below: f64,
    /// A `follow-up` received more than this many days ago lists as `follow-up-old`.
    pub follow_up_old_days: u32,
    /// Spam at or above this confidence moves to the account's Junk folder; `None` moves
    /// nothing (`[classify] junk = false`). The decisions' "Spam" bullet: 0.9 by default.
    pub junk_at: Option<f64>,
}

impl Default for GroupRules {
    /// `other_below` 0.4 is provisional until the live check; 90 days is the decisions table's
    /// "more than 3 months".
    fn default() -> Self {
        GroupRules {
            other_below: 0.4,
            follow_up_old_days: 90,
            junk_at: Some(0.9),
        }
    }
}

impl GroupRules {
    /// The group a message lists under: `spam` when the choice is spam, whatever the confidence;
    /// else `other` when the confidence is below [`other_below`](Self::other_below); else
    /// `follow-up-old` for a `follow-up` received more than
    /// [`follow_up_old_days`](Self::follow_up_old_days) before `now`; else the choice. A message
    /// with no arrival time is never old.
    pub fn effective(
        &self,
        choice: Choice,
        confidence: f64,
        received: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> Group {
        if choice == Choice::Spam {
            return Group::Spam;
        }
        if confidence < self.other_below {
            return Group::Other;
        }
        let old =
            received.is_some_and(|r| now - r > TimeDelta::days(i64::from(self.follow_up_old_days)));
        if choice == Choice::FollowUp && old {
            return Group::FollowUpOld;
        }
        choice.group()
    }

    /// Whether a message classified `choice` at `confidence` moves to Junk: spam at or above
    /// [`junk_at`](Self::junk_at), and nothing else. Lower-confidence spam stays where it is,
    /// labelled `spam`.
    pub fn junk(&self, choice: Choice, confidence: f64) -> bool {
        choice == Choice::Spam && self.junk_at.is_some_and(|at| confidence >= at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(day: i64) -> Option<DateTime<Utc>> {
        DateTime::from_timestamp(day * 86_400, 0)
    }

    #[test]
    fn names_round_trip_and_unknown_names_are_refused() {
        let names: Vec<&str> = Group::ALL.iter().map(|g| g.name()).collect();
        assert_eq!(
            names,
            [
                "main",
                "follow-up",
                "follow-up-old",
                "recruitment",
                "receipts-invoices",
                "orders-shipping",
                "newsletters",
                "promotions",
                "notifications",
                "spam",
                "other"
            ]
        );
        for g in Group::ALL {
            assert_eq!(g.name().parse::<Group>().unwrap(), g);
        }
        assert!("Main".parse::<Group>().is_err());
        assert!("unclassified".parse::<Group>().is_err());
        assert_eq!(Choice::from_name("follow-up-old"), None);
    }

    #[test]
    fn spam_wins_then_low_confidence_is_other_then_old_follow_up_is_follow_up_old() {
        let rules = GroupRules::default();
        let now = at(1000).unwrap();
        let cases = [
            (Choice::Spam, 0.1, at(0), Group::Spam),
            (Choice::Main, 0.39, at(999), Group::Other),
            (Choice::FollowUp, 0.2, at(0), Group::Other),
            (Choice::Main, 0.4, at(999), Group::Main),
            (Choice::FollowUp, 0.9, at(1000 - 91), Group::FollowUpOld),
            (Choice::FollowUp, 0.9, at(1000 - 90), Group::FollowUp),
            (Choice::FollowUp, 0.9, None, Group::FollowUp),
            (Choice::Newsletters, 0.9, at(0), Group::Newsletters),
        ];
        for (choice, confidence, received, want) in cases {
            assert_eq!(
                rules.effective(choice, confidence, received, now),
                want,
                "{choice} {confidence} {received:?}"
            );
        }
        let strict = GroupRules {
            other_below: 0.95,
            follow_up_old_days: 10,
            ..GroupRules::default()
        };
        assert_eq!(
            strict.effective(Choice::Main, 0.9, at(999), now),
            Group::Other
        );
        assert_eq!(
            strict.effective(Choice::FollowUp, 0.99, at(980), now),
            Group::FollowUpOld
        );
    }

    #[test]
    fn only_spam_at_or_above_junk_at_moves_to_junk() {
        let rules = GroupRules::default();
        assert!(!rules.junk(Choice::Spam, 0.89));
        assert!(rules.junk(Choice::Spam, 0.9));
        assert!(rules.junk(Choice::Spam, 1.0));
        for choice in Choice::ALL.into_iter().filter(|c| *c != Choice::Spam) {
            assert!(!rules.junk(choice, 1.0), "{choice:?}");
        }
        let off = GroupRules {
            junk_at: None,
            ..rules
        };
        assert!(!off.junk(Choice::Spam, 1.0), "junk = false moves nothing");
    }
}
