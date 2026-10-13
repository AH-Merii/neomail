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

/// A link fails closed: whatever its host, none of its path, query or user info goes out, and a
/// host holding a number or a known name gives a bare `<link>`.
#[test]
fn no_token_user_name_number_or_name_in_a_link_survives() {
    let links = [
        // A host that isn't plain ASCII, before a path or query.
        "https://почта.рф/reset?token=AbCdEf123&user=jbloggs",
        "https://例え.jp/reset?token=AbCdEf123&user=jbloggs",
        "https://shop.例え.jp/u/jbloggs?token=AbCdEf123",
        "https://bücher.example?token=AbCdEf123&user=jbloggs",
        "www.почта.рф/u/jbloggs?token=AbCdEf123",
        "https://%61cme.example?t=SECRET99&user=jbloggs",
        "http://[fe80::1%25eth0]:8080/u/jbloggs?t=SECRET99",
        "https://jörg:Secret@acme.example/u/jo?t=abcSECRET",
        "https://jbloggs%40acme.example/u/jo?t=abcSECRET",
        "https://[zz]/u/jo?t=abcSECRET",
        "https://—/u/jo?t=abcSECRET",
        // No host, or user info that isn't plain ASCII, with nothing after it.
        "https://—jbloggs",
        "https://jörg:Secret@acme.example",
        // An underscore is part of a host.
        "https://my_host.example/reset?token=abcSECRET",
        "www.my_host.example/u/jo?t=abcSECRET",
        // A host carrying a card, a phone number or a known name.
        "https://acme.example-4111111111111111",
        "https://acme.example.4111111111111111",
        "https://acme.example-555-123-4567",
        "https://acme.example.Smith",
        "https://Smith.acme.example/u/jo",
        "https://acme-Smith.example?t=abcSECRET",
        // A host that goes on in letters that aren't ASCII, with no path after it.
        "https://bücher.example&token=AbCdEf123",
        "https://bücher.example=AbCdEf123",
        "https://bücher.example;jsessionid=ABCD1234XYZ",
        "https://bu\u{308}cher.example&token=AbCdEf123",
        "https://www.Jörg-Schmidt.example",
        "https://Müller.example",
        // A backslash for a path.
        r"https://acme.example\u\jbloggs\AbCdEf123",
        r"https://acme.example\u\jbloggs?t=AbCdEf123",
    ];
    let frames = [
        "{}",
        "Visit {} now",
        "({})",
        "{}.",
        "詳細は{}です",
        "see {}, ok",
    ];
    let secrets = [
        "AbCdEf123",
        "SECRET99",
        "abcSECRET",
        "Secret",
        "jbloggs",
        "jo",
        "jörg",
        "örg",
        "Smith",
        "SMITH",
        "Schmidt",
        "Müller",
        "üller",
    ];
    let known = Known::new(["Jane Smith", "Jörg Schmidt", "Zoë Müller"], [""; 0]);
    let mut checked = 0;
    for link in links {
        for frame in frames {
            let text = frame.replace("{}", link);
            let out = redact(&text, &known);
            let out = out.as_str();
            let mut run = 0;
            let digits4 = out.chars().any(|c| {
                run = if c.is_numeric() { run + 1 } else { 0 };
                run >= 4
            });
            let survivor = out
                .split(|c: char| !c.is_alphanumeric())
                .find(|w| secrets.contains(w));
            assert!(
                !digits4 && survivor.is_none(),
                "{text:?} -> {out:?} ({survivor:?})"
            );
            checked += 1;
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
    // Apostrophes in any form, whichever the header used.
    const APOSTROPHES: [char; 6] = ['’', '‘', 'ʼ', '´', '`', '＇'];
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
        "O‘Brien",
        "O´Brien",
        "O`Brien",
        "O＇Brien",
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
                let fold = |w: &str| w.replace(APOSTROPHES, "'");
                let want = fold(&form);
                let survivors: Vec<String> = out
                    .as_str()
                    .split(|c: char| {
                        !(c.is_alphanumeric() || c == '\'' || APOSTROPHES.contains(&c))
                    })
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
fn a_megabyte_of_direction_marks_or_tatweel_redacts_quickly() {
    let e = within(30, || {
        let body = format!(
            "محمد{}x ١{}٢ محـ{}مد",
            "\u{200f}".repeat(1 << 19),
            "\u{200e}".repeat(1 << 19),
            "ـ".repeat(1 << 19)
        );
        Excerpt::new("", &body, &Known::new(["محمد"], [""; 0]))
    });
    assert!(e.body().as_str().starts_with("<name>"));
}

#[test]
fn a_large_arabic_body_of_names_dates_amounts_and_codes_redacts_quickly() {
    let e = within(30, || {
        let body = "شكرا لمحمد، الموعد ١٦ يناير ٢٠٢٦، المبلغ ١٬٢٥٠٫٥٠ ر.س، رمز التحقق: ٤٨٢٩١٠، اتصل ٠٥٠\u{200e}-١٢٣\u{200e}-٤٥٦٧. "
            .repeat(1 << 14);
        Excerpt::new("", &body, &Known::new(["محمد العتيبي"], [""; 0]))
    });
    assert!(e.body().as_str().starts_with(
        "شكرا <name>، الموعد ١٦ يناير ٢٠٢٦، المبلغ <amount>، رمز التحقق: <code>، اتصل <phone>. شكرا"
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

/// Arabic name words from made-up headers, each with a body spelling: as written, with short
/// vowels, with a stretched letter, with a hamza left off.
const ARABIC_NAMES: &[(&str, &[&str])] = &[
    ("محمد", &["محمد", "مُحَمَّد", "محـمد", "محـــمد"]),
    ("العتيبي", &["العتيبي", "العُتَيْبِي", "العتـيبي"]),
    ("سارة", &["سارة", "سَارَة", "سـارة"]),
    ("أحمد", &["أحمد", "احمد", "أَحْمَد", "أحـمد"]),
    ("إيمان", &["إيمان", "ايمان", "إِيمَان"]),
    ("نور", &["نور", "نُور"]),
];

#[test]
fn no_known_arabic_name_survives_as_a_word() {
    let known = Known::new(["محمد العتيبي", "سارة أحمد", "إيمان نور"], [""; 0]);
    // No given name carries the article, so it is never a prefix; a family name keeps its own.
    let prefixes = ["", "و", "ب", "ل"];
    let frames = [
        "{}",
        "مرحبا {}،",
        "شكرا {}.",
        "({})",
        "{}؟",
        "x {} y",
        "رد {}\n",
    ];
    let fold = |w: &str| -> String {
        w.chars()
            .filter(|c| !matches!(c, '\u{64b}'..='\u{65f}' | '\u{640}'))
            .map(|c| if matches!(c, 'أ' | 'إ') { 'ا' } else { c })
            .collect()
    };
    let mut checked = 0;
    for (name, forms) in ARABIC_NAMES {
        for form in *forms {
            for prefix in prefixes {
                // `ل` before the article drops its alef: `للعتيبي`.
                let word = match form.strip_prefix('ا') {
                    Some(rest) if prefix == "ل" && name.starts_with("ال") => format!("ل{rest}"),
                    _ => format!("{prefix}{form}"),
                };
                for frame in frames {
                    let text = frame.replace("{}", &word);
                    let out = redact(&text, &known);
                    let leaked = out
                        .as_str()
                        .split(|c: char| !c.is_alphabetic() && !matches!(c, '\u{64b}'..='\u{65f}'))
                        .any(|w| fold(w).ends_with(&fold(name)));
                    assert!(!leaked, "{text:?} -> {:?}", out.as_str());
                    checked += 1;
                }
            }
        }
    }
    assert!(checked > 300, "{checked}");
}

#[test]
fn a_number_split_by_direction_marks_is_masked_whole() {
    let marks = [
        '\u{200e}', '\u{200f}', '\u{61c}', '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}',
    ];
    let numbers = [
        "٠٥٠-١٢٣-٤٥٦٧",
        "١٢٣٤ ٥٦٧٨",
        "1234-5678",
        "٤١١١ ١١١١ ١١١١ ١١١١",
        "١٢٣٤٥٦",
    ];
    // No frame holds a digit, so any digit left is part of a number not masked whole.
    let frames = ["{}", "رقم {}.", "اتصل على {} الآن", "المرجع: {}", "({})"];
    let mut checked = 0;
    for mark in marks {
        for n in numbers {
            // A mark after every digit group, and one between every two digits.
            let grouped: String = n
                .chars()
                .flat_map(|c| [(!c.is_numeric()).then_some(mark), Some(c)])
                .flatten()
                .collect();
            let spread: String = n.chars().flat_map(|c| [c, mark]).collect();
            for number in [grouped, spread] {
                for frame in frames {
                    let text = frame.replace("{}", &number);
                    let out = redact(&text, &Known::default());
                    let out = out.as_str();
                    assert!(!out.contains(char::is_numeric), "{text:?} -> {out:?}");
                    checked += 1;
                }
            }
        }
    }
    assert!(checked > 300, "{checked}");
}
