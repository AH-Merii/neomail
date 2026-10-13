//! Rules that hold over a table of many inputs, and the excerpt's cut. Every example is made up.

use nm_redact::{EXCERPT_CHARS, Excerpt, Known, Piece, redact};

const NUMBERS: &[&str] = &[
    "1234",
    "98765",
    "0000",
    "4111111111111111",
    "12345678901234567890",
    "١٢٣٤٥",
    "20260116",
    "0612345678",
    "１２３４５６７８",
];
const NUMBER_FRAMES: &[&str] = &[
    "Ref {}.",
    "#{}",
    "({})",
    "id:{}",
    "{}abc",
    "abc{}",
    "x_{}_y",
    "{}-{}",
    "+{}",
    "€{}",
    "code {}",
    "PIN: {}",
    "tel {} {}",
    "{}/{}/{}",
    "{}.{}",
    "\"{}\"",
    "{},{}",
    // Straight before a full stop with no space: not a domain.
    "Call me on {}.thanks",
    "{}.ok",
    "Card {}.co",
    // A link with no path and no space after it ends at its host.
    "Visit https://acme.example—call {} today",
    "詳細はhttps://acme.example。電話{}です",
    "see https://acme.example,{} ok",
];

#[test]
fn no_run_of_four_or_more_digits_survives() {
    let digits4 = |s: &str| {
        let mut run = 0;
        s.chars().any(|c| {
            run = if c.is_numeric() { run + 1 } else { 0 };
            run >= 4
        })
    };
    let mut checked = 0;
    for n in NUMBERS {
        for frame in NUMBER_FRAMES {
            let text = frame.replace("{}", n);
            let out = redact(&text, &Known::default());
            assert!(!digits4(out.as_str()), "{text:?} -> {:?}", out.as_str());
            checked += 1;
        }
    }
    assert!(checked > 100);
}

#[test]
fn no_local_part_survives() {
    let locals = [
        "ana.lopez",
        "a+tag",
        "Z_9",
        "jo-ann",
        "ünï.çø",
        "o'neil",
        "mary.o'connor",
        "sales/eu",
        "\"john doe\"",
        "x",
    ];
    let domains = [
        "acme.example",
        "mail.co.example",
        "Shop42.example",
        "www.acme.example",
    ];
    let frames = [
        "<{}>",
        "mailto:{}",
        "({})",
        "{}.",
        "From: Ana <{}>",
        "{},next",
        "\"{}\"",
    ];
    let mut checked = 0;
    for local in locals {
        for domain in domains {
            for frame in frames {
                let text = frame.replace("{}", &format!("{local}@{domain}"));
                let out = redact(&text, &Known::default());
                let out = out.as_str();
                // Nothing of the local part survives, not even a piece of it.
                let want = frame.replace("{}", &format!("<email>@{domain}"));
                assert_eq!(out, want, "{text:?}");
                checked += 1;
            }
        }
    }
    assert!(checked > 100);
}

#[test]
fn no_known_name_survives_as_a_word() {
    let names = [
        "Will Turner",
        "May Chen",
        "Ana de la Cruz",
        "Siobhán O'Brien",
        "Jean-Luc Picard",
        "Seán O’Neil",
    ];
    // Apostrophes in either form, whichever the header used.
    let words = [
        "Will",
        "Turner",
        "May",
        "Chen",
        "Ana",
        "Cruz",
        "Siobhán",
        "O'Brien",
        "O’Brien",
        "Jean",
        "Luc",
        "O'Neil",
        "O’Neil",
    ];
    let frames = [
        "{}",
        "{},",
        "Dear {}:",
        "({})",
        "{}'s",
        "{}’s",
        "— {}!",
        "{}\n",
        "x {} y",
        "Regards,{}.see you",
        "https://acme.example—{}",
    ];
    let known = Known::new(names, [""; 0]);
    let mut checked = 0;
    for word in words {
        for form in [word.to_string(), word.to_uppercase()] {
            for frame in frames {
                let text = frame.replace("{}", &form);
                let out = redact(&text, &known);
                let fold = |w: &str| w.replace('’', "'");
                let want = fold(&form);
                let survivors: Vec<String> = out
                    .as_str()
                    .split(|c: char| !(c.is_alphanumeric() || c == '\'' || c == '’'))
                    .map(fold)
                    .filter(|w| *w == want || w.strip_suffix("'s") == Some(want.as_str()))
                    .collect();
                assert!(survivors.is_empty(), "{text:?} -> {:?}", out.as_str());
                checked += 1;
            }
        }
    }
    assert!(checked > 100);
}

/// A body whose masks land at every offset around the edge.
fn body(pad: usize) -> String {
    let mut s = "a".repeat(pad);
    while s.chars().count() < EXCERPT_CHARS + 200 {
        s.push_str(" ana@acme.example https://acme.example/p 12345 ü Ana +44 20 7946 0958");
    }
    s
}

#[test]
fn the_cut_never_splits_a_placeholder_nor_passes_the_limit() {
    let known = Known::new(["Ana Lopez"], ["me@example.org"]);
    let mut with_masks = 0;
    for pad in 1_400..1_520 {
        let e = Excerpt::new("", &body(pad), &known);
        let b = e.body();
        assert!(e.truncated(), "pad {pad}");
        assert!(b.as_str().chars().count() <= EXCERPT_CHARS, "pad {pad}");
        with_masks += usize::from(!b.masks().is_empty());
        for piece in b.pieces() {
            match piece {
                Piece::Mask(_, t) => {
                    assert!(t.starts_with('<') && t.ends_with('>'), "pad {pad}: {t}")
                }
                Piece::Text(t) => {
                    assert!(!t.contains('<'), "pad {pad}: half a placeholder in {t:?}")
                }
            }
        }
    }
    assert!(with_masks > 50, "{with_masks}");
}

#[test]
fn a_value_straddling_the_edge_is_masked_before_the_cut() {
    let text = format!("{} 4111111111111111 tail", "y".repeat(EXCERPT_CHARS - 4));
    let e = Excerpt::new("", &text, &Known::default());
    assert!(e.truncated());
    assert_eq!(
        e.body().as_str(),
        format!("{} ", "y".repeat(EXCERPT_CHARS - 4))
    );
    assert!(!e.body().as_str().contains("411"));
}

#[test]
fn the_cut_counts_characters_not_bytes() {
    for text in ["é".repeat(2 * EXCERPT_CHARS), "a😀ü".repeat(EXCERPT_CHARS)] {
        let e = Excerpt::new("", &text, &Known::default());
        assert!(e.truncated());
        assert_eq!(e.body().as_str().chars().count(), EXCERPT_CHARS);
        assert!(text.starts_with(e.body().as_str()));
    }
    // Exactly the limit is not cut.
    let text = "ü".repeat(EXCERPT_CHARS);
    let e = Excerpt::new("", &text, &Known::default());
    assert_eq!((e.truncated(), e.body().as_str()), (false, text.as_str()));
}

/// Runs `make` on a thread and fails if it takes longer than `secs`; a quadratic rule takes hours
/// on these inputs, a linear one well under a second.
fn within<T: Send + 'static>(secs: u64, make: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || tx.send(make()));
    match rx.recv_timeout(std::time::Duration::from_secs(secs)) {
        Ok(v) => v,
        Err(e) => panic!("redaction took over {secs} s, or failed: {e}"),
    }
}

#[test]
fn a_url_ending_in_a_megabyte_of_brackets_redacts_quickly() {
    let e = within(30, || {
        let body = format!("https://a.example/{}", ")".repeat(1 << 20));
        Excerpt::new("", &body, &Known::default())
    });
    assert!(e.body().as_str().starts_with("<link host=a.example>"));
}

#[test]
fn a_large_body_of_links_dates_numbers_and_names_redacts_quickly() {
    let e = within(30, || {
        let body = "a.example/p on 2026-01-16, ref 12345, Ana at a.example. ".repeat(1 << 15);
        Excerpt::new("", &body, &Known::new(["Ana"], [""; 0]))
    });
    assert!(e.body().as_str().starts_with(
        "<link host=a.example> on 2026-01-16, ref <number>, <name> at a.example. <link"
    ));
}

#[test]
fn a_short_body_and_the_whole_subject_are_kept() {
    let subject = format!("Order 12345 {}", "s".repeat(2 * EXCERPT_CHARS));
    let e = Excerpt::new(
        &subject,
        "Hi Ana, call 555-2671.",
        &Known::new(["Ana"], [""; 0]),
    );
    assert!(!e.truncated());
    assert_eq!(e.body().as_str(), "Hi <name>, call <phone>.");
    assert!(e.subject().as_str().starts_with("Order <number> sss"));
    assert_eq!(
        e.subject().as_str().chars().count(),
        subject.chars().count() - 5 + 8
    );
    let masks: Vec<(&str, usize)> = e
        .masks()
        .iter()
        .filter(|(_, n)| *n > 0)
        .map(|(k, n)| (k.name(), n))
        .collect();
    assert_eq!(masks, [("phone", 1), ("number", 1), ("name", 1)]);
}
