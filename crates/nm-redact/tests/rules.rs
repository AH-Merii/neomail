//! Each rule: what it masks, and what it must leave alone. Every example is made up.

use nm_redact::{Known, redact};

fn plain(text: &str) -> String {
    redact(text, &Known::default()).as_str().to_string()
}

fn with(names: &[&str], owner: &[&str], text: &str) -> String {
    redact(text, &Known::new(names, owner)).as_str().to_string()
}

#[test]
fn an_address_loses_its_local_part_and_keeps_its_domain() {
    assert_eq!(
        plain("Write to ana.lopez@acme.example today"),
        "Write to <email>@acme.example today"
    );
    assert_eq!(
        plain("mailto:Jo+news@Mail.Acme.example."),
        "mailto:<email>@Mail.Acme.example."
    );
    // A domain starting with `www.` is still an address's domain, not a link.
    assert_eq!(plain("jo@www.acme.example"), "<email>@www.acme.example");
    assert_eq!(
        plain("jo@mail.www.acme.example"),
        "<email>@mail.www.acme.example"
    );
    // RFC 5322 section 3.2.3 allows `'` and `/`; section 3.4.1 a quoted local part.
    assert_eq!(plain("mary.o'connor@acme.example"), "<email>@acme.example");
    assert_eq!(plain("sales/eu@acme.example"), "<email>@acme.example");
    assert_eq!(plain("\"john doe\"@acme.example"), "<email>@acme.example");
    // A quote or a slash before the address is not its local part.
    assert_eq!(plain("'jo@acme.example'"), "'<email>@acme.example'");
}

#[test]
fn the_owners_addresses_are_masked_whole_without_case() {
    let owner = ["You@Outlook.example", "me@proton.example"];
    assert_eq!(
        with(
            &[],
            &owner,
            "From you@OUTLOOK.example to ME@proton.example and bo@outlook.example"
        ),
        "From <email> to <email> and <email>@outlook.example"
    );
}

#[test]
fn a_url_keeps_only_its_host() {
    assert_eq!(
        plain("Pay at https://ana:pw@pay.acme.example:8443/inv/8812?u=7#top now."),
        "Pay at <link host=pay.acme.example> now."
    );
    assert_eq!(
        plain("(see www.acme.example/a?b=1)"),
        "(see <link host=www.acme.example>)"
    );
    assert_eq!(
        plain("at acme.example/track/9921, today"),
        "at <link host=acme.example>, today"
    );
    assert_eq!(plain("http://[::1]:8080/x"), "<link host=[::1]>");
    // Without a scheme, in capitals or mixed case.
    assert_eq!(
        plain("Reset at ACME.EXAMPLE/reset/AbCd now"),
        "Reset at <link host=ACME.EXAMPLE> now"
    );
    assert_eq!(plain("Acme.Com/u/x?t=y"), "<link host=Acme.Com>");
    // With no space after a link that has no path, the host ends it, and the rest of the text
    // goes through the other rules rather than into the host.
    assert_eq!(
        plain("Visit https://acme.example—call 555-123-4567 today"),
        "Visit <link host=acme.example>—call <phone> today"
    );
    assert_eq!(
        plain("詳細はhttps://acme.example。電話090-1234-5678です"),
        "詳細は<link host=acme.example>。電話090-<number>-<number>です"
    );
    assert_eq!(
        plain("see https://acme.example,4111111111111111 ok"),
        "see <link host=acme.example>,<card> ok"
    );
    assert_eq!(
        plain("at https://acme.example:8443."),
        "at <link host=acme.example>."
    );
    // Path parameters and percent-encoding stay inside the link.
    assert_eq!(
        plain("see https://acme.example;jsessionid=ABCD1234XYZ"),
        "see <link host=acme.example>"
    );
    assert_eq!(
        plain("see https://acme.example%2Fu%2Fjbloggs%3Ft%3DSECRET"),
        "see <link host=acme.example>"
    );
    // A host that isn't plain ASCII, before a path or query: the whole URL, with no host.
    assert_eq!(
        plain("Visit https://почта.рф/reset?token=AbCdEf123&user=jbloggs now"),
        "Visit <link> now"
    );
    assert_eq!(
        plain("https://shop.例え.jp/u/jbloggs?token=AbCdEf123"),
        "<link>"
    );
    assert_eq!(
        plain("http://[fe80::1%25eth0]:8080/u/jbloggs?t=SECRET99"),
        "<link>"
    );
    assert_eq!(
        plain("https://jörg:Secret@acme.example/u/jo?t=abcSECRET"),
        "<link>"
    );
    assert_eq!(plain("https://jörg:Secret@acme.example ok"), "<link> ok");
    // A host that goes on in letters that aren't ASCII, whatever follows, and text written
    // against a host with no space, as Japanese is.
    assert_eq!(
        plain("see https://bücher.example&token=AbCdEf123 ok"),
        "see <link> ok"
    );
    assert_eq!(plain("see https://Müller.example ok"), "see <link> ok");
    assert_eq!(plain("詳細はhttps://acme.exampleです"), "詳細は<link>");
    // An invisible character alone after a host is no part of it.
    assert_eq!(
        plain("Visit https://acme.example\u{200b} now"),
        "Visit <link host=acme.example>\u{200b} now"
    );
    // A backslash starts a path as `/` does.
    assert_eq!(
        plain(r"see https://acme.example\u\jbloggs\AbCdEf123 ok"),
        "see <link host=acme.example> ok"
    );
    assert_eq!(plain("http://[fe80::1%25eth0]/"), "<link>");
    assert_eq!(
        plain("https://my_host.example/reset?token=abcSECRET"),
        "<link host=my_host.example>"
    );
    // A host carrying a card, a phone number or a known name.
    for host in [
        "acme.example-4111111111111111",
        "acme.example.4111111111111111",
        "acme.example-555-123-4567",
        "acme.example.Smith",
    ] {
        assert_eq!(
            with(&["Jane Smith"], &[], &format!("https://{host} ok")),
            "<link> ok"
        );
    }
    // A host longer than DNS allows is no host.
    let long = "www.".repeat(100) + "acme.example";
    assert_eq!(plain(&format!("go {long} now")), "go <link> now");
}

#[test]
fn a_bare_domain_in_capitals_is_kept_but_a_missing_space_is_not_a_domain() {
    assert_eq!(
        with(
            &["Acme Shop", "Will Brown"],
            &[],
            "ACME.EXAMPLE and acme.example"
        ),
        "ACME.EXAMPLE and acme.example"
    );
    // A title-case top-level domain is a host only with a path, so the name rule still applies.
    assert_eq!(
        with(&["Will Brown"], &[], "Ask Will.It works"),
        "Ask <name>.It works"
    );
    // A file name in capitals is not a domain either (`md` is also Moldova's top-level domain).
    assert_eq!(with(&["Will Brown"], &[], "See BROWN.MD"), "See <name>.MD");
}

#[test]
fn a_domain_or_link_host_is_never_masked_by_a_later_rule() {
    assert_eq!(
        with(
            &["Acme Shop"],
            &[],
            "https://shop2024.example/p, x@web1234.example and Acme.example"
        ),
        "<link host=shop2024.example>, <email>@web1234.example and Acme.example"
    );
    // A bare domain is kept from names only: digits in it are masked, so a value written straight
    // before a full stop never passes as a domain.
    assert_eq!(
        with(
            &["Anna"],
            &[],
            "shop5678.example, Card 4111111111111111.co, Anna.de"
        ),
        "shop<number>.example, Card <card>.co, Anna.de"
    );
    // Only a real top-level domain makes a domain.
    assert_eq!(
        with(
            &["Anna"],
            &[],
            "Call 0612345678.thanks, IBAN DE89370400440532013000.ok, Regards,Anna.see you"
        ),
        "Call <number>.thanks, IBAN <iban>.ok, Regards,<name>.see you"
    );
    // A file name is not a host: its digits are masked.
    assert_eq!(plain("see invoice9921.pdf"), "see invoice<number>.pdf");
}

#[test]
fn phone_numbers_are_masked() {
    for phone in [
        "+44 20 7946 0958",
        "(415) 555-2671",
        "020 7946 0958",
        "0049 30 1234567",
        "01.23.45.67.89",
        // Not a date: no year starts with 06.
        "0612-12-12",
        // Digits from other scripts, in groups too short for the digit-run rule.
        "０９０ １２３ ４５６",
        "٠٩٠ ١٢٣ ٤٥٦",
        "+४४ २० ७९४६ ०९५८",
    ] {
        assert_eq!(
            plain(&format!("Call {phone} today")),
            "Call <phone> today",
            "{phone}"
        );
    }
}

#[test]
fn dates_and_times_are_not_phone_numbers_and_stay() {
    for date in [
        "2026-01-16",
        "2026/01/16",
        "16/01/2026",
        "01/16/2026",
        "16.01.2026",
        "16-01-26",
        "2026-01-16 10:30",
        "2026-01-16T10:30:00Z",
        "16 January 2026",
        "Jan 16, 2026",
        "May 2026",
        "10:30",
        "10:30:15",
    ] {
        assert_eq!(
            plain(&format!("Due {date}, call +44 20 7946 0958.")),
            format!("Due {date}, call <phone>."),
            "{date}"
        );
    }
}

#[test]
fn a_luhn_valid_card_is_a_card_and_a_failing_one_a_number() {
    // Every number here passes Luhn: 13 to 19 digits make a card, 12 or 20 do not.
    for (digits, want) in [
        ("4222222222222", "<card>"),
        ("378282246310005", "<card>"),
        ("4111111111111111", "<card>"),
        ("4111111111111111110", "<card>"),
        ("４１１１１１１１１１１１１１１１", "<card>"),
        ("411111111117", "<number>"),
        ("41111111111111111115", "<number>"),
    ] {
        assert_eq!(
            plain(&format!("Card {digits} on file")),
            format!("Card {want} on file"),
            "{digits}"
        );
    }
    assert_eq!(
        plain("Card 4111 1111 1111 1111 on file"),
        "Card <card> on file"
    );
    assert_eq!(plain("Card 4111-1111-1111-1111."), "Card <card>.");
    assert_eq!(plain("Card 5555555555554444"), "Card <card>");
    assert_eq!(plain("Ref 4111111111111112"), "Ref <number>");
    assert_eq!(
        plain("Ref 4111 1111 1111 1112"),
        "Ref <number> <number> <number> <number>"
    );
}

#[test]
fn an_iban_with_valid_check_digits_is_an_iban() {
    assert_eq!(
        plain("IBAN GB82 WEST 1234 5698 7654 32 please"),
        "IBAN <iban> please"
    );
    assert_eq!(plain("to DE89370400440532013000."), "to <iban>.");
    // The same characters with wrong check digits: only the digit runs go.
    assert_eq!(
        plain("IBAN GB83 WEST 1234 5698 7654 32"),
        "IBAN GB83 WEST <number> <number> <number> 32"
    );
    // Each candidate is checked from its own start: an earlier token does not hide an IBAN.
    assert_eq!(
        plain("Ref AB12 GB82 WEST 1234 5698 7654 32"),
        "Ref AB12 <iban>"
    );
    assert_eq!(
        plain("DE89 3704 0044 0532 0130 00 GB82 WEST 1234 5698 7654 32"),
        "<iban> <iban>"
    );
    // Countries from the registry's later releases (release 99), letters in the BBAN or not.
    for iban in ["FK88SC123456789012", "RU0204452560040702810412345678901"] {
        assert_eq!(plain(&format!("to {iban} now")), "to <iban> now", "{iban}");
    }
}

#[test]
fn an_amount_with_a_currency_is_masked() {
    for amount in [
        "$135k",
        "1.250,00 EUR",
        "£40",
        "EUR 99",
        "€ 1,234.50",
        "120 euros",
        "US$15",
    ] {
        assert_eq!(
            plain(&format!("Total {amount} due")),
            "Total <amount> due",
            "{amount}"
        );
    }
    let kept = "1,500 users, 1.250.000 views, 12.500.000 hits, 125.000.000 bytes";
    assert_eq!(plain(kept), kept);
}

#[test]
fn a_one_time_code_after_a_keyword_is_masked() {
    assert_eq!(
        plain("Your verification code is 482910."),
        "Your verification code is <code>."
    );
    assert_eq!(
        plain("OTP: 7K3X9 (valid 10 min)"),
        "OTP: <code> (valid 10 min)"
    );
    assert_eq!(plain("PIN 0042"), "PIN <code>");
    // Digit groups joined by a space, a dot or a hyphen.
    assert_eq!(
        plain("Your verification code is 482 917."),
        "Your verification code is <code>."
    );
    assert_eq!(plain("Your code: 123.456"), "Your code: <code>");
    assert_eq!(plain("code 12-34-56 now"), "code <code> now");
    assert_eq!(plain("code 4 8 2 9 1 7 now"), "code <code> now");
    assert_eq!(plain("Your code is １２３ ４５６"), "Your code is <code>");
    // A code right after its keyword wins over a date.
    assert_eq!(plain("code: 12-03-45"), "code: <code>");
    assert_eq!(plain("sort code 12-03-45"), "sort code <code>");
    // Every keyword, in any case; 16 characters at most.
    for kw in [
        "code",
        "otp",
        "pin",
        "passcode",
        "password",
        "verification",
        "2fa",
    ] {
        let title = kw[..1].to_uppercase() + &kw[1..];
        for kw in [kw.to_string(), title, kw.to_uppercase()] {
            assert_eq!(plain(&format!("{kw}: 7K3X9")), format!("{kw}: <code>"));
            assert_eq!(
                plain(&format!("{kw}: 7K3X9ABCDEFGHIJK")),
                format!("{kw}: <code>")
            );
            // After password, passcode or PIN, up to 64 characters.
            let long = format!("{kw}: 7K3X9ABCDEFGHIJKL");
            let secret = ["pin", "passcode", "password"].contains(&kw.to_lowercase().as_str());
            let want = if secret {
                format!("{kw}: <code>")
            } else {
                long.clone()
            };
            assert_eq!(plain(&long), want);
        }
    }
    // After password, passcode or PIN, any token: symbols, and no digit needed.
    for (text, want) in [
        (
            "Your temporary password is: Xk#92!pq7",
            "Your temporary password is: <code>",
        ),
        (
            "Your temporary password: Ab$12345!",
            "Your temporary password: <code>",
        ),
        (
            "Temporary password - P@ssw0rd2026",
            "Temporary password - <code>",
        ),
        ("Password: Tr0ub4dor&3.", "Password: <code>."),
        ("passcode is: hunterxyz, ok", "passcode is: <code>, ok"),
    ] {
        assert_eq!(plain(text), want, "{text}");
    }
    // After password, passcode or PIN with no separator, only a token with a digit or a symbol;
    // after one or several separators, any token; a keyword itself never.
    for (text, want) in [
        ("Reset your password here", "Reset your password here"),
        (
            "Please change your password before Friday",
            "Please change your password before Friday",
        ),
        ("Pin this message to the top", "Pin this message to the top"),
        ("PIN code: 4821", "PIN code: <code>"),
        ("password hunter2 ok", "password <code> ok"),
        ("password: hunterxyz", "password: <code>"),
        ("password — Xk#92!pq7", "password — <code>"),
        ("password – Xk#92!pq7", "password – <code>"),
        ("password → Xk#92!pq7", "password → <code>"),
        ("password -> Xk#92!pq7", "password -> <code>"),
        ("password: : Xk#92!pq7", "password: : <code>"),
        ("password - - Xk#92!pq7", "password - - <code>"),
        ("password is: hunterxyz", "password is: <code>"),
        // After "is" or "was" alone, only a token that looks like a secret: a digit, a symbol, or
        // a capital after its first letter. Security notices say "was changed".
        ("Your password was changed", "Your password was changed"),
        (
            "Your password is about to expire",
            "Your password is about to expire",
        ),
        ("passcode is hunterxyz, ok", "passcode is hunterxyz, ok"),
        // Title Case subjects: a capital first letter is no sign of a secret.
        ("Your Password Was Changed", "Your Password Was Changed"),
        (
            "Your Password Is About To Expire",
            "Your Password Is About To Expire",
        ),
        ("Your password was Reset", "Your password was Reset"),
        // All capitals: a capital after the first letter is no sign of a secret either.
        ("YOUR PASSWORD WAS CHANGED", "YOUR PASSWORD WAS CHANGED"),
        (
            "YOUR PASSWORD IS ABOUT TO EXPIRE",
            "YOUR PASSWORD IS ABOUT TO EXPIRE",
        ),
        ("Your password is hunterXyz", "Your password is <code>"),
        ("Your password is hunter2", "Your password is <code>"),
        ("Your password is Xk#pq", "Your password is <code>"),
        ("Your password was tempPass", "Your password was <code>"),
        (
            "password: passcode: Xk#92!pq7",
            "password: passcode: <code>",
        ),
        ("password password: Xk#92!pq7", "password password: <code>"),
        // A dash joining two words is no separator.
        (
            "This file is password-protected.",
            "This file is password-protected.",
        ),
        ("Set a PIN-protected lock", "Set a PIN-protected lock"),
        (
            "Your new password—remember it well",
            "Your new password—remember it well",
        ),
        ("password-hunter2 ok", "password-<code> ok"),
        ("password—Xk#92!pq7", "password—<code>"),
    ] {
        assert_eq!(plain(text), want, "{text}");
    }
    let long = format!("password: {}", "x".repeat(65));
    assert_eq!(plain(&long), long);
    // No digit, or too short: not a code.
    assert_eq!(
        plain("Promo code SUMMER, code 12"),
        "Promo code SUMMER, code 12"
    );
}

#[test]
fn runs_of_four_or_more_digits_are_numbers_and_shorter_ones_stay() {
    assert_eq!(
        plain("Order 12345 has 3 items, 42 left, ref A1234B"),
        "Order <number> has 3 items, 42 left, ref A<number>B"
    );
    assert_eq!(plain("in 2019 and 999"), "in <number> and 999");
}

#[test]
fn known_names_are_masked_as_whole_words_with_case() {
    let names = ["Will May", "Ana de la Cruz", "Siobhán O'Brien", "Jean-Luc"];
    assert_eq!(
        with(
            &names,
            &[],
            "Will said you will see May in may. ANA, Ana's cat, Anabel, de la Cruz."
        ),
        "<name> said you will see <name> in may. <name>, <name>'s cat, Anabel, de la <name>."
    );
    assert_eq!(
        with(&names, &[], "Dear Siobhán O'Brien and Jean-Luc, from WILL"),
        "Dear <name> <name> and <name>-<name>, from <name>"
    );
    // A name written in capitals is masked in title case too, from three letters on.
    assert_eq!(
        with(
            &["SMITH John", "ANNA SCHMIDT", "IT Team"],
            &[],
            "Dear Mr Smith, John here. Anna, ANNA, Schmidt. It works, IT said."
        ),
        "Dear Mr <name>, <name> here. <name>, <name>, <name>. It works, <name> said."
    );
    // Any form of apostrophe matches any other, in the header or in the body.
    let body = "Hi O’Brien and O‘Brien and O`Brien and O´Brien and OʼBrien and O＇Brien, O´Brien's";
    let want = "Hi <name> and <name> and <name> and <name> and <name> and <name>, <name>'s";
    assert_eq!(with(&["Jane O'Brien"], &[], body), want);
    assert_eq!(with(&["Jane O´Brien"], &[], body), want);
    assert_eq!(with(&["Jane O´Brien"], &[], "O'Brien"), "<name>");
}

#[test]
fn names_in_no_header_stay() {
    assert_eq!(
        with(&["Ana Lopez"], &[], "Ask Bob Smith and Ana."),
        "Ask Bob Smith and <name>."
    );
}

#[test]
fn overlapping_matches_merge_into_one_mask_named_by_the_earlier_kind() {
    // A code and a number in a URL's query: the link names the mask.
    assert_eq!(
        plain("https://acme.example/?code=123456&Ana"),
        "<link host=acme.example>"
    );
    let r = redact("code: 4111 1111 1111 1111", &Known::default());
    assert_eq!(r.as_str(), "code: <card>");
}

#[test]
fn counts_list_every_kind_and_pieces_rebuild_the_text() {
    let r = redact(
        "Mail ana@acme.example, call +44 20 7946 0958",
        &Known::default(),
    );
    let counts: Vec<(&str, usize)> = r.counts().iter().map(|(k, n)| (k.name(), n)).collect();
    assert_eq!(
        counts,
        [
            ("email", 1),
            ("link", 0),
            ("iban", 0),
            ("card", 0),
            ("amount", 0),
            ("code", 0),
            ("phone", 1),
            ("number", 0),
            ("name", 0)
        ]
    );
    let rebuilt: String = r
        .pieces()
        .map(|p| match p {
            nm_redact::Piece::Text(t) | nm_redact::Piece::Mask(_, t) => t,
        })
        .collect();
    assert_eq!(rebuilt, r.as_str());
}

// Arabic mail (issue #60). The From name is محمد العتيبي and the To name سارة أحمد, both made up.

fn arabic(text: &str) -> String {
    with(&["محمد العتيبي", "سارة أحمد"], &[], text)
}

#[test]
fn an_arabic_name_from_the_headers_is_masked_as_a_word() {
    for (text, want) in [
        (
            "مرحبا محمد، شكرا لك يا سارة أحمد.",
            "مرحبا <name>، شكرا لك يا <name> <name>.",
        ),
        // A letter or the article joined to its front.
        ("شكرا لمحمد وسارة والعتيبي", "شكرا <name> <name> <name>"),
        (
            "بمحمد، للعتيبي، بالعتيبي، وأحمد",
            "<name>، <name>، <name>، <name>",
        ),
        // Short vowels, a stretched letter, a hamza left off.
        ("مُحَمَّد ومحـــمد وسَارَة واحمد", "<name> <name> <name> <name>"),
        // "And" before another prefix, and the vocative joined to a name.
        (
            "تحياتي لك ولمحمد وبسارة وللعتيبي",
            "تحياتي لك <name> <name> <name>",
        ),
        ("يامحمد كيفك؟ ياسارة شكراً", "<name> كيفك؟ <name> شكراً"),
        // The accusative, written with its ending either way round.
        ("أبلغت محمداً ثم محمدًا", "أبلغت <name> ثم <name>"),
    ] {
        assert_eq!(arabic(text), want, "{text}");
    }
    // A header written with vowels matches the body without them.
    assert_eq!(with(&["مُحَمَّد"], &[], "مع محمد"), "مع <name>");
}

#[test]
fn ordinary_arabic_words_that_look_like_a_name_stay() {
    // علي (Ali) is not على ("on"), منى (Muna) not مني ("from me"), رحمة (Rahma) not رحمه.
    let known = ["علي حسن", "منى", "رحمة"];
    for text in [
        "الكتاب على الطاولة",
        "رسالة مني إليك",
        "رحمه الله",
        // A prefix and a name that make another word: "perhaps I", "actual", "and perhaps I".
        "لعلي أراك غدا",
        "هذا الخصم فعلي وليس وهميا",
        "ولعلي أراك غدا",
        // A name's start or end inside a longer word.
        "عليك وعلينا",
    ] {
        assert_eq!(with(&known, &[], text), text, "{text}");
    }
    assert_eq!(with(&["ريم"], &[], "عميل كريم"), "عميل كريم");
    assert_eq!(with(&["ريم"], &[], "كريم"), "كريم");
    // Names that are ordinary words, against stock lines of mail: the definite form ("our valued
    // customer", "the light", "the happy Eid"), the madda alef ("I hope", "any", "amen"), and a
    // prefix that makes another word ("we have", "our parents", "existence").
    for (name, text) in [
        (
            "كريم منصور",
            "عميلنا الكريم، شكرا لثقتكم. نقرأ القرآن الكريم.",
        ),
        ("نور حسن", "النور جميل. بالنور. أحسن الحسن."),
        ("سعيد", "العيد السعيد"),
        ("أمل سعيد", "آمل أن تكون بخير. بالأمل نعيش."),
        ("آية حسن", "إذا كانت لديك أية أسئلة، تواصل معنا لأية أسئلة."),
        ("أمين علي", "اللهم آمين."),
        (
            "دينا خالد",
            "لدينا عرض خاص لك. ولدينا المزيد. نشكر والدينا على كل شيء.",
        ),
        ("جود خالد", "في حال وجود أي استفسار، نحن هنا."),
    ] {
        assert_eq!(with(&[name], &[], text), text, "{name}: {text}");
    }
    // The madda is kept, not dropped: a header's آية still masks آية and وآية.
    assert_eq!(with(&["آية حسن"], &[], "مع آية وآية"), "مع <name> <name>");
}

#[test]
fn an_arabic_organisation_name_masks_none_of_its_words() {
    // Made-up senders whose names are ordinary nouns, against lines of their own mail.
    for (name, text) in [
        (
            "البنك الأهلي",
            "تواصل مع البنك فوراً. البنك الأهلي لن يطلب منك رمز التحقق أبداً.",
        ),
        (
            "شركة الاتصالات السعودية",
            "فاتورة الاتصالات جاهزة. نخدمك في المملكة العربية السعودية.",
        ),
        (
            "خدمة العملاء - متجر أكمي",
            "فريق خدمة العملاء جاهز لخدمتك. تقييمك لخدمة التوصيل يهمنا.",
        ),
        ("وزارة الصحة", "نتمنى لكم الصحة والعافية."),
        ("مصرف الراجحي", "شكرا لاختيارك الراجحي."),
        (
            "صحيفة الرياض",
            "أبرز أخبار الرياض اليوم: أمطار على الرياض وجدة. اشترك في صحيفة الرياض",
        ),
        ("نادي الهلال", "نادي الهلال يفوز على النصر"),
        ("نادى الزمالك", "نادى الزمالك يفوز اليوم"),
        ("أسواق العثيم", "عروض أسواق العثيم الأسبوعية"),
    ] {
        assert_eq!(with(&[name], &[], text), text, "{name}: {text}");
    }
    // A person's name next to it on another header is still masked.
    assert_eq!(
        with(
            &["البنك الأهلي", "سارة أحمد"],
            &[],
            "عزيزتي سارة، البنك يرحب بك"
        ),
        "عزيزتي <name>، البنك يرحب بك"
    );
}

#[test]
fn arabic_name_particles_are_not_masked_alone() {
    let known = ["خالد بن عبد الله آل سعود", "أبو أحمد", "سيف الله"];
    // عبد الله is one name, in the header and the body; الله alone stays.
    assert_eq!(
        with(
            &known,
            &[],
            "من عبد الله وعبدالله، إن شاء الله، ابن خالد وأبو"
        ),
        "من <name> <name>، إن شاء الله، ابن <name> وأبو"
    );
    assert_eq!(with(&["عبدالرحمن"], &[], "عبد الرحمن"), "<name>");
}

#[test]
fn names_in_other_scripts_without_capitals_are_masked_as_words() {
    assert_eq!(
        with(&["דנה כהן"], &[], "שלום דנה כהן"),
        "שלום <name> <name>"
    );
    assert_eq!(with(&["प्रिया शर्मा"], &[], "नमस्ते प्रिया"), "नमस्ते <name>");
    assert_eq!(
        with(&["김 민수"], &[], "안녕하세요 민수 님"),
        "안녕하세요 <name> 님"
    );
}

#[test]
fn a_code_or_password_after_an_arabic_keyword_is_masked() {
    for (text, want) in [
        ("كلمة المرور: hunterxyz", "كلمة المرور: <code>"),
        // Arabic punctuation after the password ends it, as `,` does.
        ("كلمة المرور: hunterxyz، شكرا", "كلمة المرور: <code>، شكرا"),
        (
            "كلمة المرور المؤقتة: Xk#92!pq7",
            "كلمة المرور المؤقتة: <code>",
        ),
        ("كلمة السر: P@ssw0rd2026", "كلمة السر: <code>"),
        ("الرقم السري: 0042", "الرقم السري: <code>"),
        (
            "كلمة المرور الخاصة بك هي: hunterxyz",
            "كلمة المرور الخاصة بك هي: <code>",
        ),
        ("كلمة المرور هي Xk#92!pq7", "كلمة المرور هي <code>"),
        ("رمز التحقق: ٤٨٢٩١٠", "رمز التحقق: <code>"),
        (
            "رمز التحقق الخاص بك هو 482913، لا تشاركه",
            "رمز التحقق الخاص بك هو <code>، لا تشاركه",
        ),
        ("رمز التفعيل 7K3X9", "رمز التفعيل <code>"),
        ("رمز الدخول: 4821", "رمز الدخول: <code>"),
        (
            "الرمز أحادي الاستخدام الخاص بك هو:\n7319",
            "الرمز أحادي الاستخدام الخاص بك هو:\n<code>",
        ),
        ("رمز التحقق ٤٨٢ ٩١٧", "رمز التحقق <code>"),
        // "Your password", "your code", and one or two words before a colon or "is".
        ("كلمة مرورك: P@ssw0rd", "كلمة مرورك: <code>"),
        ("كلمة سرك: hunterxyz", "كلمة سرك: <code>"),
        ("رمزك: A7K9Q2", "رمزك: <code>"),
        ("كلمة مرورك هي Hunter2026", "كلمة مرورك هي <code>"),
        (
            "كلمة مروركم المؤقتة هي Xy7kL2p",
            "كلمة مروركم المؤقتة هي <code>",
        ),
        ("رقمك السري: 0042", "رقمك السري: <code>"),
        ("كلمة مرور حسابك: P@ssw0rd", "كلمة مرور حسابك: <code>"),
        (
            "كلمة سر الواي فاي: sunshine2026",
            "كلمة سر الواي فاي: <code>",
        ),
    ] {
        assert_eq!(plain(text), want, "{text}");
    }
    // "is" with no other separator, and no token that looks like a secret: ordinary words stay.
    for text in [
        "تم تغيير كلمة المرور الخاصة بك",
        "كلمة المرور هي مفتاح حسابك",
        "رمز التحقق صالح لمدة 10 دقائق",
        "أدخل رمز التحقق المرسل إليك",
        "تم تغيير كلمة مرورك",
        "تم تغيير كلمة مرورك بنجاح. إذا لم تكن أنت، تواصل معنا",
        "كلمة مرورك ستنتهي قريبا - يرجى تغييرها",
        "كلمة مرورك الجديدة هي الأقوى",
        // After other words, a colon alone does not make the next word a secret: "the password
        // has expired: please renew it", "the password is wrong: try again".
        "كلمة المرور منتهية الصلاحية: يرجى تحديثها",
        "كلمة المرور غير صحيحة: حاول مرة أخرى",
    ] {
        assert_eq!(plain(text), text, "{text}");
    }
}

#[test]
fn an_amount_in_an_arabic_currency_or_with_arabic_separators_is_masked() {
    for amount in [
        "١٢٠ ر.س",
        "١٬٢٥٠٫٥٠ ر.س",
        "\u{200f}١٬٢٥٠٫٥٠\u{a0}ر.س.\u{200f}",
        "1,250.50 د.إ.",
        "٥٠٠ جنيه",
        "500 جنيه مصري",
        "١٢٫٥٠٠ د.ك",
        "747.5 ريال",
        "460ريال",
        "75 درهم",
        "٣ دينار",
        "100 دولار",
        "١٠٠ دولار أمريكي",
        "٣ دولارات",
        "99 يورو",
        "١٢٠ \u{fdfc}",
        "\u{20c1} 120",
        "12.00 SAR",
    ] {
        let out = plain(&format!("المبلغ: {amount} فقط"));
        assert!(
            out.starts_with("المبلغ: ")
                && out.contains("<amount>")
                && !out.contains(char::is_numeric),
            "{amount:?} -> {out:?}"
        );
    }
    // The accusative ending after 11 to 99, Arab currency codes and `SR`: masked to the end.
    for (text, want) in [
        ("المبلغ ٥٠ ريالاً فقط", "المبلغ <amount> فقط"),
        ("السعر 50 ريالًا فقط", "السعر <amount> فقط"),
        ("بقيمة ٥٠ درهماً", "بقيمة <amount>"),
        ("بقيمة ٢٠ ديناراً", "بقيمة <amount>"),
        ("بقيمة ٢٠٠ جنيهاً مصرياً", "بقيمة <amount> مصرياً"),
        ("بقيمة ١٥ ليرةً", "بقيمة <amount>"),
        ("Total: EGP 1,200", "Total: <amount>"),
        ("Total 120 LYD", "Total <amount>"),
        ("Total: KWD 1,200.500", "Total: <amount>"),
        ("Total: 45 QAR", "Total: <amount>"),
        ("الإجمالي: SR 120.00", "الإجمالي: <amount>"),
        ("الإجمالي: 120.00 SR", "الإجمالي: <amount>"),
    ] {
        assert_eq!(plain(text), want, "{text}");
    }
    // A word that starts like a currency is not one; a grouped number alone is kept, as `1,500` is.
    // A score before a club whose name starts with `ريال` ("Real") is no amount.
    for text in [
        "لديك ١٠ رسائل",
        "منذ ٣ ليالٍ",
        "عدد الزوار ١٬٥٠٠",
        "برشلونة ٢-١ ريال مدريد",
        "فاز ريال بيتيس 3 - 1 ريال سوسيداد",
    ] {
        assert_eq!(plain(text), text, "{text}");
    }
}

#[test]
fn a_date_with_an_arabic_month_name_stays_a_date() {
    for date in [
        "١٦ يناير ٢٠٢٦",
        "16 أغسطس 2026",
        "١٦ كانون الثاني ٢٠٢٦",
        "٣ تشرين الأول ٢٠٢٦",
        "12 أكتوبر/تشرين الأول 2026",
        "١٥ رمضان ١٤٤٧هـ",
        "٢٧ رجب ١٤٤٧ هـ",
        "١ ذو الحجة ١٤٤٧",
        "٢٠ ذي القعدة ١٤٤٧هـ",
        "٥ ذي الحجة ١٤٤٧ هـ",
        "١ ربيع الثاني ١٤٤٨هـ",
        "4 جمادى الأولى 1447",
        "١٤٤٧/٠٥/١٢هـ",
        "يوليو وأغسطس ٢٠٢٦",
        "١٦ جانفي ٢٠٢٦",
        "يناير ٢٠٢٦",
        "١٦\u{200f}/١\u{200f}/٢٠٢٦",
    ] {
        let text = format!("الموعد {date} صباحا");
        assert_eq!(plain(&text), text, "{date}");
    }
    // A word that ends in a month's name is no date: `الحساب` ("the account") ends in `اب`.
    for (text, want) in [
        ("رقم الحساب 2045 مغلق", "رقم الحساب <number> مغلق"),
        ("من الكتاب ١٤٣٢", "من الكتاب <number>"),
    ] {
        assert_eq!(plain(text), want, "{text}");
    }
}

#[test]
fn a_number_with_direction_marks_between_its_groups_is_masked_whole() {
    for mark in [
        '\u{200e}', '\u{200f}', '\u{61c}', '\u{2066}', '\u{2069}', '\u{202b}',
    ] {
        let text = format!("اتصل ٠٥٠{mark}-١٢٣{mark}-٤٥٦٧{mark} الآن");
        assert_eq!(plain(&text), format!("اتصل <phone>{mark} الآن"), "{mark:?}");
        let text = format!("رقم الطلب 12{mark}34{mark}56");
        assert_eq!(plain(&text), "رقم الطلب <number>", "{mark:?}");
    }
    // A mark between two letters still splits two words.
    assert_eq!(
        with(&["Ana Lopez"], &[], "Ana\u{200e}Lopez"),
        "<name>\u{200e}<name>"
    );
}
