//! Syncing one account, the one code path `neomail sync` and `neomail daemon` share: the login
//! (a password, or an OAuth access token kept in memory until it is about to expire), then the
//! engine the account's server needs (IMAP or Microsoft Graph).

use std::collections::HashMap;
use std::io;
use std::num::NonZeroU32;
use std::time::{Duration, SystemTime};

use anyhow::{Result, bail};
use nm_core::{Cache, SyncReport, sync_inbox_with_progress};
use nm_imap::{ImapConfig, ImapSource};
use tokio::runtime::Runtime;

use crate::config::{Account, Login, OAuthSettings, Server};
use crate::keyring::Keyring;
use crate::oauth::{MailGrant, Refreshed, Secret};
use crate::push::WatchLogin;
use crate::{account, graph, output, secret};

/// An access token is refreshed this long before it expires, so it never runs out mid-sync.
pub const REFRESH_MARGIN: Duration = Duration::from_secs(5 * 60);

/// How long an unattended `password_command` may run: the same limit as one IMAP step.
pub const PASSWORD_COMMAND_TIMEOUT: Duration = Duration::from_secs(60);

/// What syncing accounts needs between accounts and, in the daemon, between rounds: the
/// runtime, the logins, the first sync's limit and the progress line.
pub struct Syncer<'a> {
    runtime: &'a Runtime,
    logins: Logins,
    limit: NonZeroU32,
    push_logins: bool,
    pub progress: output::ProgressLine,
}

impl<'a> Syncer<'a> {
    pub fn new(runtime: &'a Runtime, limit: NonZeroU32, progress: output::ProgressLine) -> Self {
        Self {
            runtime,
            logins: Logins::default(),
            limit,
            push_logins: false,
            progress,
        }
    }

    /// For the daemon, which nobody watches: a `password_command` cannot prompt and is stopped
    /// after `limit` ([`PASSWORD_COMMAND_TIMEOUT`]), so one stuck command (gpg waiting for
    /// pinentry) fails its account instead of stalling every account while holding its lock.
    pub fn unattended(mut self, limit: Duration) -> Self {
        self.logins.password_mode = secret::CommandMode::Unattended(limit);
        self
    }

    /// For the daemon's push: a successful IMAP sync hands over what it logged in with
    /// ([`Synced::watch`]), so the account's watcher never reads the keyring itself.
    pub fn push_logins(mut self) -> Self {
        self.push_logins = true;
        self
    }

    /// Syncs one account's INBOX into `cache`. Keyring calls stay outside `block_on`: their
    /// blocking calls start a runtime of their own, which panics inside another.
    pub fn sync(&mut self, account: &Account, cache: &mut Cache) -> Synced {
        let mut warnings = Vec::new();
        let mut stored = SyncReport::default();
        let mut watch = None;
        match self.sync_inner(account, cache, &mut stored, &mut warnings, &mut watch) {
            Ok(()) => Synced {
                report: Ok(stored),
                partial: SyncReport::default(),
                warnings,
                watch,
            },
            Err(e) => {
                self.logins.forget(&account.name);
                Synced {
                    report: Err(e),
                    partial: stored,
                    warnings,
                    watch: None,
                }
            }
        }
    }

    /// Drops what is kept for accounts no longer in `accounts`.
    pub fn retain(&mut self, accounts: &[Account]) {
        self.logins.tokens.retain(accounts);
    }

    fn sync_inner(
        &mut self,
        account: &Account,
        cache: &mut Cache,
        report: &mut SyncReport,
        warnings: &mut Vec<String>,
        watch: &mut Option<WatchLogin>,
    ) -> Result<()> {
        let runtime = self.runtime;
        let push = self.push_logins;
        // Only once the sync succeeded: its login works.
        let hand_over = |imap: &ImapConfig, login| push.then(|| WatchLogin::new(imap, login));
        let limit = self.limit;
        let progress = &mut self.progress;
        let credential = self.logins.credential(runtime, account, warnings)?;
        match (&account.server, credential) {
            (Server::Imap(imap), credential) => {
                let login = credential.imap();
                runtime.block_on(sync_imap(
                    imap,
                    &account.name,
                    login,
                    cache,
                    limit,
                    report,
                    progress,
                ))?;
                *watch = hand_over(imap, login);
                Ok(())
            }
            (Server::Graph { .. }, Credential::AccessToken(token)) => runtime.block_on(
                graph::sync(&account.name, &token.secret, cache, limit, report, progress),
            ),
            (Server::Graph { .. }, Credential::Password(_)) => {
                bail!("a Microsoft Graph account signs in with OAuth, not a password")
            }
        }
    }
}

/// The secret an account logs in with: a password, or an OAuth access token, which reaches IMAP
/// (XOAUTH2) or Microsoft Graph.
pub enum Credential<'a> {
    Password(secret::Password),
    AccessToken(&'a AccessToken),
}

/// An OAuth access token, and what the sign-in it came from lets neomail do with mail.
#[derive(Debug)]
pub struct AccessToken {
    pub secret: Secret,
    pub grant: MailGrant,
}

impl AccessToken {
    pub fn expose(&self) -> &str {
        self.secret.expose()
    }
}

impl Credential<'_> {
    /// The IMAP login for this credential.
    pub fn imap(&self) -> nm_imap::Login<'_> {
        match self {
            Self::Password(password) => nm_imap::Login::Password(password.expose()),
            Self::AccessToken(token) => nm_imap::Login::XOAuth2 {
                access_token: token.expose(),
            },
        }
    }
}

/// How accounts log in, the one owner of it for `neomail sync`, `neomail daemon` and `neomail
/// mark`: a password (`password_command` or the keyring), or an OAuth access token kept in memory
/// until it is about to expire. The keyring is opened on first use.
pub struct Logins {
    keyring: Option<Keyring>,
    tokens: Tokens,
    password_mode: secret::CommandMode,
}

impl Default for Logins {
    fn default() -> Self {
        Self {
            keyring: None,
            tokens: Tokens::default(),
            password_mode: secret::CommandMode::Interactive,
        }
    }
}

impl Logins {
    /// The account's credential. Keyring calls stay outside `block_on`: their blocking calls
    /// start a runtime of their own, which panics inside another.
    pub fn credential(
        &mut self,
        runtime: &Runtime,
        account: &Account,
        warnings: &mut Vec<String>,
    ) -> Result<Credential<'_>> {
        match &account.login {
            Login::Password(source) => {
                if matches!(account.server, Server::Graph { .. }) {
                    bail!("a password account reads mail over IMAP");
                }
                Ok(Credential::Password(secret::account_password(
                    &account.name,
                    source,
                    &mut self.keyring,
                    self.password_mode,
                )?))
            }
            Login::OAuth(settings) => {
                let keyring = &mut self.keyring;
                let token = self.tokens.get_or_refresh(account, SystemTime::now(), || {
                    refresh(runtime, keyring, account, settings, warnings)
                })?;
                Ok(Credential::AccessToken(token))
            }
        }
    }

    /// After a failure: the token may be what failed (revoked, or refused by the server), so the
    /// next login asks for a new one. A keyring that failed is opened again, in case its service
    /// restarted.
    pub fn forget(&mut self, account: &str) {
        self.tokens.forget(account);
        self.keyring = None;
    }
}

/// One account's sync: its report or error, and the warnings that did not fail it, each
/// without the `neomail: ACCOUNT: ` prefix. The caller prints them: `neomail sync` at once, the
/// daemon with the time and only when one differs from the account's last.
pub struct Synced {
    pub report: Result<SyncReport>,
    /// When the sync failed, what it changed before the failure: the batches it already stored,
    /// with their [`SyncReport::new_ids`]. Empty when it succeeded; `report` has it all then.
    pub partial: SyncReport,
    pub warnings: Vec<String>,
    /// After a successful IMAP sync by a [`Syncer::push_logins`] syncer: the login it used, for
    /// the account's watcher.
    pub watch: Option<WatchLogin>,
}

/// A new access token from the keyring's refresh token. A rotated refresh token is written back
/// outside `block_on`, like every keyring call; the old one is not revoked, so a failed write is
/// only a warning.
fn refresh(
    runtime: &Runtime,
    keyring: &mut Option<Keyring>,
    account: &Account,
    settings: &OAuthSettings,
    warnings: &mut Vec<String>,
) -> Result<Refreshed> {
    let saved = secret::oauth_secrets(&account.name, keyring)?;
    let refreshed = runtime.block_on(account::refresh(account, settings, &saved))?;
    if let Some(rotated) = refreshed.rotated(&saved.refresh_token)
        && let Err(e) = secret::save_refresh_token(&account.name, keyring, &saved, rotated)
    {
        warnings.push(format!(
            "warning: {} sent a new refresh token, but it could not be saved ({e:#}); the saved \
             one is used until it stops working",
            settings.provider.name
        ));
    }
    Ok(refreshed)
}

/// Access tokens in memory, never on disk, each until [`REFRESH_MARGIN`] before it expires. A
/// token is kept only for the whole account it was issued under, server and login included: an
/// account replaced under the same name (`account add`, or `account login` as another user)
/// gets a new token rather than the old user's.
///
/// Times are wall-clock ([`SystemTime`]), not [`std::time::Instant`]: on Linux an `Instant` is
/// `CLOCK_MONOTONIC`, which stops while the machine is suspended, so a token would outlive its
/// expiry across a laptop's sleep. A wall clock set back before a token was issued drops the token
/// too, so a clock change never stretches a token's life.
#[derive(Default)]
pub struct Tokens {
    by_account: HashMap<String, Cached>,
}

struct Cached {
    account: Account,
    token: AccessToken,
    issued_at: SystemTime,
    refresh_at: SystemTime,
}

impl Tokens {
    /// The account's access token: the one in memory while `now` is between its issue and refresh
    /// times, else a new one from `refresh`. A token whose lifetime is unknown or shorter than the margin
    /// is used once and not kept.
    pub fn get_or_refresh(
        &mut self,
        account: &Account,
        now: SystemTime,
        refresh: impl FnOnce() -> Result<Refreshed>,
    ) -> Result<&AccessToken> {
        let fresh = self
            .by_account
            .get(&account.name)
            .is_some_and(|c| c.account == *account && (c.issued_at..c.refresh_at).contains(&now));
        if !fresh {
            self.by_account.remove(&account.name);
            let refreshed = refresh()?;
            let refresh_at = refreshed
                .expires_in
                .and_then(|lifetime| lifetime.checked_sub(REFRESH_MARGIN))
                .and_then(|keep| now.checked_add(keep))
                // Used for this sync only.
                .unwrap_or(now);
            self.by_account.insert(
                account.name.clone(),
                Cached {
                    account: account.clone(),
                    token: AccessToken {
                        secret: refreshed.access_token,
                        grant: refreshed.grant,
                    },
                    issued_at: now,
                    refresh_at,
                },
            );
        }
        Ok(&self.by_account[&account.name].token)
    }

    /// Drops the tokens of accounts no longer in `accounts`.
    pub fn retain(&mut self, accounts: &[Account]) {
        self.by_account
            .retain(|name, _| accounts.iter().any(|a| &a.name == name));
    }

    /// Drops the account's token, so the next sync asks for a new one.
    pub fn forget(&mut self, account: &str) {
        self.by_account.remove(account);
    }
}

/// Syncs an IMAP account's INBOX.
async fn sync_imap(
    imap: &ImapConfig,
    account: &str,
    login: nm_imap::Login<'_>,
    cache: &mut Cache,
    limit: NonZeroU32,
    report: &mut SyncReport,
    progress: &mut output::ProgressLine,
) -> Result<()> {
    let mut source = ImapSource::connect(imap, login).await?;
    let result = sync_inbox_with_progress(&mut source, cache, account, limit, report, |p| {
        progress.update(&mut io::stderr(), account, p);
    })
    .await;
    source.logout().await;
    Ok(result?)
}

/// `sync`'s line for one account, the same for IMAP and Graph.
pub fn report_line(account: &str, r: &SyncReport) -> String {
    let reset = if r.reset {
        " (the server's mailbox is not the cached one; cache rebuilt)"
    } else {
        ""
    };
    let missing = if r.missing > 0 {
        format!(", {} without a body (retried next sync)", r.missing)
    } else {
        String::new()
    };
    format!(
        "{account}: {} new, {} updated, {} removed{missing}{reset}",
        r.new, r.updated, r.removed
    )
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::oauth::{GOOGLE, MICROSOFT, Provider};

    fn oauth(name: &str, username: &str, provider: &Provider, client_id: &str) -> Account {
        let toml = crate::config::oauth_account_toml(name, username, provider, client_id);
        crate::config::Config::parse(&toml)
            .unwrap()
            .accounts
            .remove(0)
    }

    fn gmail() -> Account {
        oauth("gmail", "you@example.com", &GOOGLE, "client")
    }

    fn answer(n: u32, expires_in: Option<u64>) -> Result<Refreshed> {
        Ok(Refreshed {
            access_token: Secret::new(format!("token-{n}")),
            grant: MailGrant::ReadWrite,
            refresh_token: None,
            expires_in: expires_in.map(Duration::from_secs),
        })
    }

    #[test]
    fn an_unattended_syncer_stops_a_stuck_password_command() {
        let config = crate::config::Config::parse(
            "[[accounts]]\nname = \"work\"\nusername = \"you@example.com\"\n\
             password_command = \"exec sleep 3600\"\nimap = { host = \"127.0.0.1\" }\n",
        )
        .unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut syncer = Syncer::new(
            &runtime,
            nm_core::sync::DEFAULT_LIMIT,
            output::ProgressLine::new(false),
        )
        .unattended(Duration::from_millis(50));
        let mut cache = Cache::open_in_memory().unwrap();
        let e = syncer
            .sync(&config.accounts[0], &mut cache)
            .report
            .unwrap_err();
        assert_eq!(
            format!("{e:#}"),
            "password_command did not finish within 0.05 s"
        );
    }

    #[test]
    fn a_token_is_reused_until_five_minutes_before_it_expires() {
        let mut tokens = Tokens::default();
        let refreshes = Cell::new(0);
        let s = gmail();
        let t0 = SystemTime::now();
        let mut at = |offset: u64| {
            tokens
                .get_or_refresh(&s, t0 + Duration::from_secs(offset), || {
                    refreshes.set(refreshes.get() + 1);
                    answer(refreshes.get(), Some(3600))
                })
                .unwrap()
                .expose()
                .to_owned()
        };
        assert_eq!(at(0), "token-1");
        // Valid for an hour: kept until 55 minutes, refreshed from then on.
        assert_eq!(at(60), "token-1");
        assert_eq!(at(55 * 60 - 1), "token-1");
        assert_eq!(at(55 * 60), "token-2");
        assert_eq!(at(56 * 60), "token-2");
        assert_eq!(at(110 * 60), "token-3");
        assert_eq!(refreshes.get(), 3);
    }

    /// The time a laptop spends suspended counts: the clock jumps past the refresh point in one
    /// step, and the next sync gets a new token rather than the expired one.
    #[test]
    fn a_token_is_refreshed_after_a_jump_past_its_refresh_time() {
        let mut tokens = Tokens::default();
        let s = gmail();
        let t0 = SystemTime::now();
        let mut n = 0;
        let mut at = |offset: u64| {
            tokens
                .get_or_refresh(&s, t0 + Duration::from_secs(offset), || {
                    n += 1;
                    answer(n, Some(3600))
                })
                .unwrap()
                .expose()
                .to_owned()
        };
        assert_eq!(at(0), "token-1");
        assert_eq!(at(5 * 60), "token-1");
        // Asleep for three hours between two rounds.
        assert_eq!(at(3 * 3600 + 5 * 60), "token-2");
    }

    /// A wall clock set back before the token was issued drops the token, so it never lives
    /// longer than its lifetime.
    #[test]
    fn a_token_is_refreshed_when_the_clock_is_set_back() {
        let mut tokens = Tokens::default();
        let s = gmail();
        let t0 = SystemTime::now();
        let mut n = 0;
        let mut at = |t: SystemTime| {
            tokens
                .get_or_refresh(&s, t, || {
                    n += 1;
                    answer(n, Some(3600))
                })
                .unwrap()
                .expose()
                .to_owned()
        };
        assert_eq!(at(t0), "token-1");
        assert_eq!(at(t0), "token-1");
        assert_eq!(at(t0 - Duration::from_secs(1)), "token-2");
    }

    #[test]
    fn a_token_without_a_usable_lifetime_is_used_once() {
        let mut tokens = Tokens::default();
        let s = gmail();
        let now = SystemTime::now();
        for (n, expires_in) in [(1, None), (2, None), (3, Some(300)), (4, Some(299))] {
            let got = tokens
                .get_or_refresh(&s, now, || answer(n, expires_in))
                .unwrap();
            assert_eq!(got.expose(), format!("token-{n}"));
        }
    }

    /// The account's token at `now`, from a refresh answering `token-{n}` if one is needed.
    fn token(tokens: &mut Tokens, account: &Account, now: SystemTime, n: u32) -> String {
        tokens
            .get_or_refresh(account, now, || answer(n, Some(3600)))
            .unwrap()
            .expose()
            .to_owned()
    }

    #[test]
    fn a_token_is_dropped_when_forgotten_or_the_account_changes() {
        let mut tokens = Tokens::default();
        let now = SystemTime::now();
        let work = |username, provider, client_id| oauth("work", username, provider, client_id);
        let first = work("you@example.com", &GOOGLE, "a");
        assert_eq!(token(&mut tokens, &first, now, 1), "token-1");
        assert_eq!(token(&mut tokens, &first.clone(), now, 2), "token-1");
        // Another client, provider or signed-in user under the same name: never the old token.
        let changes = [
            work("you@example.com", &GOOGLE, "b"),
            work("you@example.com", &MICROSOFT, "b"),
            work("someone@example.com", &MICROSOFT, "b"),
        ];
        for (n, account) in (3..).zip(&changes) {
            assert_eq!(token(&mut tokens, account, now, n), format!("token-{n}"));
        }
        let last = &changes[2];
        tokens.forget("work");
        assert_eq!(token(&mut tokens, last, now, 6), "token-6");
        assert_eq!(token(&mut tokens, last, now, 7), "token-6");
    }

    #[test]
    fn the_tokens_of_removed_accounts_are_dropped() {
        let mut tokens = Tokens::default();
        let now = SystemTime::now();
        let home = oauth("home", "you@example.com", &GOOGLE, "client");
        assert_eq!(token(&mut tokens, &gmail(), now, 1), "token-1");
        assert_eq!(token(&mut tokens, &home, now, 2), "token-2");
        tokens.retain(std::slice::from_ref(&home));
        assert_eq!(
            token(&mut tokens, &home, now, 3),
            "token-2",
            "still configured"
        );
        assert_eq!(
            token(&mut tokens, &gmail(), now, 4),
            "token-4",
            "removed, then back"
        );
    }

    #[test]
    fn a_failed_refresh_keeps_nothing() {
        let mut tokens = Tokens::default();
        let s = gmail();
        let now = SystemTime::now();
        assert!(
            tokens
                .get_or_refresh(&s, now, || bail!("invalid_grant"))
                .is_err()
        );
        let got = tokens
            .get_or_refresh(&s, now, || answer(7, Some(3600)))
            .unwrap();
        assert_eq!(got.expose(), "token-7");
    }
}
