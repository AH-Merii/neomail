//! The one builder of what classification sends for a message: `neomail redact` prints it, and
//! `neomail classify`'s request takes the same [`Outgoing`]. The rules live in nm-redact; this
//! module only gathers what they need from the parsed message and the config.

use nm_core::parse::{HTML_STRIPPED_NOTE, ParsedMessage};
use nm_redact::{Excerpt, HeaderFlags, Known, Outgoing};

use crate::config::Config;

/// The excerpt of `parsed`: its subject and the start of its body as `neomail show` renders it,
/// redacted. The known names are the display names in From, To, Cc and Reply-To;
/// `owner_addresses` are masked whole.
pub fn excerpt<A: AsRef<str>>(
    parsed: &ParsedMessage,
    owner_addresses: impl IntoIterator<Item = A>,
) -> Excerpt {
    let names = parsed
        .from
        .iter()
        .chain(&parsed.to)
        .chain(&parsed.cc)
        .chain(&parsed.reply_to)
        .filter_map(|a| a.name.as_deref());
    let known = Known::new(names, owner_addresses);
    // The note neomail puts before HTML it could not lay out is neomail's, not the sender's.
    let body = parsed
        .body
        .strip_prefix(HTML_STRIPPED_NOTE)
        .map_or(parsed.body.as_str(), str::trim_start);
    Excerpt::new(parsed.subject.as_deref().unwrap_or(""), body, &known)
}

/// Everything classification sends for `parsed`: its [`excerpt`], and the signals nm-redact
/// reads from the From address and three header flags.
pub fn outgoing<A: AsRef<str>>(
    parsed: &ParsedMessage,
    owner_addresses: impl IntoIterator<Item = A>,
) -> Outgoing {
    let from = parsed.from.first().and_then(|a| a.address.as_deref());
    let flags = HeaderFlags {
        mailing_list: parsed.headers.mailing_list(),
        automated: parsed.headers.automated(),
        reply: parsed.headers.reply(),
    };
    Outgoing::new(excerpt(parsed, owner_addresses), from, flags)
}

/// The owner's addresses: every account's login name that is an address.
pub fn owner_addresses(config: &Config) -> Vec<&str> {
    config
        .accounts
        .iter()
        .map(|a| a.username())
        .filter(|u| u.contains('@'))
        .collect()
}

#[cfg(test)]
mod tests {
    use nm_redact::Kind;

    use super::*;

    fn excerpt_of(raw: &str, owners: &[&str]) -> Excerpt {
        excerpt(&nm_core::parse::parse(raw.as_bytes()).unwrap(), owners)
    }

    #[test]
    fn every_header_s_display_names_are_known_names() {
        let e = excerpt_of(
            "From: Ada Lovelace <ada@mail.example>\r\nTo: Bob Stone <bob@mail.example>\r\n\
             Cc: Carol <carol@mail.example>\r\nReply-To: Dana Desk <desk@mail.example>\r\n\
             Subject: For Ada\r\n\r\nHi Bob, Carol and Dana. Grace stays.\r\n",
            &[],
        );
        assert_eq!(e.subject().as_str(), "For <name>");
        assert_eq!(
            e.body().as_str().trim_end(),
            "Hi <name>, <name> and <name>. Grace stays."
        );
        assert_eq!(e.masks().get(Kind::Name), 4);
    }

    #[test]
    fn owner_addresses_are_masked_whole_and_others_keep_their_domain() {
        let e = excerpt_of(
            "From: a@mail.example\r\nSubject: x\r\n\r\nMail ME@Owner.example or bob@mail.example\r\n",
            &["me@owner.example"],
        );
        assert_eq!(
            e.body().as_str().trim_end(),
            "Mail <email> or <email>@mail.example"
        );
    }

    #[test]
    fn outgoing_reads_each_signal_from_its_own_header() {
        let parsed = |extra: &str| {
            let raw = format!(
                "From: Ada Lovelace <Orders@Shop.Example>\r\nTo: me@owner.example\r\n{extra}\
                 Subject: Your order\r\n\r\nHi\r\n"
            );
            nm_core::parse::parse(raw.as_bytes()).unwrap()
        };
        let signals = |extra: &str| {
            outgoing(&parsed(extra), ["me@owner.example"])
                .signals()
                .clone()
        };
        let plain = signals("");
        assert_eq!(plain.domain(), Some("shop.example"));
        assert_eq!(plain.kind(), nm_redact::SenderKind::Role);
        assert!(!plain.mailing_list() && !plain.automated() && !plain.reply());
        let list = signals("List-Id: <news.shop.example>\r\n");
        assert!(list.mailing_list() && !list.automated() && !list.reply());
        let auto = signals("Auto-Submitted: auto-generated\r\n");
        assert!(!auto.mailing_list() && auto.automated() && !auto.reply());
        let reply = signals("In-Reply-To: <a@shop.example>\r\n");
        assert!(!reply.mailing_list() && !reply.automated() && reply.reply());
    }

    #[test]
    fn owner_addresses_are_every_account_login_that_is_an_address() {
        let config = Config::parse(
            r#"
[[accounts]]
name = "work"
username = "me@work.example"
password_command = "true"
imap = { host = "imap.work.example" }

[[accounts]]
name = "bare"
username = "melogin"
password_command = "true"
imap = { host = "imap.bare.example" }

[[accounts]]
name = "outlook"
username = "you@outlook.example"
oauth = { provider = "microsoft", client_id = "00000000-0000-0000-0000-000000000000" }
"#,
        )
        .unwrap();
        assert_eq!(
            owner_addresses(&config),
            ["me@work.example", "you@outlook.example"]
        );
    }

    #[test]
    fn a_message_without_subject_or_text_body_gives_empty_text() {
        let e = excerpt_of(
            "From: a@mail.example\r\nContent-Type: application/pdf\r\n\r\nJVBERi0=\r\n",
            &[],
        );
        assert_eq!(e.subject().as_str(), "");
        assert_eq!(e.body().as_str(), "");
        assert!(!e.truncated());
    }

    #[test]
    fn neomail_s_own_html_note_is_not_sent() {
        // Deeper than nm-core lays out, so the body starts with neomail's note.
        let html = format!("{}hi Ada{}", "<div>".repeat(200), "</div>".repeat(200));
        let e = excerpt_of(
            &format!("From: Ada <ada@mail.example>\r\nContent-Type: text/html\r\n\r\n{html}\r\n"),
            &[],
        );
        assert!(
            !e.body().as_str().contains("neomail:"),
            "{}",
            e.body().as_str()
        );
        assert!(e.body().as_str().contains("hi <name>"));
    }
}
