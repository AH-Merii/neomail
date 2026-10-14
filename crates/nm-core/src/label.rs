//! The names groups get on the server: an Outlook category, a Gmail label or a Proton label.
//! [`LabelPrefix`] is the one owner of which names are neomail's (DEC-2026-10-13-labels,
//! DEC-2026-10-13-categories): the prefix followed by one of the eleven group names, ignoring
//! ASCII case. Every other name is the user's and is never changed. Inside neomail the prefix is
//! never shown: lists and the TUI show the bare group.

use std::fmt;

use crate::group::Group;

/// The prefix of a group's name on the server: `[classify] label_prefix`, `nm-` by default. Never
/// empty, so a user's own category called `main` is never taken for neomail's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LabelPrefix(String);

/// A prefix that cannot be used.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BadPrefix {
    #[error("must not be empty")]
    Empty,
    #[error("must not hold control characters")]
    Control,
    /// Gmail and Bridge both name nested labels with `/`: a prefix holding one would put
    /// neomail's labels under a parent label.
    #[error("must not hold /")]
    Slash,
}

impl LabelPrefix {
    pub fn new(prefix: &str) -> Result<Self, BadPrefix> {
        if prefix.trim().is_empty() {
            return Err(BadPrefix::Empty);
        }
        if prefix.chars().any(char::is_control) {
            return Err(BadPrefix::Control);
        }
        if prefix.contains('/') {
            return Err(BadPrefix::Slash);
        }
        Ok(Self(prefix.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The name `group` gets on the server: `nm-receipts-invoices`.
    pub fn name(&self, group: Group) -> String {
        format!("{}{}", self.0, group.name())
    }

    /// The group whose name `name` is, ignoring ASCII case, or `None` when the name is not
    /// neomail's.
    pub fn owns(&self, name: &str) -> Option<Group> {
        Group::ALL
            .into_iter()
            .find(|g| self.name(*g).eq_ignore_ascii_case(name))
    }
}

impl Default for LabelPrefix {
    fn default() -> Self {
        Self("nm-".to_owned())
    }
}

impl fmt::Display for LabelPrefix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A message's names after neomail sets `want` on it, or `None` when `current` needs no change.
/// Names neomail owns ([`LabelPrefix::owns`], or exactly `last`, what it wrote before) go, apart
/// from the first that is exactly `want`; every other name stays, in its order. `want` is added
/// at the end when it is not there.
pub fn merge(
    current: &[String],
    prefix: &LabelPrefix,
    last: Option<&str>,
    want: &str,
) -> Option<Vec<String>> {
    let mut kept_want = false;
    let mut merged: Vec<String> = Vec::with_capacity(current.len() + 1);
    for name in current {
        if name == want && !kept_want {
            kept_want = true;
            merged.push(name.clone());
        } else if prefix.owns(name).is_none() && Some(name.as_str()) != last {
            merged.push(name.clone());
        }
    }
    if !kept_want {
        merged.push(want.to_owned());
    }
    (merged != current).then_some(merged)
}

/// What a Gmail message's labels need, as single names to add and remove: Gmail's IMAP adds
/// and removes names one by one, so no STORE ever replaces the message's whole set
/// (DEC-2026-10-14-labels-junk). The names are [`merge`]'s: what neomail owns goes, apart from
/// the first that is exactly `want`, and `want` is added when it is missing. Every other name,
/// the system labels (`\Inbox`) included, is never named.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LabelPlan {
    pub add: Option<String>,
    pub remove: Vec<String>,
}

impl LabelPlan {
    /// Whether the message already carries what it should.
    pub fn is_empty(&self) -> bool {
        self.add.is_none() && self.remove.is_empty()
    }
}

/// The [`LabelPlan`] that turns `current` into what [`merge`] would give.
pub fn plan(current: &[String], prefix: &LabelPrefix, last: Option<&str>, want: &str) -> LabelPlan {
    let mut plan = LabelPlan::default();
    let mut kept_want = false;
    for name in current {
        if name == want && !kept_want {
            kept_want = true;
        } else if (prefix.owns(name).is_some() || Some(name.as_str()) == last)
            && !plan.remove.contains(name)
        {
            plan.remove.push(name.clone());
        }
    }
    if !kept_want {
        plan.add = Some(want.to_owned());
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn every_group_has_a_name_that_maps_back_to_it_ignoring_ascii_case() {
        let prefix = LabelPrefix::default();
        assert_eq!(prefix.name(Group::ReceiptsInvoices), "nm-receipts-invoices");
        assert_eq!(prefix.owns("NM-Follow-Up-Old"), Some(Group::FollowUpOld));
        for g in Group::ALL {
            assert_eq!(prefix.owns(&prefix.name(g)), Some(g));
            assert_eq!(prefix.owns(&prefix.name(g).to_uppercase()), Some(g));
        }
        let custom = LabelPrefix::new("neo.").unwrap();
        assert_eq!(custom.name(Group::Spam), "neo.spam");
        assert_eq!(custom.owns("nm-spam"), None, "another prefix's name");
    }

    #[test]
    fn names_that_are_not_the_prefix_and_a_group_are_the_users() {
        let prefix = LabelPrefix::default();
        for name in [
            "nm-project",
            "main",
            "Spam",
            "nm-",
            "nm-main ",
            "xnm-main",
            "",
        ] {
            assert_eq!(prefix.owns(name), None, "{name:?}");
        }
    }

    #[test]
    fn a_prefix_is_never_empty_or_holds_control_characters() {
        assert_eq!(LabelPrefix::new(""), Err(BadPrefix::Empty));
        assert_eq!(LabelPrefix::new("  "), Err(BadPrefix::Empty));
        assert_eq!(LabelPrefix::new("nm\n"), Err(BadPrefix::Control));
        assert_eq!(LabelPrefix::new("nm/"), Err(BadPrefix::Slash));
        assert_eq!(LabelPrefix::new("a/nm-"), Err(BadPrefix::Slash));
        assert_eq!(LabelPrefix::new("nm-").unwrap(), LabelPrefix::default());
    }

    #[test]
    fn merging_keeps_the_users_names_in_order_and_swaps_neomails() {
        let prefix = LabelPrefix::default();
        // (current, last, want, merged)
        type Case<'a> = (
            &'a [&'a str],
            Option<&'a str>,
            &'a str,
            Option<&'a [&'a str]>,
        );
        let cases: [Case<'_>; 8] = [
            // An empty list gets the name.
            (&[], None, "nm-main", Some(&["nm-main"])),
            // The user's names stay, their own nm- name included.
            (
                &["Red", "nm-project", "Blue"],
                None,
                "nm-spam",
                Some(&["Red", "nm-project", "Blue", "nm-spam"]),
            ),
            // The old group's name goes, wherever it is.
            (
                &["nm-follow-up", "Red"],
                Some("nm-follow-up"),
                "nm-follow-up-old",
                Some(&["Red", "nm-follow-up-old"]),
            ),
            // Already right, wherever it is: no change.
            (&["nm-main", "Red"], Some("nm-main"), "nm-main", None),
            (&["Red", "nm-main"], None, "nm-main", None),
            // Two of neomail's names from a hand edit collapse to the wanted one.
            (
                &["nm-main", "Red", "NM-SPAM"],
                Some("nm-main"),
                "nm-spam",
                Some(&["Red", "nm-spam"]),
            ),
            // What neomail wrote under an old prefix is its own too.
            (
                &["old-main", "Red"],
                Some("old-main"),
                "nm-main",
                Some(&["Red", "nm-main"]),
            ),
            // A duplicate of the wanted name goes.
            (&["nm-main", "nm-main"], None, "nm-main", Some(&["nm-main"])),
        ];
        for (current, last, want, merged) in cases {
            assert_eq!(
                merge(&names(current), &prefix, last, want),
                merged.map(names),
                "{current:?} {last:?} {want}"
            );
        }
    }
    #[test]
    fn a_plan_names_only_neomails_labels_and_gives_what_merging_gives() {
        let prefix = LabelPrefix::default();
        // (current, last, want)
        let cases: [(&[&str], Option<&str>, &str); 6] = [
            (&["\\Inbox", "\\Important"], None, "nm-main"),
            (
                &["\\Inbox", "Red", "nm-follow-up"],
                Some("nm-follow-up"),
                "nm-follow-up-old",
            ),
            (&["nm-main", "Red"], Some("nm-main"), "nm-main"),
            (&["nm-main", "NM-SPAM", "nm-main"], None, "nm-spam"),
            (&["old-main", "nm-project"], Some("old-main"), "nm-main"),
            (&["nm-project"], None, "nm-spam"),
        ];
        for (current, last, want) in cases {
            let current = names(current);
            let plan = plan(&current, &prefix, last, want);
            for name in plan.remove.iter().chain(&plan.add) {
                let users = prefix.owns(name).is_none() && Some(name.as_str()) != last;
                assert!(!users || name == want, "named the user's {name:?}");
            }
            let mut applied: Vec<String> = current
                .iter()
                .filter(|n| !plan.remove.contains(n))
                .cloned()
                .collect();
            applied.extend(plan.add.clone());
            applied.dedup();
            let merged = merge(&current, &prefix, last, want).unwrap_or(current.clone());
            let set = |v: &[String]| v.iter().cloned().collect::<std::collections::BTreeSet<_>>();
            assert_eq!(set(&applied), set(&merged), "{current:?} {last:?} {want}");
            assert_eq!(
                plan.is_empty(),
                set(&merged) == set(&current),
                "{current:?}"
            );
        }
    }
}
