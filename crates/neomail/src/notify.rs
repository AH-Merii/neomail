//! Desktop notifications of new mail from `neomail daemon`: one per account per round, after the
//! round. On Linux they go to the freedesktop notification service over the session bus
//! (<https://specifications.freedesktop.org/notification/latest/protocol.html>); elsewhere they
//! are off for now.
//!
//! The daemon reaches the service through a function (`notify_round` in `daemon.rs` takes a
//! closure, like its `sync`), so the tests use a fake; the D-Bus call itself is tested against a
//! server on a peer-to-peer connection.

use std::fmt;
use std::time::Duration;

use anyhow::Result;
use nm_core::{MessageId, SyncReport, cache::ListEntry};
use tokio::runtime::Runtime;
use unicode_segmentation::UnicodeSegmentation;

use crate::output::one_line;

/// How long one call to the notification service may take, so one that hangs never stalls a
/// round.
pub const TIMEOUT: Duration = Duration::from_secs(5);

/// How many of the new messages a notification names.
pub const SHOWN: usize = 3;

/// The longest subject shown, in characters (grapheme clusters), the ellipsis included.
const SUBJECT_MAX: usize = 80;

/// The new mail of one account in one round, from the sync's report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMail {
    pub account: String,
    pub ids: Vec<MessageId>,
}

impl NewMail {
    /// The messages a sync added, unless it started from an empty mailbox (the account's first
    /// sync, or a rebuild): those were not new mail.
    pub fn from_report(account: &str, report: &SyncReport) -> Option<Self> {
        (!report.initial && !report.new_ids.is_empty()).then(|| Self {
            account: account.to_owned(),
            ids: report.new_ids.clone(),
        })
    }
}

/// What a notification shows: a plain-text summary, and a body that is safe to read as markup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub summary: String,
    pub body: String,
}

impl Notification {
    /// "ACCOUNT: N new messages", then up to [`SHOWN`] of `newest` (newest first) as "Sender:
    /// Subject", and "and N more" for the rest of `count`.
    pub fn new_mail(account: &str, count: usize, newest: &[ListEntry]) -> Self {
        let s = if count == 1 { "" } else { "s" };
        let summary = format!("{}: {count} new message{s}", clean(account));
        let shown = &newest[..newest.len().min(SHOWN)];
        let mut lines: Vec<String> = shown
            .iter()
            .map(|e| escape(&format!("{}: {}", sender(e), subject(e))))
            .collect();
        if count > shown.len() {
            lines.push(format!("and {} more", count - shown.len()));
        }
        Self {
            summary,
            body: lines.join("\n"),
        }
    }
}

/// The From display name, else the address.
fn sender(e: &ListEntry) -> String {
    let from = e.from.as_ref();
    [
        from.and_then(|f| f.name.as_deref()),
        from.and_then(|f| f.address.as_deref()),
    ]
    .into_iter()
    .flatten()
    .map(clean)
    .find(|s| !s.is_empty())
    .unwrap_or_else(|| "(unknown sender)".to_owned())
}

/// The subject cut to [`SUBJECT_MAX`] with an ellipsis, or "(no subject)".
fn subject(e: &ListEntry) -> String {
    let subject = clean(e.subject.as_deref().unwrap_or(""));
    if subject.is_empty() {
        return "(no subject)".to_owned();
    }
    let graphemes: Vec<&str> = subject.graphemes(true).collect();
    if graphemes.len() <= SUBJECT_MAX {
        return subject;
    }
    let mut cut = graphemes[..SUBJECT_MAX - 1].concat().trim_end().to_owned();
    cut.push('…');
    cut
}

/// One line without control characters or bidi controls, as the plain output makes it.
fn clean(s: &str) -> String {
    one_line(s).trim().to_owned()
}

/// The body may be read as markup (the "body-markup" capability,
/// <https://specifications.freedesktop.org/notification/latest/protocol.html>), so its text
/// escapes the characters markup gives a meaning to.
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Whether the daemon notifies, as its start line says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    On,
    NoService,
    Disabled,
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    Unsupported,
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Status::On => "notifications on",
            Status::NoService => "notifications off (no notification service)",
            Status::Disabled => "notifications off (--no-notify)",
            Status::Unsupported => "notifications off (not supported here yet)",
        })
    }
}

/// What the daemon does with a round's new mail, from [`Notifier::ready`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub enum Ready {
    /// Notify.
    On,
    /// Notify: the notification service was not there at start, and is now.
    Found,
    /// Do not notify.
    Off,
}

/// The daemon's way to the notification service: looked for at start, and again before a
/// round's notifications while it has not been found.
pub struct Notifier {
    status: Status,
    /// The session bus, while it works; connected again after a failure.
    #[cfg(target_os = "linux")]
    connection: Option<zbus::Connection>,
}

impl Notifier {
    /// Off with `--no-notify` (without touching D-Bus) or where notifications are not supported;
    /// on when the session bus has a notification service, running or activatable.
    pub fn start(runtime: &Runtime, no_notify: bool) -> Self {
        if no_notify {
            return Self::off(Status::Disabled);
        }
        #[cfg(target_os = "linux")]
        {
            match look(runtime) {
                Some(connection) => Self {
                    status: Status::On,
                    connection: Some(connection),
                },
                None => Self::off(Status::NoService),
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = runtime;
            Self::off(Status::Unsupported)
        }
    }

    /// Whether to notify now. A notifier that found no service at start looks again, within
    /// [`TIMEOUT`]: a desktop can start its notification service after the daemon. Runs the
    /// runtime itself, so it is called outside `block_on`, like [`send`](Self::send).
    pub fn ready(&mut self, runtime: &Runtime) -> Ready {
        #[cfg(target_os = "linux")]
        {
            self.ready_with(|| look(runtime))
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = runtime;
            Ready::Off
        }
    }

    /// [`ready`](Self::ready), with `look` as the search for a service.
    #[cfg(target_os = "linux")]
    fn ready_with(&mut self, look: impl FnOnce() -> Option<zbus::Connection>) -> Ready {
        match self.status {
            Status::On => Ready::On,
            Status::NoService => match look() {
                Some(connection) => {
                    self.status = Status::On;
                    self.connection = Some(connection);
                    Ready::Found
                }
                None => Ready::Off,
            },
            Status::Disabled | Status::Unsupported => Ready::Off,
        }
    }

    fn off(status: Status) -> Self {
        Self {
            status,
            #[cfg(target_os = "linux")]
            connection: None,
        }
    }

    pub fn status(&self) -> Status {
        self.status
    }

    /// Shows `n`, within [`TIMEOUT`]. Runs the runtime itself, so it is called outside
    /// `block_on`, like the keyring.
    pub fn send(&mut self, runtime: &Runtime, n: &Notification) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            let connection = self.connection.take();
            let sent = runtime.block_on(within(async {
                let connection = match connection {
                    Some(c) => c,
                    None => zbus::Connection::session().await?,
                };
                desktop::notify(&connection, n).await?;
                Ok(connection)
            }));
            // A connection that failed is dropped; the next notification connects again.
            self.connection = Some(sent?);
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (runtime, n);
            anyhow::bail!("{}", Status::Unsupported)
        }
    }
}

/// The session bus, if a notification service owns its name there or can be started for it;
/// looked for within [`TIMEOUT`].
#[cfg(target_os = "linux")]
fn look(runtime: &Runtime) -> Option<zbus::Connection> {
    let found = runtime.block_on(within(async {
        let connection = zbus::Connection::session().await?;
        Ok(desktop::available(&connection).await?.then_some(connection))
    }));
    found.ok().flatten()
}

/// `call` within [`TIMEOUT`], with the error a person can read.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
async fn within<T>(call: impl Future<Output = Result<T>>) -> Result<T> {
    match tokio::time::timeout(TIMEOUT, call).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(e)) => Err(e.context("could not show a desktop notification")),
        Err(_) => anyhow::bail!(
            "could not show a desktop notification: the notification service did not answer \
             within {} s",
            TIMEOUT.as_secs()
        ),
    }
}

/// The freedesktop Desktop Notifications calls.
#[cfg(target_os = "linux")]
pub mod desktop {
    use std::collections::HashMap;

    use zbus::Connection;
    use zbus::names::BusName;
    use zbus::zvariant::Value;

    use super::Notification;

    pub const NAME: &str = "org.freedesktop.Notifications";
    pub const PATH: &str = "/org/freedesktop/Notifications";

    /// Whether a notification service owns its name on the bus or can be started for it.
    pub async fn available(connection: &Connection) -> zbus::Result<bool> {
        let bus = zbus::fdo::DBusProxy::new(connection).await?;
        if bus.name_has_owner(BusName::try_from(NAME)?).await? {
            return Ok(true);
        }
        let activatable = bus.list_activatable_names().await?;
        Ok(activatable.iter().any(|n| n.as_str() == NAME))
    }

    /// `Notify` (<https://specifications.freedesktop.org/notification/latest/protocol.html>):
    /// - app_icon "mail-unread", the icon naming spec's "icon used for an electronic mail that
    ///   is unread" (<https://specifications.freedesktop.org/icon-naming/latest/#status>);
    /// - hints: category "email.arrived", "A new e-mail notification"
    ///   (<https://specifications.freedesktop.org/notification/latest/categories.html>), and
    ///   urgency 1, normal, a BYTE
    ///   (<https://specifications.freedesktop.org/notification/latest/hints.html>);
    /// - expire_timeout -1: the server's own setting.
    ///
    /// Returns the server's id for the notification.
    pub async fn notify(connection: &Connection, n: &Notification) -> zbus::Result<u32> {
        let hints: HashMap<&str, Value<'_>> = HashMap::from([
            ("category", Value::from("email.arrived")),
            ("urgency", Value::from(1u8)),
        ]);
        let actions: Vec<&str> = Vec::new();
        let reply = connection
            .call_method(
                Some(NAME),
                PATH,
                Some(NAME),
                "Notify",
                &(
                    "neomail",
                    0u32,
                    "mail-unread",
                    n.summary.as_str(),
                    n.body.as_str(),
                    actions,
                    hints,
                    -1i32,
                ),
            )
            .await?;
        reply.body().deserialize()
    }
}

#[cfg(test)]
mod tests {
    use nm_core::Flags;
    use nm_core::parse::EmailAddress;

    use super::*;

    fn entry(
        id: i64,
        name: Option<&str>,
        address: Option<&str>,
        subject: Option<&str>,
    ) -> ListEntry {
        ListEntry {
            id: MessageId::new(id),
            account: "work".to_owned(),
            uid: None,
            date: None,
            from: Some(EmailAddress {
                name: name.map(str::to_owned),
                address: address.map(str::to_owned),
            }),
            subject: subject.map(str::to_owned),
            flags: Flags::default(),
        }
    }

    fn message(n: i64) -> ListEntry {
        let address = format!("sender{n}@example.com");
        entry(
            n,
            Some(&format!("Sender {n}")),
            Some(&address),
            Some(&format!("Subject {n}")),
        )
    }

    #[test]
    fn one_message() {
        let n = Notification::new_mail("work", 1, &[message(1)]);
        assert_eq!(n.summary, "work: 1 new message");
        assert_eq!(n.body, "Sender 1: Subject 1");
    }

    #[test]
    fn three_messages_are_all_named() {
        let newest: Vec<_> = [3, 2, 1].into_iter().map(message).collect();
        let n = Notification::new_mail("work", 3, &newest);
        assert_eq!(n.summary, "work: 3 new messages");
        assert_eq!(
            n.body,
            "Sender 3: Subject 3\nSender 2: Subject 2\nSender 1: Subject 1"
        );
    }

    #[test]
    fn five_messages_name_the_newest_three_and_count_the_rest() {
        // More entries than shown are given: only the first three count.
        let newest: Vec<_> = [5, 4, 3, 2].into_iter().map(message).collect();
        let n = Notification::new_mail("work", 5, &newest);
        assert_eq!(n.summary, "work: 5 new messages");
        assert_eq!(
            n.body,
            "Sender 5: Subject 5\nSender 4: Subject 4\nSender 3: Subject 3\nand 2 more"
        );
    }

    #[test]
    fn the_body_escapes_markup_and_the_summary_stays_plain() {
        let e = entry(1, Some("A&B <x>"), None, Some("<b>1 > 0 & 2</b>"));
        let n = Notification::new_mail("R&D", 1, &[e]);
        assert_eq!(n.summary, "R&D: 1 new message");
        assert_eq!(
            n.body,
            "A&amp;B &lt;x&gt;: &lt;b&gt;1 &gt; 0 &amp; 2&lt;/b&gt;"
        );
    }

    #[test]
    fn control_and_bidi_characters_are_cleaned_from_every_field() {
        let e = entry(
            1,
            Some("Ev\u{202E}il\u{1b}[31m"),
            None,
            Some("Line\none\u{2066}\ttwo\u{7}"),
        );
        let n = Notification::new_mail("wo\u{200F}rk\r", 1, &[e]);
        assert_eq!(n.summary, "wo rk: 1 new message");
        assert_eq!(n.body, "Ev il [31m: Line one  two");
    }

    #[test]
    fn line_separators_cannot_fake_another_line() {
        let e = entry(
            1,
            Some("Eve\u{2028}Bank: Pay now"),
            None,
            Some("Hi\u{2029}Boss: Wire\u{85}it"),
        );
        let n = Notification::new_mail("work", 1, &[e]);
        assert_eq!(n.body, "Eve Bank: Pay now: Hi Boss: Wire it");
        assert_eq!(n.body.lines().count(), 1);
    }

    #[test]
    fn a_long_subject_is_cut_to_80_characters_with_an_ellipsis() {
        let long = "é".repeat(100);
        let n = Notification::new_mail("work", 1, &[entry(1, Some("A"), None, Some(&long))]);
        let subject = n.body.strip_prefix("A: ").unwrap();
        assert_eq!(subject.chars().count(), 80);
        assert!(subject.ends_with('…'));
        let exact = "x".repeat(80);
        let n = Notification::new_mail("work", 1, &[entry(1, Some("A"), None, Some(&exact))]);
        assert_eq!(n.body, format!("A: {exact}"), "80 fit");
    }

    #[test]
    fn no_subject_and_a_sender_without_a_name() {
        let blank = entry(1, Some("  "), Some("you@example.com"), Some("\u{7} "));
        let none = entry(2, None, Some("you@example.com"), None);
        let nobody = ListEntry {
            from: None,
            ..entry(3, None, None, None)
        };
        let n = Notification::new_mail("work", 3, &[blank, none, nobody]);
        assert_eq!(
            n.body,
            "you@example.com: (no subject)\nyou@example.com: (no subject)\n\
             (unknown sender): (no subject)"
        );
    }

    #[test]
    fn an_initial_sync_or_one_without_new_mail_is_not_new_mail() {
        let ids = vec![MessageId::new(1), MessageId::new(2)];
        let report = |initial, new_ids: &[MessageId]| SyncReport {
            new: new_ids.len(),
            initial,
            new_ids: new_ids.to_vec(),
            ..SyncReport::default()
        };
        assert_eq!(NewMail::from_report("work", &report(true, &ids)), None);
        assert_eq!(NewMail::from_report("work", &report(false, &[])), None);
        assert_eq!(
            NewMail::from_report("work", &report(false, &ids)),
            Some(NewMail {
                account: "work".to_owned(),
                ids
            })
        );
    }

    #[test]
    fn no_notify_turns_notifications_off_without_looking_for_a_service() {
        let runtime = crate::account::runtime().unwrap();
        let notifier = Notifier::start(&runtime, true);
        assert_eq!(notifier.status(), Status::Disabled);
        assert_eq!(
            notifier.status().to_string(),
            "notifications off (--no-notify)"
        );
    }

    /// On paused time, which jumps to the deadline once nothing else can run.
    #[test]
    fn a_call_that_never_answers_ends_at_the_timeout() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .start_paused(true)
            .build()
            .unwrap();
        let e = runtime
            .block_on(within(std::future::pending::<Result<()>>()))
            .unwrap_err();
        assert_eq!(
            e.to_string(),
            "could not show a desktop notification: the notification service did not answer \
             within 5 s"
        );
    }

    #[cfg(target_os = "linux")]
    mod dbus {
        use std::collections::HashMap;
        use std::sync::{Arc, Mutex};

        use zbus::zvariant::OwnedValue;

        use super::super::{Notification, Notifier, Ready, Status, desktop};

        /// What the fake service received: the method's arguments, by name.
        #[derive(Debug, Clone, PartialEq)]
        struct Call {
            app_name: String,
            replaces_id: u32,
            app_icon: String,
            summary: String,
            body: String,
            actions: Vec<String>,
            category: String,
            urgency: u8,
            hints: Vec<String>,
            expire_timeout: i32,
        }

        struct Fake(Arc<Mutex<Vec<Call>>>);

        #[zbus::interface(name = "org.freedesktop.Notifications")]
        impl Fake {
            #[allow(clippy::too_many_arguments)] // The spec's signature.
            fn notify(
                &self,
                app_name: String,
                replaces_id: u32,
                app_icon: String,
                summary: String,
                body: String,
                actions: Vec<String>,
                hints: HashMap<String, OwnedValue>,
                expire_timeout: i32,
            ) -> u32 {
                let mut names: Vec<String> = hints.keys().cloned().collect();
                names.sort();
                let call = Call {
                    app_name,
                    replaces_id,
                    app_icon,
                    summary,
                    body,
                    actions,
                    category: String::try_from(hints["category"].clone()).unwrap(),
                    urgency: u8::try_from(&hints["urgency"]).unwrap(),
                    hints: names,
                    expire_timeout,
                };
                self.0.lock().unwrap().push(call);
                42
            }
        }

        /// A fake notification service on the other end of a socket pair, and the client's
        /// connection to it: no bus at all.
        async fn pair(calls: &Arc<Mutex<Vec<Call>>>) -> (zbus::Connection, zbus::Connection) {
            let (ours, theirs) = tokio::net::UnixStream::pair().unwrap();
            let guid = zbus::Guid::generate();
            let server = zbus::connection::Builder::unix_stream(theirs)
                .server(guid)
                .unwrap()
                .p2p()
                .serve_at(desktop::PATH, Fake(Arc::clone(calls)))
                .unwrap()
                .build();
            let client = zbus::connection::Builder::unix_stream(ours).p2p().build();
            let (server, client) = tokio::join!(server, client);
            (server.unwrap(), client.unwrap())
        }

        fn new_mail() -> Notification {
            Notification {
                summary: "work: 1 new message".to_owned(),
                body: "Ada: Hello".to_owned(),
            }
        }

        #[test]
        fn notify_calls_the_spec_s_method_with_its_arguments_and_hints() {
            let runtime = crate::account::runtime().unwrap();
            let calls = Arc::new(Mutex::new(Vec::new()));
            let id = runtime.block_on(async {
                let (_server, client) = pair(&calls).await;
                desktop::notify(&client, &new_mail()).await.unwrap()
            });
            assert_eq!(id, 42, "the server's id comes back");
            assert_eq!(
                *calls.lock().unwrap(),
                [Call {
                    app_name: "neomail".to_owned(),
                    replaces_id: 0,
                    app_icon: "mail-unread".to_owned(),
                    summary: "work: 1 new message".to_owned(),
                    body: "Ada: Hello".to_owned(),
                    actions: Vec::new(),
                    category: "email.arrived".to_owned(),
                    urgency: 1,
                    hints: vec!["category".to_owned(), "urgency".to_owned()],
                    expire_timeout: -1,
                }]
            );
        }

        /// A desktop can start its notification service after the daemon: a notifier that
        /// found none looks again before a round's notifications and keeps what it finds.
        #[test]
        fn a_service_that_starts_after_the_daemon_is_found_and_used() {
            let runtime = crate::account::runtime().unwrap();
            let calls = Arc::new(Mutex::new(Vec::new()));
            let (_server, client) = runtime.block_on(pair(&calls));

            let mut notifier = Notifier::off(Status::NoService);
            assert_eq!(notifier.ready_with(|| None), Ready::Off, "still none");
            assert_eq!(notifier.status(), Status::NoService);
            assert_eq!(notifier.ready_with(|| Some(client)), Ready::Found);
            assert_eq!(notifier.status(), Status::On);
            assert_eq!(
                notifier.ready_with(|| panic!("a notifier that is on looked again")),
                Ready::On
            );
            notifier.send(&runtime, &new_mail()).unwrap();
            assert_eq!(
                calls.lock().unwrap().len(),
                1,
                "the found connection is used"
            );

            let mut disabled = Notifier::off(Status::Disabled);
            assert_eq!(
                disabled.ready_with(|| panic!("--no-notify looked for a service")),
                Ready::Off
            );
        }
    }
}
