//! Push for IMAP accounts: one watcher per account ([`nm_imap::Watcher`], IDLE on INBOX, RFC 2177)
//! whose only job is to wake its account when INBOX changes. The daemon syncs the account then,
//! through the same `Syncer` and account lock as a round; the timer rounds stay as the safety net.
//!
//! A watcher runs as a task on the daemon's runtime. It logs in with what the account's last
//! successful sync used ([`WatchLogin`], handed over outside the runtime, so it never reads the
//! keyring or runs a `password_command`) and drops that secret once it has logged in.

use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;

use nm_imap::{ImapConfig, WatchEvent, Watcher};
use tokio::runtime::Handle;
use tokio::sync::mpsc;
use tokio::task::AbortHandle;
use tokio::time::Instant;

use crate::config::Account;
use crate::daemon::Line;
use crate::oauth::Secret;

/// The shortest time between two watcher starts for one account, so a server that drops every
/// IDLE connection cannot make a loop of connections and syncs.
pub const START_EVERY: Duration = Duration::from_secs(60);

/// What an account's last successful sync logged in with: where, and the password or access token.
/// `Debug` is redacted.
pub struct WatchLogin {
    pub imap: ImapConfig,
    secret: Secret,
    xoauth2: bool,
}

impl WatchLogin {
    pub fn new(imap: &ImapConfig, login: nm_imap::Login<'_>) -> Self {
        let (secret, xoauth2) = match login {
            nm_imap::Login::Password(password) => (password, false),
            nm_imap::Login::XOAuth2 { access_token } => (access_token, true),
        };
        Self {
            imap: imap.clone(),
            secret: Secret::new(secret.to_owned()),
            xoauth2,
        }
    }

    pub fn login(&self) -> nm_imap::Login<'_> {
        let secret = self.secret.expose();
        if self.xoauth2 {
            nm_imap::Login::XOAuth2 {
                access_token: secret,
            }
        } else {
            nm_imap::Login::Password(secret)
        }
    }
}

impl std::fmt::Debug for WatchLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "WatchLogin({:?}, {:?})", self.imap, self.login())
    }
}

/// What a watcher tells the daemon, for its account and its generation (which start it was).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub account: String,
    pub generation: u64,
    pub kind: Kind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// Sync the account: INBOX changed, the first IDLE started (mail may have come in since the
    /// sync), or a watcher that was idling ended (that sync catches up and starts a new one).
    Wake,
    /// Sync the account at this time, without a settle time: sent by [`Push::start`] itself when
    /// [`START_EVERY`] held a watcher back, so that sync starts it once the minute is up.
    WakeAt(Instant),
    /// The first IDLE was accepted: push is on.
    On,
    /// The server has no IDLE in its CAPABILITY: the account keeps polling.
    NoIdle,
    /// The watcher could not start, or ended before its first IDLE: why.
    Failed(String),
    /// The watcher ended; nothing more comes from it.
    Ended,
}

/// A watcher's way to the daemon.
#[derive(Clone)]
pub struct Events {
    account: String,
    generation: u64,
    tx: mpsc::UnboundedSender<Event>,
}

impl Events {
    #[cfg(test)]
    pub fn account(&self) -> &str {
        &self.account
    }

    pub fn send(&self, kind: Kind) {
        // The daemon is gone: nobody is left to tell.
        let _ = self.tx.send(Event {
            account: self.account.clone(),
            generation: self.generation,
            kind,
        });
    }
}

/// The watcher the daemon runs: [`Watcher::connect`], then [`Watcher::run`] until the connection
/// ends. Every path ends with [`Kind::Ended`].
pub async fn watch(login: WatchLogin, events: Events) {
    let connected = Watcher::connect(&login.imap, login.login()).await;
    // Logged in, or not: either way the secret goes.
    drop(login);
    let watcher = match connected {
        Ok(Some(watcher)) => watcher,
        Ok(None) => return events.send_all([Kind::NoIdle, Kind::Ended]),
        Err(e) => return events.send_all([Kind::Failed(message(e)), Kind::Ended]),
    };
    let mut idling = false;
    let ended = watcher
        .run(|event| {
            if event == WatchEvent::Idling {
                idling = true;
                events.send(Kind::On);
            }
            events.send(Kind::Wake);
        })
        .await;
    if idling {
        events.send_all([Kind::Wake, Kind::Ended]);
    } else {
        events.send_all([Kind::Failed(message(ended)), Kind::Ended]);
    }
}

impl Events {
    fn send_all<const N: usize>(&self, kinds: [Kind; N]) {
        for kind in kinds {
            self.send(kind);
        }
    }
}

fn message(e: nm_imap::ImapError) -> String {
    format!("{:#}", anyhow::Error::new(e))
}

/// What was last said about an account's push.
#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    On,
    /// With the server that has no IDLE: another server gets a watcher again.
    NoIdle(ImapConfig),
    Failing(String),
}

struct Running {
    generation: u64,
    imap: ImapConfig,
    task: AbortHandle,
}

/// The daemon's watchers, one per IMAP account at most. `watch` starts one: [`watch`] in the
/// daemon, a fake in the tests. Dropping it ends every watcher at once.
pub struct Push<W> {
    runtime: Handle,
    watch: W,
    tx: mpsc::UnboundedSender<Event>,
    running: HashMap<String, Running>,
    started: HashMap<String, Instant>,
    state: HashMap<String, State>,
    generation: u64,
}

impl<W, F> Push<W>
where
    W: FnMut(WatchLogin, Events) -> F,
    F: Future<Output = ()> + Send + 'static,
{
    /// The watchers' events arrive on the receiver.
    pub fn new(runtime: Handle, watch: W) -> (Self, mpsc::UnboundedReceiver<Event>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let push = Self {
            runtime,
            watch,
            tx,
            running: HashMap::new(),
            started: HashMap::new(),
            state: HashMap::new(),
            generation: 0,
        };
        (push, rx)
    }

    /// After a successful sync: starts the account's watcher with `login`, unless one runs
    /// already, its server has no IDLE, or one started less than [`START_EVERY`] ago. In that
    /// last case the account gets a [`Kind::WakeAt`] for when the minute is up, whose sync starts
    /// the watcher then rather than at the next round. A watcher for another server (the account
    /// was changed) is replaced.
    pub fn start(&mut self, account: &str, login: WatchLogin) {
        if let Some(running) = self.running.get(account) {
            if running.imap == login.imap {
                return;
            }
            running.task.abort();
            self.running.remove(account);
        }
        if matches!(self.state.get(account), Some(State::NoIdle(imap)) if *imap == login.imap) {
            return;
        }
        let now = self.now();
        if let Some(last) = self.started.get(account)
            && now < *last + START_EVERY
        {
            // The daemon is gone when this fails: nobody is left to sync.
            let _ = self.tx.send(Event {
                account: account.to_owned(),
                generation: self.generation,
                kind: Kind::WakeAt(*last + START_EVERY),
            });
            return;
        }
        self.started.insert(account.to_owned(), now);
        self.generation += 1;
        let events = Events {
            account: account.to_owned(),
            generation: self.generation,
            tx: self.tx.clone(),
        };
        let imap = login.imap.clone();
        let task = self
            .runtime
            .spawn((self.watch)(login, events))
            .abort_handle();
        self.running.insert(
            account.to_owned(),
            Running {
                generation: self.generation,
                imap,
                task,
            },
        );
    }

    /// The runtime's clock (paused in the tests), read outside `block_on`.
    fn now(&self) -> Instant {
        let _entered = self.runtime.enter();
        Instant::now()
    }
}

impl<W> Push<W> {
    /// A watcher's event other than [`Kind::Wake`]: the line to print when the account's push
    /// state changed. A failure is said again only when it differs from the last one. Events of
    /// a watcher that was replaced or ended are ignored.
    pub fn event(&mut self, event: Event) -> Option<Line> {
        let account = event.account;
        let current = self
            .running
            .get(&account)
            .is_some_and(|r| r.generation == event.generation);
        if !current {
            return None;
        }
        let state = match event.kind {
            Kind::Wake | Kind::WakeAt(_) => return None,
            Kind::Ended => {
                self.running.remove(&account);
                return None;
            }
            Kind::On => State::On,
            Kind::NoIdle => State::NoIdle(self.running[&account].imap.clone()),
            Kind::Failed(why) => State::Failing(why),
        };
        if self.state.get(&account) == Some(&state) {
            return None;
        }
        let line = match &state {
            State::On => Line::Error(format!("neomail daemon: {account}: push on")),
            State::NoIdle(_) => Line::Error(format!(
                "neomail daemon: {account}: push off (the server has no IDLE)"
            )),
            State::Failing(why) => Line::Error(format!("neomail: {account}: push failing: {why}")),
        };
        self.state.insert(account, state);
        Some(line)
    }

    /// After a resume: ends every watcher, since its connection may have died while the machine
    /// slept, and lets the next syncs start new ones at once.
    pub fn replace_all(&mut self) {
        self.stop_all();
        self.started.clear();
    }

    /// Ends every watcher at once.
    pub fn stop_all(&mut self) {
        for (_, running) in self.running.drain() {
            running.task.abort();
        }
    }

    /// Ends the watchers of accounts no longer in `accounts` and forgets them.
    pub fn retain(&mut self, accounts: &[Account]) {
        let configured = |name: &String| accounts.iter().any(|a| &a.name == name);
        self.running.retain(|name, running| {
            let keep = configured(name);
            if !keep {
                running.task.abort();
            }
            keep
        });
        self.started.retain(|name, _| configured(name));
        self.state.retain(|name, _| configured(name));
    }
}

impl<W> Drop for Push<W> {
    fn drop(&mut self) {
        self.stop_all();
    }
}
