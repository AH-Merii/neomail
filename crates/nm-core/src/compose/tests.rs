#![allow(clippy::unwrap_used)]

use chrono::{TimeZone, Utc};
use mail_parser::{MessageParser, MimeHeaders};

use super::*;
use crate::parse::parse;

fn me() -> Mailbox {
    Mailbox::parse("Me Myself <me@example.com>").unwrap()
}

fn draft(markdown: &str) -> Draft {
    Draft {
        from: me(),
        to: vec![Mailbox::parse("Ann <ann@example.org>").unwrap()],
        cc: Vec::new(),
        bcc: Vec::new(),
        subject: "Hello".to_owned(),
        markdown: markdown.to_owned(),
        text_only: false,
        in_reply_to: None,
        references: Vec::new(),
    }
}

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 13, 9, 30, 0).unwrap()
}

/// The decoded text of the part with this content type.
fn part(mime: &[u8], subtype: &str) -> Option<String> {
    let msg = MessageParser::default().parse(mime).unwrap();
    msg.parts
        .iter()
        .find(|p| {
            p.content_type()
                .is_some_and(|ct| ct.ctype() == "text" && ct.subtype() == Some(subtype))
        })
        .and_then(|p| p.text_contents().map(str::to_owned))
}

/// `text` in the canonical form of a MIME text part, every line break a CRLF ("The canonical form
/// of any MIME "text" subtype MUST always represent a line break as a CRLF sequence", RFC 2046
/// section 4.1.1).
fn canonical(text: &str) -> String {
    text.replace('\n', "\r\n")
}

/// The header section only, before the first empty line.
fn header_section(mime: &[u8]) -> String {
    let text = String::from_utf8_lossy(mime);
    text.split("\r\n\r\n").next().unwrap().to_owned()
}

const MARKDOWN: &str = "# Plan\n\n**bold** and a [link](https://example.com)\n\n> quoted\n> line two\n\n- one\n- two\n";

#[test]
fn the_plain_part_is_the_markdown_and_the_html_part_is_rendered() {
    let out = draft(MARKDOWN).build(now()).unwrap();
    let msg = MessageParser::default().parse(&out.mime).unwrap();
    let ct = msg.content_type().unwrap();
    assert_eq!(
        (ct.ctype(), ct.subtype()),
        ("multipart", Some("alternative"))
    );
    assert_eq!(part(&out.mime, "plain").unwrap(), canonical(MARKDOWN));
    let html = part(&out.mime, "html").unwrap();
    for want in [
        "<h1>Plan</h1>",
        "<strong>bold</strong>",
        "<a href=\"https://example.com\">link</a>",
        "<blockquote>",
        "quoted<br />",
        "<li>one</li>",
    ] {
        assert!(html.contains(want), "{want} missing from {html}");
    }
}

#[test]
fn raw_html_is_escaped_never_passed_through() {
    for markdown in [
        "<script>alert(1)</script>\n",
        "text <img src=x onerror=alert(1)> more\n",
        "<div onclick=\"x()\">\nblock\n</div>\n",
    ] {
        let html = render_html(markdown);
        assert!(!html.contains("<script") && !html.contains("<img") && !html.contains("<div"));
        assert!(html.contains("&lt;"), "{html}");
        // And the plain part keeps the source as typed.
        let out = draft(markdown).build(now()).unwrap();
        assert_eq!(part(&out.mime, "plain").unwrap(), canonical(markdown));
    }
}

#[test]
fn text_only_sends_a_single_plain_part() {
    let mut d = draft(MARKDOWN);
    d.text_only = true;
    let out = d.build(now()).unwrap();
    let msg = MessageParser::default().parse(&out.mime).unwrap();
    let ct = msg.content_type().unwrap();
    assert_eq!((ct.ctype(), ct.subtype()), ("text", Some("plain")));
    assert_eq!(msg.parts.len(), 1);
    assert_eq!(msg.body_text(0).unwrap(), canonical(MARKDOWN));
}

#[test]
fn non_ascii_subject_and_names_round_trip() {
    let mut d = draft("hi\n");
    d.from = Mailbox::parse("Zoë Ødegård <zoe@example.com>").unwrap();
    d.subject =
        "Grüße, 東京 🎉 and a long subject that has to be folded over more than one line".into();
    let out = d.build(now()).unwrap();
    assert!(header_section(&out.mime).is_ascii());
    let msg = MessageParser::default().parse(&out.mime).unwrap();
    assert_eq!(msg.subject(), Some(d.subject.as_str()));
    let from = msg.from().unwrap().first().unwrap();
    assert_eq!(from.name(), Some("Zoë Ødegård"));
    assert_eq!(from.address(), Some("zoe@example.com"));
}

/// SMTP without 8BITMIME carries 7-bit lines of at most 998 octets (RFC 5321 section 4.5.3.1.6):
/// non-ASCII text and long lines must be transfer-encoded.
#[test]
fn the_message_is_7_bit_with_short_lines() {
    let mut d = draft(&format!("Grüße aus 東京\n\n{}\n", "long ".repeat(400)));
    d.subject = "Zoë".into();
    let out = d.build(now()).unwrap();
    assert!(out.mime.is_ascii());
    assert!(out.mime.split(|b| *b == b'\n').all(|l| l.len() <= 1000));
    assert_eq!(part(&out.mime, "plain").unwrap(), canonical(&d.markdown));
}

#[test]
fn line_breaks_never_reach_a_header() {
    let mut d = draft("x\n");
    d.subject = "Hi\r\nBcc: victim@example.net".into();
    assert_eq!(d.build(now()), Err(ComposeError::LineBreak("subject")));
    d.subject = "Hi\nthere".into();
    assert_eq!(d.build(now()), Err(ComposeError::LineBreak("subject")));
    for typed in [
        "a@example.com\r\nBcc: victim@example.net",
        "Na\nme <a@example.com>",
        "a@exam\nple.com",
    ] {
        assert_eq!(
            Mailbox::parse(typed),
            Err(ComposeError::LineBreak(if typed.starts_with("Na\n") {
                "display name"
            } else {
                "address"
            })),
            "{typed:?}"
        );
    }
}

#[test]
fn addresses_are_checked() {
    for bad in [
        "",
        "plain",
        "a@",
        "@b.example",
        "a b@example.com",
        "a@b@example.com",
        "a@.example",
        "ü@example.com",
        "a,b@example.com",
    ] {
        assert!(
            matches!(Mailbox::parse(bad), Err(ComposeError::Address(..))),
            "{bad:?}"
        );
    }
    let m = Mailbox::parse("\"Ann Lee\" <ann@example.org>").unwrap();
    assert_eq!(
        (m.name(), m.address()),
        (Some("Ann Lee"), "ann@example.org")
    );
    assert_eq!(
        Mailbox::parse(" ann@example.org ").unwrap().to_string(),
        "ann@example.org"
    );
}

#[test]
fn bcc_is_in_the_envelope_only_and_each_recipient_once() {
    let mut d = draft("x\n");
    d.cc = vec![
        Mailbox::parse("cat@example.org").unwrap(),
        Mailbox::parse("ANN@example.org").unwrap(),
    ];
    d.bcc = vec![Mailbox::parse("Hidden Person <hidden@example.net>").unwrap()];
    let out = d.build(now()).unwrap();
    assert_eq!(out.envelope_from, "me@example.com");
    assert_eq!(
        out.envelope_to,
        ["ann@example.org", "cat@example.org", "hidden@example.net"]
    );
    assert_eq!(out.bcc, ["hidden@example.net"]);
    let text = String::from_utf8_lossy(&out.mime).to_ascii_lowercase();
    assert!(!text.contains("hidden"), "{text}");
    assert!(!text.contains("bcc"), "{text}");
}

#[test]
fn a_message_needs_a_recipient() {
    let mut d = draft("x\n");
    d.to.clear();
    assert_eq!(d.build(now()), Err(ComposeError::NoRecipients));
    d.bcc = vec![Mailbox::parse("b@example.org").unwrap()];
    assert!(d.build(now()).is_ok());
}

#[test]
fn every_message_gets_its_own_id_on_the_senders_domain() {
    let d = draft("x\n");
    let a = d.build(now()).unwrap();
    let b = d.build(now()).unwrap();
    assert_ne!(a.message_id, b.message_id);
    assert!(a.message_id.ends_with("@example.com"), "{}", a.message_id);
    let msg = MessageParser::default().parse(&a.mime).unwrap();
    assert_eq!(msg.message_id(), Some(a.message_id.as_str()));
    assert_eq!(msg.date().unwrap().to_timestamp(), now().timestamp());
}

/// A cached original with `extra` headers.
fn original(extra: &str, body: &str) -> ParsedMessage {
    let raw = format!(
        "From: Ann <ann@example.org>\r\nTo: Me Myself <me@example.com>, bob@example.org\r\n\
         Cc: cat@example.org, ME@EXAMPLE.COM\r\nSubject: Lunch\r\n\
         Date: Mon, 12 Oct 2026 09:30:00 +0000\r\n{extra}\r\n{body}"
    );
    parse(raw.as_bytes()).unwrap()
}

fn addresses(list: &[Mailbox]) -> Vec<&str> {
    list.iter().map(Mailbox::address).collect()
}

/// RFC 5322 section 3.6.4, each case of the parent's headers.
#[test]
fn replies_thread_as_rfc_5322_says() {
    let cases: [(&str, Option<&str>, &[&str]); 5] = [
        (
            "Message-ID: <p@x>\r\nIn-Reply-To: <g@x>\r\nReferences: <a@x> <g@x>\r\n",
            Some("p@x"),
            &["a@x", "g@x", "p@x"],
        ),
        (
            "Message-ID: <p@x>\r\nIn-Reply-To: <g@x>\r\n",
            Some("p@x"),
            &["g@x", "p@x"],
        ),
        // In-Reply-To with several ids is not a chain: only the parent's id.
        (
            "Message-ID: <p@x>\r\nIn-Reply-To: <g@x> <h@x>\r\n",
            Some("p@x"),
            &["p@x"],
        ),
        ("Message-ID: <p@x>\r\n", Some("p@x"), &["p@x"]),
        ("", None, &[]),
    ];
    for (headers, in_reply_to, references) in cases {
        let d = reply(&original(headers, "hi\r\n"), false, &me(), &[], "ok").unwrap();
        assert_eq!(d.in_reply_to.as_deref(), in_reply_to, "{headers:?}");
        assert_eq!(d.references, references, "{headers:?}");
        let msg = d.build(now()).unwrap();
        let parsed = MessageParser::default().parse(&msg.mime).unwrap();
        assert_eq!(parsed.in_reply_to().as_text(), in_reply_to, "{headers:?}");
        let sent: Vec<&str> = parsed
            .references()
            .as_text_list()
            .unwrap_or_default()
            .iter()
            .map(|r| r.as_ref())
            .collect();
        assert_eq!(sent, references, "{headers:?}");
    }
}

#[test]
fn re_and_fwd_are_never_doubled() {
    for (subject, re, fwd) in [
        ("Lunch", "Re: Lunch", "Fwd: Lunch"),
        ("Re: Lunch", "Re: Lunch", "Fwd: Re: Lunch"),
        ("RE: Lunch", "RE: Lunch", "Fwd: RE: Lunch"),
        ("re:Lunch", "re:Lunch", "Fwd: re:Lunch"),
        ("Fwd: Lunch", "Re: Fwd: Lunch", "Fwd: Lunch"),
        ("FW: Lunch", "Re: FW: Lunch", "FW: Lunch"),
        ("Reply hazy", "Re: Reply hazy", "Fwd: Reply hazy"),
    ] {
        let mut o = original("", "x");
        o.subject = Some(subject.to_owned());
        assert_eq!(reply(&o, false, &me(), &[], "").unwrap().subject, re);
        assert_eq!(forward(&o, &me(), Vec::new(), "").subject, fwd);
    }
}

/// The original's subject is not the user's to fix: a control character in it, a tab or one an
/// encoded word decodes to, must not stop a reply or forward from being built.
#[test]
fn a_control_character_in_the_original_subject_still_builds() {
    for subject in [
        "Lunch\tplans",
        "=?utf-8?q?Lunch=09plans?=",
        "=?utf-8?q?Lunch=0D=0Aplans?=",
    ] {
        let raw = format!(
            "From: ann@example.org\r\nTo: me@example.com\r\nSubject: {subject}\r\n\r\nhi\r\n"
        );
        let o = parse(raw.as_bytes()).unwrap();
        assert!(
            o.subject
                .as_deref()
                .is_some_and(|s| s.chars().any(char::is_control)),
            "the case must hold a control character after parsing: {subject:?}"
        );
        let r = reply(&o, false, &me(), &[], "ok").unwrap();
        assert_eq!(r.subject, "Re: Lunch plans", "{subject:?}");
        assert!(r.build(now()).is_ok(), "{subject:?}");
        let f = forward(
            &o,
            &me(),
            vec![Mailbox::parse("bob@example.org").unwrap()],
            "",
        );
        assert_eq!(f.subject, "Fwd: Lunch plans", "{subject:?}");
        assert!(f.build(now()).is_ok(), "{subject:?}");
    }
}

#[test]
fn a_reply_goes_to_reply_to_else_from() {
    let d = reply(&original("", "x"), false, &me(), &[], "").unwrap();
    assert_eq!(
        (addresses(&d.to), addresses(&d.cc)),
        (vec!["ann@example.org"], vec![])
    );
    let d = reply(
        &original("Reply-To: list@example.org\r\n", "x"),
        false,
        &me(),
        &[],
        "",
    )
    .unwrap();
    assert_eq!(addresses(&d.to), ["list@example.org"]);
}

/// A sender neomail cannot send to is an error, never a reply to someone else: not to the To list,
/// and not to From in place of an unusable Reply-To.
#[test]
fn an_unusable_sender_address_is_an_error_not_a_reroute() {
    for (headers, address) in [
        ("From: j\u{f6}e@example.org\r\n", "j\u{f6}e@example.org"),
        (
            "From: ann@example.org\r\nReply-To: j\u{f6}e@example.org\r\n",
            "j\u{f6}e@example.org",
        ),
        (
            "From: me@example.com\r\nReply-To: me@example.com, j\u{f6}e@example.org\r\n",
            "j\u{f6}e@example.org",
        ),
    ] {
        let raw = format!("{headers}To: me@example.com, team@example.org\r\nSubject: s\r\n\r\nx");
        let o = parse(raw.as_bytes()).unwrap();
        for all in [false, true] {
            assert_eq!(
                reply(&o, all, &me(), &[], ""),
                Err(ComposeError::Unsendable(
                    address.to_owned(),
                    "only ASCII addresses can be sent for now"
                )),
                "{headers:?} all={all}"
            );
        }
    }
}

/// An address copied from the original's To or Cc that neomail cannot send to is an error too,
/// never left out: in a reply to one's own message, and in a reply to all.
#[test]
fn an_unusable_to_or_cc_address_is_an_error_not_a_smaller_reply() {
    let bad = "j\u{f6}e@example.org";
    let unsendable = Err(ComposeError::Unsendable(
        bad.to_owned(),
        "only ASCII addresses can be sent for now",
    ));
    let own = format!("From: me@example.com\r\nTo: {bad}, bob@example.org\r\nSubject: s\r\n\r\nx");
    let o = parse(own.as_bytes()).unwrap();
    for all in [false, true] {
        assert_eq!(
            reply(&o, all, &me(), &[], ""),
            unsendable,
            "own message, all={all}"
        );
    }
    for headers in [
        format!("To: me@example.com, {bad}\r\n"),
        format!("To: me@example.com\r\nCc: bob@example.org, {bad}\r\n"),
    ] {
        let raw = format!("From: ann@example.org\r\n{headers}Subject: s\r\n\r\nx");
        let o = parse(raw.as_bytes()).unwrap();
        assert_eq!(reply(&o, true, &me(), &[], ""), unsendable, "{headers:?}");
        // Without --all the To and Cc are not copied, so the reply to Ann goes.
        assert_eq!(
            addresses(&reply(&o, false, &me(), &[], "").unwrap().to),
            ["ann@example.org"]
        );
    }
}

/// A display name that cannot go in a header is dropped; its address still gets the reply.
#[test]
fn a_bad_display_name_keeps_its_address() {
    let raw = "From: =?utf-8?q?Ann=0AX?= <ann@example.org>\r\nTo: me@example.com, \
               team@example.org\r\nSubject: s\r\n\r\nx";
    let o = parse(raw.as_bytes()).unwrap();
    assert!(o.from[0].name.as_deref().is_some_and(|n| n.contains('\n')));
    let d = reply(&o, false, &me(), &[], "").unwrap();
    assert_eq!(addresses(&d.to), ["ann@example.org"]);
    assert_eq!(d.to[0].name(), None);
}

#[test]
fn reply_all_lists_everyone_once_and_never_the_sender_identity() {
    let o = original(
        "Reply-To: Ann Two <ANN@example.org>, ann@example.org\r\n",
        "x",
    );
    let d = reply(&o, true, &me(), &[], "").unwrap();
    assert_eq!(addresses(&d.to), ["ANN@example.org"]);
    assert_eq!(addresses(&d.cc), ["bob@example.org", "cat@example.org"]);
    assert!(d.bcc.is_empty());
}

#[test]
fn a_reply_to_ones_own_message_goes_to_its_recipients() {
    let raw =
        "From: Me <me@example.com>\r\nTo: bob@example.org, me@example.com\r\nSubject: s\r\n\r\nx";
    let d = reply(&parse(raw.as_bytes()).unwrap(), false, &me(), &[], "").unwrap();
    assert_eq!(addresses(&d.to), ["bob@example.org"]);
    let alone = "From: me@example.com\r\nTo: me@example.com\r\nSubject: s\r\n\r\nx";
    assert_eq!(
        reply(&parse(alone.as_bytes()).unwrap(), true, &me(), &[], ""),
        Err(ComposeError::NoRecipients)
    );
}

#[test]
fn a_reply_quotes_the_original_below_the_answer() {
    let d = reply(
        &original("", "First line\r\n\r\n<b>third</b>\r\n"),
        false,
        &me(),
        &[],
        "Sure!\n\n",
    )
    .unwrap();
    assert_eq!(
        d.markdown,
        "Sure!\n\nOn Mon, 12 Oct 2026 at 09:30 UTC, Ann <ann@example.org> wrote:\n\n\
         > First line\n>\n> <b>third</b>\n"
    );
    let html = render_html(&d.markdown);
    assert!(html.contains("<blockquote>"), "{html}");
    assert!(html.contains("&lt;b&gt;third"), "{html}");
}

#[test]
fn a_forward_carries_the_original_headers_and_text_inline() {
    let to = vec![Mailbox::parse("dan@example.org").unwrap()];
    let d = forward(
        &original("Message-ID: <p@x>\r\n", "Body text\r\n"),
        &me(),
        to,
        "FYI",
    );
    assert_eq!(
        d.markdown,
        "FYI\n\n---------- Forwarded message ----------\n\n\
         > From: Ann <ann@example.org>\n> Date: Mon, 12 Oct 2026 at 09:30 UTC\n> Subject: Lunch\n\
         > To: Me Myself <me@example.com>, bob@example.org\n\
         > Cc: cat@example.org, ME@EXAMPLE.COM\n>\n> Body text\n"
    );
    assert_eq!(addresses(&d.to), ["dan@example.org"]);
    assert_eq!((d.in_reply_to, d.references.len()), (None, 0));
    // Each header keeps its own line in the HTML part.
    let html = render_html(&d.markdown);
    assert!(html.contains("Lunch<br />"), "{html}");
}

/// When `me` is an alias, the account's login is never a recipient either, and a message sent
/// from the login is one's own.
#[test]
fn reply_all_never_lists_the_accounts_other_addresses() {
    let login = ["login@example.com"];
    let raw = "From: ann@example.org\r\nTo: me@example.com, LOGIN@example.com\r\n\
               Cc: bob@example.org, login@example.com\r\nSubject: s\r\n\r\nx";
    let d = reply(&parse(raw.as_bytes()).unwrap(), true, &me(), &login, "").unwrap();
    assert_eq!(
        (addresses(&d.to), addresses(&d.cc)),
        (vec!["ann@example.org"], vec!["bob@example.org"])
    );
    let own = "From: login@example.com\r\nTo: bob@example.org\r\nSubject: s\r\n\r\nx";
    let d = reply(&parse(own.as_bytes()).unwrap(), true, &me(), &login, "").unwrap();
    assert_eq!(
        (addresses(&d.to), addresses(&d.cc)),
        (vec!["bob@example.org"], vec![])
    );
}
