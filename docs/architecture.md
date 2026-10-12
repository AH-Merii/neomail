# Architecture

This page follows the code: it describes what exists, not what is planned. Product decisions live
in [decisions.md](decisions.md). The slices so far follow
[DEC-2026-10-11-build-first](decisions.md#dec-2026-10-11-build-first): a read-only INBOX in the
CLI (slice 1), IMAP passwords in the OS keyring (slice 2), Gmail through the user's own OAuth
client (slice 3) and getting started and setup (slice 4).

## Crates

| Crate | What it holds | Why it is a crate |
| ----- | ------------- | ----------------- |
| `crates/neomail` | The binary: CLI (clap), config, passwords (command or OS keyring), OAuth sign-in, output, wiring. | The composition root. Nothing depends on it. |
| `crates/nm-core` | Domain types, the `MailSource` trait, the SQLite cache, the sync engine, mail parsing. | The seam: providers and front ends depend on it, and it never touches the network, so its tests run offline. |
| `crates/nm-imap` | `ImapSource`: TCP, TLS or STARTTLS (rustls), certificate pinning, IMAP (async-imap). | The first `MailSource`. It keeps the TLS and IMAP dependencies away from the core; Graph and the Gmail API become sibling crates. |

A new crate needs a reason of the same kind: a second implementation of a seam, or a heavy
dependency to isolate.

## Seams

**`MailSource`** (`nm-core/src/source.rs`) is a read-only view of one account's INBOX:

- `examine_inbox()` returns UIDVALIDITY and the message count;
- `list_all()` returns every UID with its flags and INTERNALDATE (for the first sync), and
  `flags_from(uid)` the UIDs from `uid` up with their flags;
- `fetch(uids)` returns whole messages without marking them read.

The contract is in the trait's docs: a source never changes the server, and the engine filters
and sorts whatever comes back. The sync tests drive the engine through a fake source
(`nm-core/tests/sync.rs`). `nm-imap/src/wire_tests.rs` runs `ImapSource` and the engine against
a scripted IMAP server over an in-memory pipe and checks the commands on the wire: `EXAMINE`,
`BODY.PEEK[]`, and never `SELECT`, `STORE`, `COPY`, `MOVE`, `EXPUNGE` or `APPEND`. The IMAP
source is also checked live against Proton Bridge.

**The password** comes from one of two sources, fixed per account when the config is parsed
(`PasswordSource` in `neomail/src/config.rs`): the account's `password_command`, run with `sh -c`,
or, without one, the OS keyring entry with service `neomail` and the account name as the user.
An account with a command never opens the keyring. `secret::account_password`
(`neomail/src/secret.rs`) resolves it into a `Password` whose `Debug` is redacted; it is passed to
`ImapSource::connect` and dropped after login. `neomail account set-password` and
`delete-password` write and remove the keyring entry; the new password is prompted for without
echo (rpassword) or read as one line of standard input.

`Keyring` (`neomail/src/keyring.rs`) holds the platform's store, chosen when it is opened: the
Secret Service over the D-Bus session bus on Linux, through zbus (pure Rust, so the static musl
build needs no libdbus), and the login keychain on macOS. Both come from keyring-core's store
crates; the tests give `Keyring` keyring-core's in-memory mock store instead, so no test writes to
a real keyring, and the CLI tests point D-Bus at a socket that does not exist. zbus runs its
blocking calls on a tokio runtime of its own, which panics inside another runtime, so `neomail
sync` reads each account's password before it enters `block_on`. When the keyring cannot be
reached the error says so and suggests `password_command`, adding a WSL note when
`WSL_DISTRO_NAME` is set or `/proc/sys/fs/binfmt_misc/WSLInterop` exists.

Neither source is a trait: `PasswordSource` is a closed enum matched in one function, and
keyring-core's `CredentialStore` already is the seam between platforms and the mock.

**The login method** is fixed per account when the config is parsed (`Login` in
`neomail/src/config.rs`): a password from a `PasswordSource`, or OAuth with a provider and the
user's client ID (`oauth = { provider = "google", client_id = "…" }`; both at once is an error).
`nm_imap::Login` carries the credential into `ImapSource::connect`: `LOGIN` with a password, or
`AUTHENTICATE XOAUTH2` with an access token. A failed XOAUTH2 login gets Gmail's base64 JSON
challenge, which is answered with an empty response so the server ends with `NO`; the whole
exchange has one 60-second bound. `MailSource` does not change.

**OAuth** (`neomail/src/oauth/`) is the authorization code flow for installed apps with PKCE
(S256, a verifier from the OS CSPRNG via ring) and a loopback redirect: `loopback.rs` listens on
127.0.0.1 on a port the OS picks, answers anything but the redirect with 404, ignores a wrong
`state`, bounds each request (16 KiB, 10 s) and the whole wait (5 minutes), and never echoes the
code. The code is exchanged at the token endpoint; the sign-in is refused without the mail scope
(Google's consent screen lets it be unticked) or a refresh token, and the address comes from the
ID token's `email` claim, with no signature check because it arrives straight from the token endpoint
over TLS (OpenID Connect Core 1.0, 3.1.3.7 step 6). A **provider is data** (`Provider`:
endpoints, scopes, IMAP host), so Microsoft becomes a second value, not a trait. **HTTP** is one
seam, `oauth::http::Http` (a form POST): `Https` writes HTTP/1.1 over the same rustls, ring and
platform verifier as IMAP, so no HTTP client crate is needed, and the tests give a scripted fake.
**The browser** is opened by `browser.rs` (`$BROWSER`, else `wslview` or `explorer.exe` on WSL,
else `xdg-open`, never with `--no-browser`); the URL is always printed, and choosing the command
is a pure function, so no test launches anything.

**Where OAuth secrets live:** the client secret and refresh token are one OS keyring entry, JSON,
with the service `neomail-oauth` and the account name as the user, so it can never collide with a
password entry (`neomail`). Access tokens are never stored: `neomail sync` reads the entry before it
enters `block_on`, then refreshes the access token and logs in inside it. An `invalid_grant`
says the sign-in expired or was revoked (7 days while the app is in Testing) and to run
`neomail account login NAME`. Every type holding a token or secret (`Secret`, `OAuthEntry`, `Pkce`,
`Login`, the HTTP `Response`) has a redacted `Debug`, and errors never quote a response body.

**The config writer** (`config::append_account`, used by `neomail account add`) appends one
`[[accounts]]` table to the file as text, so existing text and comments stay. It refuses a taken
name, parses the result before writing, and writes atomically (a temporary file in the same
directory, then a rename). A new file gets mode 0600 in a directory created with mode 0700; an
existing one keeps its mode.

**Setup** (`neomail/src/setup.rs`, DEC-2026-10-12-cli-ux): a password **provider is data**, a
`Preset` (`IMAP`, `PROTON`, `GMAIL_APP_PASSWORD`: fixed host, port and security, prompts, an
intro, and whether to pin the certificate), and one flow serves them all. `gather` takes the flags
and asks for the rest only when `Input::can_prompt` (standard input is a terminal and neither
`--no-input` nor `NEOMAIL_NO_INPUT` is set); otherwise a missing value is a `UsageError` (exit 2)
naming the flags. `execute` then runs, in order: `check_append`, before any network or keyring
step; for Proton, `nm_imap::server_certificate` reads Bridge's certificate over a connection that
accepts any certificate and sends nothing, shows its SHA-256 and pins it only when it equals
`--cert-sha256` or the user confirms; the secret (the keyring is opened before it is asked for,
or the `--password-command` is run); `nm_imap::check_login`, a read-only login that opens INBOX
with `EXAMINE` and logs out; and only then the keyring write and `append_account`. A failed step
writes nothing and stores nothing. The steps that leave the process are fields of `Deps`, plain
function values, so the tests give a mock keyring and fake certificate and login steps; the
login check itself is tested on the wire against the scripted server (`nm-imap/src/wire_tests.rs`).
Gmail with OAuth keeps its own flow (`account.rs`); `main.rs` picks the provider, from a numbered
menu on a terminal. No new trait: the presets are values, `Deps` has one real and one fake set,
and the seams below it (`CredentialStore`, `Transport`, the config writer) already exist.

**Getting started** (`main.rs`): bare `neomail` without a config file prints the start lines and
the README's "Try it" URL on standard error and exits 2, never prompting; with a config it prints
help. `list` and `show` check only whether the config file exists (`config::exists`) to tell "no
accounts yet; add one with `neomail account add`" from "run `neomail sync`". `account list` and
`config check` read only the config; `config check` prints a syntax error as `PATH:LINE:COL:
message` from `config::SyntaxError`, never quoting the file. `sync` shows a per-account counter
(`output::ProgressLine`, fed by `nm_core::sync_inbox_with_progress` after each batch) on standard
error only when standard error is a terminal.

Errors never quote a password, a token, the command's output or the command, and keyring errors are
rewritten so the stored bytes keyring-core carries in some of them are never printed. Nor do
config errors quote the file: a syntax error gives its message, line and column, never the
offending line, which could hold a pasted secret.

**The cache** (`nm-core/src/cache.rs`) is one SQLite file, `cache.sqlite3`, in the data
directory (created with mode 0700). On Unix, `neomail` makes the file and SQLite's `-wal` and
`-shm` files mode 0600 each time it opens the cache, even in a directory that already existed
with looser modes (`neomail/src/main.rs`, `restrict_cache_files`). Tables:

- `mailbox (account, name, uid_validity, highest_uid)`: the sync state;
- `message (mailbox_id, uid, flags, received, date, from_*, subject, raw)`: the message source
  plus the few fields `list` needs, so listing never parses mail.

`PRAGMA user_version` holds the schema version. A new cache gets its schema and version in one
`BEGIN IMMEDIATE` transaction that reads the version again, so two first syncs at once or a
crash cannot leave half a schema. A newer version than the binary knows is refused rather than
guessed at. `neomail list` and `show` read only the cache and work offline.

**TLS** (`nm-imap/src/tls.rs`): rustls with the *ring* provider. Without a pin the certificate
is checked by `rustls-platform-verifier` (the system trust store). With `cert_sha256` the server
must present exactly that certificate, and the handshake signature is still verified against it,
so the server must hold the matching private key. There is no option to skip verification. Every
network step has a 60-second timeout: connecting, the handshake, each command's reply and each
response of a FETCH. A FETCH as a whole is not bounded, so a slow transfer of large mail is not cut
off.

## How sync works

`sync_inbox` (`nm-core/src/sync.rs`) runs once per account:

1. `EXAMINE INBOX` (read-only). If UIDVALIDITY differs from the cached one, the mailbox's cache
   is dropped and the sync starts as a first sync; the summary says so.
2. List the server's UIDs and flags:
   - first sync (no `highest_uid` yet): every message (`FETCH 1:* (UID FLAGS INTERNALDATE)`, no
     bodies), keeping the `--limit` with the latest INTERNALDATE. Not the highest UIDs: Proton
     Bridge numbers imported mail newest first;
   - later: everything from the oldest cached UID up (`UID FETCH u:* (UID FLAGS)`), or from just
     above the highest UID seen when nothing is cached any more.
3. Compare with the cache: changed flags are updated, and cached UIDs missing from the listing
   were expunged and are dropped. A first sync that runs again keeps the messages it cached
   before, even when newer mail has pushed them out of the `--limit` it chose.
4. Fetch the chosen messages (first sync) or every UID above `highest_uid` (later syncs, no cap,
   so no new mail is skipped) with `UID FETCH … (UID FLAGS INTERNALDATE BODY.PEEK[])`, 50 at a
   time, oldest UID first. `BODY.PEEK` leaves `\Seen` alone. Each batch is stored in one
   transaction. On a later sync it raises `highest_uid`, so an interrupted sync resumes where it
   stopped. A first sync sets `highest_uid` to the highest UID in its listing only when it
   completes; an interrupted one runs again as a first sync and skips what it already stored.
   A UID the server answers without a body is not stored, and `highest_uid` stays below it (a
   first sync sets none), so the next sync asks for it again; UIDs above it that were stored are
   not fetched twice.
5. Print `name: N new, N updated, N removed`, followed by `, N without a body (retried next
   sync)` when some came back without one.

A second sync with no changes on the server fetches no message bodies and prints `0 new, 0 updated,
0 removed`.

## Output

Two choices, each made once in `main.rs` and passed down, decide how anything is printed:

- **The mode** (`output::Mode`): human on a terminal, plain TSV off one or with `--plain`, or
  `--json` or `--jsonl`. `output::write_listing` turns one list of serialisable items into any of
  the four, so `list` and `account list` share the rule. JSON writers take no `Paint`, so they
  cannot colour.
- **The colour** (`style::decide`, a pure function of `--color`, NO_COLOR, CLICOLOR_FORCE,
  CLICOLOR, TERM and whether the stream is a terminal), decided separately for standard output
  and standard error and handed down as `style::Colors`. Plain output is coloured only when
  colour is forced, so `--plain` on a terminal stays plain. clap's help and usage errors are
  rendered with the same decision.

Results go to standard output; prompts, progress, notes and errors go to standard error. Every
writer returns `io::Result`, so a closed pipe reaches `main` as `BrokenPipe` and exits 0 quietly;
completions are generated into memory first, because clap_complete panics on a failed write.
`show`'s human output on a terminal goes through the pager (`pager.rs`: `$NEOMAIL_PAGER`,
`$PAGER`, then `less` with LESS=FRX), falling back to printing directly when the pager cannot
start. While the pager runs, neomail holds Ctrl-C off (signal-hook, Unix only) and waits for it,
as git does; it dies of SIGINT afterwards only if the pager did. These are plain functions over `Write` and environment lookups, not traits: there is one
implementation of each, and the tests drive them with in-memory writers and fake environments.

`list` prints one line per message within the terminal's width (80 when unknown), measured per
grapheme cluster (unicode-segmentation and unicode-width), so emoji sequences count as the two
columns a terminal gives them. `show` prints the headers, then the text/plain part, or the HTML part converted to
text (html2text), then the attachments. HTML over 1 MiB, or whose parsed tree nests more than
100 elements deep, is only stripped of its tags, with a note saying so, because html5ever's and
html2text's work grows faster than linearly with nesting. The depth is measured on html5ever's
tree while it is parsed in 16 KiB chunks, not estimated from the tags, since the parser ignores
stray end tags and reopens formatting elements. Control
characters and bidirectional overrides in mail are neutralised before they reach the terminal.
`--json` shapes and exit codes are in [json.md](json.md) and only grow by adding fields.

## CI and edge builds

`.github/workflows/ci.yml` runs `cargo fmt --check`, clippy with `-D warnings` and the tests on
Linux and macOS for every pull request and push to `main`, and `cargo check` on Rust 1.88, the
`rust-version` in `Cargo.toml`. Every pull request and push to `main` then builds and packages
`neomail` for x86_64 Linux (musl, statically linked, so it does not depend on the system's glibc)
and Apple-silicon macOS, so a dependency that breaks the musl build fails before it merges.
Linux comes first ([DEC-2026-10-12-linux-first](decisions.md#dec-2026-10-12-linux-first)): the
macOS check and build may fail without failing the run, and the macOS build is published even
when the macOS check failed. A failed macOS build leaves edge without a macOS tarball until the
next one succeeds; the release notes say so. Only a push to `main` publishes the tarballs, as
the rolling `edge` pre-release
([DEC-2026-10-11-main-usable](decisions.md#dec-2026-10-11-main-usable)). Each tarball holds a
`THIRD-PARTY-LICENSES` file that cargo-about generates from the dependency tree (`about.toml`
lists the accepted licences; a crate under any other fails the build). cargo-about sees only
crates, so each tarball also holds the toolchain's `COPYRIGHT-library.html`, the Rust standard
library's notices (several licences, Unicode-3.0 among them), which `about.hbs` points to, and the
Linux build appends the notice of musl 1.2.5, the musl that Rust ships for the target and links statically
(`licenses/musl-1.2.5-COPYRIGHT`). Once the release exists,
publishing uploads the assets, then updates the notes, which name the commit, and moves the
`edge` tag last, so the tag never runs ahead of the uploaded assets. A run cancelled during the
upload can leave a tarball missing until the next publishing run, because `--clobber` deletes the
old asset before it uploads the new one. The very first publish creates the release as a draft,
uploads, then publishes it, which creates the tag; a cancelled first publish leaves a draft
without a tag, which the next run uploads to and publishes. Only the publishing job
can write to the repository, and a newer push cancels an older run.

The assets are `neomail-<target>.tar.gz`, each holding `neomail-<target>/neomail` next to the
README and licences, and `SHA256SUMS` (`sha256sum` format, one line per tarball), which
publishing writes into `dist/` so it is uploaded with the tarballs and kept by the loop that
drops assets the build did not produce
([DEC-2026-10-12-install-channels](decisions.md#dec-2026-10-12-install-channels)).
`install.sh`, at the repository root, picks the target from `uname` (x86_64 Linux and WSL take
the musl build; Apple silicon, or Rosetta on it, the macOS one), downloads the tarball and
`SHA256SUMS` from edge, verifies the checksum, and installs `neomail` atomically into
`~/.local/bin` or the directory the user chose; it refuses to replace a binary there that is not
neomail. All its code sits in `main`, called on the last line, so a truncated download runs
nothing. `NEOMAIL_INSTALL_BASE_URL` points it at a mirror or a local server for tests. CI runs
shellcheck on it for every pull request and push, and after each publish runs it against the new
edge in Ubuntu, Alpine, Fedora and Arch containers, plus a check that half the script installs
nothing. mise reads the same release through its GitHub backend.

## What comes next

The order of the slices is in [roadmap.md](roadmap.md). What they change in the code:

- **More providers:** Microsoft Graph and the Gmail API become new `MailSource` crates; Graph's
  delta sync may widen the trait (a sync token next to UIDs).
- **More mail:** folders beyond INBOX, CONDSTORE/QRESYNC to refresh flags cheaply, and IDLE.
- **Actions:** flags, moves and deletes need a write side on the source and a way to record
  pending changes in the cache.
- **The TUI** builds on the same cache and actions as the CLI.
