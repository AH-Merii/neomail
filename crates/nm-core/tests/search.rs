//! `Cache::search` over caches of synthetic mail: what a query matches, the scopes,
//! and that the index follows the cache as messages change and go. No network.
#![allow(clippy::unwrap_used)] // Test helpers outside #[test] functions.

use chrono::{DateTime, Utc};
use nm_core::{
    Cache, Choice, Classification, FetchedMessage, Flags, Group, GroupFilter, GroupRules, IdListed,
    MessageId, Query, QueryError, RemoteId, Scope, Uid, UidValidity,
};

/// A message with these headers (a `Name: value` per line) and a plain body.
fn mail(uid: u32, received: i64, headers: &str, body: &str) -> FetchedMessage {
    let raw = format!(
        "{}\r\nMessage-ID: <m{uid}@mail.example>\r\n\r\n{body}\r\n",
        headers.replace('\n', "\r\n")
    );
    FetchedMessage {
        uid: Uid::new(uid).unwrap(),
        flags: Flags::default(),
        received: DateTime::from_timestamp(received, 0),
        raw: raw.into_bytes(),
    }
}

/// A message whose only difference is its subject, received at `uid` (so newer uids are newer).
fn subject(uid: u32, subject: &str) -> FetchedMessage {
    mail(uid, i64::from(uid), &format!("Subject: {subject}"), "")
}

/// A cache with `messages` in `account`'s INBOX, indexed.
fn cache_with(account: &str, messages: &[FetchedMessage]) -> Cache {
    let mut cache = Cache::open_in_memory().unwrap();
    add(&mut cache, account, messages);
    cache
}

fn add(cache: &mut Cache, account: &str, messages: &[FetchedMessage]) {
    let inbox = match cache.mailbox(account, "INBOX").unwrap() {
        Some(inbox) => inbox,
        None => cache
            .reset_mailbox(account, "INBOX", UidValidity::new(1).unwrap())
            .unwrap(),
    };
    cache.insert_messages(inbox.id, messages).unwrap();
    while cache.index_pending(500).unwrap() > 0 {}
}

fn search_in(cache: &Cache, query: &str, scope: &Scope) -> Vec<String> {
    cache
        .search(
            &Query::parse(query).unwrap(),
            scope,
            50,
            &GroupRules::default(),
            Utc::now(),
        )
        .unwrap()
        .into_iter()
        .map(|g| g.entry.subject.unwrap_or_default())
        .collect()
}

/// The subjects of the messages `query` finds anywhere, newest first.
fn search(cache: &Cache, query: &str) -> Vec<String> {
    search_in(cache, query, &Scope::everything())
}

#[test]
fn every_word_must_match_and_each_matches_as_a_prefix() {
    let cache = cache_with(
        "work",
        &[
            mail(1, 1, "Subject: Paid", "Your invoice is attached."),
            mail(2, 2, "Subject: Reminder", "The invoice is overdue."),
            mail(3, 3, "Subject: Lunch", "Pizza at noon?"),
        ],
    );
    assert_eq!(search(&cache, "invo"), ["Reminder", "Paid"]);
    assert_eq!(search(&cache, "invoice overdue"), ["Reminder"]);
    assert_eq!(search(&cache, "invoice pizza"), Vec::<String>::new());
}

#[test]
fn case_and_latin_accents_are_ignored_both_ways() {
    let cache = cache_with(
        "work",
        &[subject(1, "Café Mönch"), subject(2, "CAFE MONCH")],
    );
    for query in ["cafe", "CAFÉ", "mönch", "monch"] {
        assert_eq!(
            search(&cache, query),
            ["CAFE MONCH", "Café Mönch"],
            "{query}"
        );
    }
}

#[test]
fn arabic_marks_tatweel_and_hamza_on_alef_are_folded_but_close_words_stay_apart() {
    let cache = cache_with(
        "work",
        &[
            subject(1, "محمد"),
            subject(2, "احمد"),
            subject(3, "علي"),
            subject(4, "على"),
        ],
    );
    assert_eq!(search(&cache, "مُحَمَّد"), ["محمد"]);
    assert_eq!(search(&cache, "محـمد"), ["محمد"]);
    assert_eq!(search(&cache, "أحمد"), ["احمد"]);
    assert_eq!(search(&cache, "علي"), ["علي"], "ي and ى stay apart");
}

#[test]
fn an_html_only_body_is_searched_as_its_text() {
    let html = mail(
        1,
        1,
        "Subject: Sale\nContent-Type: text/html; charset=utf-8",
        "<p>Big <b>discounts</b> today</p>",
    );
    let cache = cache_with("work", &[html, subject(2, "Other")]);
    assert_eq!(search(&cache, "discounts"), ["Sale"]);
}

#[test]
fn a_field_restricts_its_word_to_that_part_of_the_message() {
    let cache = cache_with(
        "work",
        &[
            mail(
                1,
                1,
                "Subject: About Ada\nFrom: Bob <bob@mail.example>",
                "Ada said hi",
            ),
            mail(
                2,
                2,
                "Subject: Hello\nFrom: Ada Lovelace <ada@mail.example>",
                "",
            ),
            mail(
                3,
                3,
                "Subject: Copied\nTo: x@mail.example\nCc: Ada <ada@else.example>",
                "",
            ),
        ],
    );
    assert_eq!(search(&cache, "ada"), ["Copied", "Hello", "About Ada"]);
    assert_eq!(search(&cache, "from:ada"), ["Hello"]);
    assert_eq!(search(&cache, "FROM:ada@mail"), ["Hello"]);
    assert_eq!(search(&cache, "to:ada"), ["Copied"]);
    assert_eq!(search(&cache, "subject:ada"), ["About Ada"]);
    assert_eq!(search(&cache, "subject:\"about ada\""), ["About Ada"]);
    assert_eq!(
        search(&cache, "note:ada"),
        Vec::<String>::new(),
        "an unknown name: is text"
    );
}

#[test]
fn a_quoted_phrase_matches_its_words_in_order() {
    let cache = cache_with(
        "work",
        &[subject(1, "pay now please"), subject(2, "now pay")],
    );
    assert_eq!(search(&cache, "\"pay now\""), ["pay now please"]);
    assert_eq!(
        search(&cache, "\"pay no"),
        Vec::<String>::new(),
        "a phrase is whole words"
    );
    assert_eq!(search(&cache, "pay now"), ["now pay", "pay now please"]);
}

#[test]
fn fts_syntax_in_a_query_is_text() {
    let cache = cache_with(
        "work",
        &[subject(1, "near or not"), subject(2, "something else")],
    );
    for query in [
        "NEAR", "near(", "OR", "or*", "\"or\"", "-not", "not)", "^near", "{near}",
    ] {
        assert_eq!(search(&cache, query), ["near or not"], "{query}");
    }
    assert!(Query::parse("- * \" ( )").unwrap().is_empty());
}

#[test]
fn a_field_without_a_word_is_refused() {
    for query in ["from:", "subject: invoice", "to:"] {
        assert!(
            matches!(Query::parse(query), Err(QueryError::EmptyField(_))),
            "{query}"
        );
    }
}

fn id_of(cache: &Cache, subject: &str) -> MessageId {
    cache
        .list(None, 100)
        .unwrap()
        .into_iter()
        .find(|e| e.subject.as_deref() == Some(subject))
        .unwrap()
        .id
}

fn scope(account: Option<&str>, group: GroupFilter, thread: Option<MessageId>) -> Scope {
    Scope {
        account: account.map(str::to_owned),
        group,
        thread,
    }
}

#[test]
fn the_account_group_and_thread_scopes_narrow_the_search() {
    let mut cache = cache_with(
        "work",
        &[
            mail(1, 1, "Subject: root note", ""),
            mail(
                2,
                2,
                "Subject: reply note\nIn-Reply-To: <m1@mail.example>\nReferences: <m1@mail.example>",
                "",
            ),
            mail(
                3,
                3,
                "Subject: second reply\nIn-Reply-To: <m2@mail.example>\n\
                 References: <m1@mail.example> <m2@mail.example>",
                "",
            ),
            mail(4, 4, "Subject: unrelated note", ""),
        ],
    );
    add(
        &mut cache,
        "home",
        &[mail(
            9,
            9,
            "Subject: home note\nReferences: <m1@mail.example>",
            "",
        )],
    );
    let result = Classification {
        choice: Choice::Newsletters,
        confidence: 0.9,
        probabilities: vec![(Choice::Newsletters, 0.9)],
        model: "jev-test".into(),
        request_version: 1,
        input_tokens: None,
        classified_at: DateTime::from_timestamp(5, 0).unwrap(),
    };
    cache
        .store_classification("work", "m4@mail.example", &result)
        .unwrap();
    let any = GroupFilter::Any;

    assert_eq!(
        search_in(&cache, "note", &scope(Some("home"), any, None)),
        ["home note"]
    );
    let newsletters = GroupFilter::Is(Group::Newsletters);
    assert_eq!(
        search_in(&cache, "note", &scope(None, newsletters, None)),
        ["unrelated note"]
    );
    assert_eq!(
        search_in(
            &cache,
            "note",
            &scope(None, GroupFilter::Unclassified, None)
        ),
        ["home note", "reply note", "root note"]
    );
    let thread = Some(id_of(&cache, "second reply"));
    assert_eq!(
        search_in(&cache, "", &scope(None, any, thread)),
        ["second reply", "reply note", "root note"],
        "the whole thread, in its own account only"
    );
    assert_eq!(
        search_in(
            &cache,
            "note",
            &scope(None, any, Some(id_of(&cache, "root note")))
        ),
        ["reply note", "root note"]
    );
    assert_eq!(
        search_in(&cache, "", &scope(Some("home"), any, thread)),
        Vec::<String>::new()
    );
    assert_eq!(
        search_in(&cache, "", &scope(None, any, Some(MessageId::new(999)))),
        Vec::<String>::new(),
        "an uncached thread message finds nothing"
    );
}

#[test]
fn a_thread_joins_across_proton_bridges_own_references_id() {
    // Bridge appends `<ID@protonmail.internalid>` to every message's References, a thread's
    // first message too, after any real ids.
    let cache = cache_with(
        "proton",
        &[
            mail(
                1,
                1,
                "Subject: root note\nReferences: <p1@protonmail.internalid>",
                "",
            ),
            mail(
                2,
                2,
                "Subject: reply note\nIn-Reply-To: <m1@mail.example>\n\
                 References: <m1@mail.example> <p2@protonmail.internalid>",
                "",
            ),
            mail(
                3,
                3,
                "Subject: other note\nReferences: <p3@protonmail.internalid>",
                "",
            ),
        ],
    );
    let any = GroupFilter::Any;
    for from in ["root note", "reply note"] {
        assert_eq!(
            search_in(&cache, "", &scope(None, any, Some(id_of(&cache, from)))),
            ["reply note", "root note"],
            "the thread of {from}"
        );
    }
    assert_eq!(
        search_in(
            &cache,
            "",
            &scope(None, any, Some(id_of(&cache, "other note")))
        ),
        ["other note"]
    );
}

#[test]
fn results_are_newest_first_up_to_the_limit() {
    let cache = cache_with(
        "work",
        &[
            subject(2, "b note"),
            subject(3, "c note"),
            subject(1, "a note"),
        ],
    );
    let found = cache
        .search(
            &Query::parse("note").unwrap(),
            &Scope::everything(),
            2,
            &GroupRules::default(),
            Utc::now(),
        )
        .unwrap();
    let subjects: Vec<_> = found.iter().map(|g| g.entry.subject.as_deref()).collect();
    assert_eq!(subjects, [Some("c note"), Some("b note")]);
}

#[test]
fn a_message_stored_again_is_found_by_its_new_text_only() {
    let mut cache = cache_with("work", &[subject(1, "draft one")]);
    add(&mut cache, "work", &[subject(1, "final two")]);
    assert_eq!(search(&cache, "draft"), Vec::<String>::new());
    assert_eq!(search(&cache, "final"), ["final two"]);
}

/// How many rows of the stored index match `word`: the text a deleted message must not leave
/// behind on disk, whether or not a search would return it.
fn stored_matches(path: &std::path::Path, word: &str) -> i64 {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT count(*) FROM message_fts WHERE message_fts MATCH ?1",
            [word],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn a_message_that_leaves_the_cache_leaves_no_text_in_the_index() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite3");
    let mut cache = Cache::open(&path).unwrap();
    add(
        &mut cache,
        "work",
        &[
            subject(1, "gone by id"),
            subject(2, "expunged"),
            subject(3, "kept"),
        ],
    );
    assert_eq!(stored_matches(&path, "gone"), 1);
    cache.remove(&[id_of(&cache, "gone by id")]).unwrap();
    let inbox = cache.mailbox("work", "INBOX").unwrap().unwrap();
    cache
        .apply_changes(inbox.id, &[], &[Uid::new(2).unwrap()])
        .unwrap();
    assert_eq!(search(&cache, "gone"), Vec::<String>::new());
    assert_eq!(search(&cache, "expunged"), Vec::<String>::new());
    assert_eq!(search(&cache, "kept"), ["kept"]);
    assert_eq!(stored_matches(&path, "gone"), 0, "removed");
    assert_eq!(stored_matches(&path, "expunged"), 0, "expunged");

    cache
        .reset_mailbox("work", "INBOX", UidValidity::new(2).unwrap())
        .unwrap();
    assert_eq!(search(&cache, "kept"), Vec::<String>::new());
    assert_eq!(stored_matches(&path, "kept"), 0, "a rebuild");

    let outlook = cache.reset_id_mailbox("out", "INBOX").unwrap();
    let listed = IdListed {
        id: RemoteId::new("AAA").unwrap(),
        flags: Flags::default(),
        received: DateTime::from_timestamp(1, 0).unwrap(),
    };
    let raw = b"Subject: by id\r\n\r\nhi\r\n";
    cache
        .store_id_messages(outlook.id, &[(&listed, &raw[..])], None)
        .unwrap();
    while cache.index_pending(500).unwrap() > 0 {}
    assert_eq!(search(&cache, "id"), ["by id"]);
    assert_eq!(stored_matches(&path, "id"), 1);
    cache
        .apply_id_changes(outlook.id, &[], std::slice::from_ref(&listed.id))
        .unwrap();
    assert_eq!(search(&cache, "id"), Vec::<String>::new());
    assert_eq!(stored_matches(&path, "id"), 0, "deleted by id");
}

#[test]
fn indexing_goes_in_batches_and_resumes_where_it_stopped() {
    let mut cache = Cache::open_in_memory().unwrap();
    let inbox = cache
        .reset_mailbox("work", "INBOX", UidValidity::new(1).unwrap())
        .unwrap();
    let messages = [
        subject(1, "x one"),
        subject(2, "x two"),
        subject(3, "x three"),
    ];
    cache.insert_messages(inbox.id, &messages).unwrap();
    assert_eq!(cache.unindexed_count().unwrap(), 3);
    assert_eq!(cache.index_pending(2).unwrap(), 2);
    assert_eq!(cache.unindexed_count().unwrap(), 1);
    assert_eq!(search(&cache, "x").len(), 2, "what is indexed is found");
    assert_eq!(cache.index_pending(2).unwrap(), 1);
    assert_eq!(cache.index_pending(2).unwrap(), 0);
    assert_eq!(search(&cache, "x"), ["x three", "x two", "x one"]);
}
