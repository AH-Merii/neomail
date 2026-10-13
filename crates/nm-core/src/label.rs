//! The names groups get on the server: an Outlook category now, a Gmail or Proton label later.
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
}

impl LabelPrefix {
    pub fn new(prefix: &str) -> Result<Self, BadPrefix> {
        if prefix.trim().is_empty() {
            return Err(BadPrefix::Empty);
        }
        if prefix.chars().any(char::is_control) {
            return Err(BadPrefix::Control);
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
        let custom = LabelPrefix::new("neo/").unwrap();
        assert_eq!(custom.name(Group::Spam), "neo/spam");
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
}
