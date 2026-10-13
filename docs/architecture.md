# Architecture

This page follows the code: it describes what exists, not what is planned. Product decisions live
in [decisions.md](decisions.md). The slices so far follow
[DEC-2026-10-11-build-first](decisions.md#dec-2026-10-11-build-first): a read-only INBOX in the CLI
(slice 1), IMAP passwords in the OS keyring (slice 2), Gmail through the user's own OAuth client
(slice 3), getting started and setup (slice 4), output for people and pipes (slice 5), Outlook
through Microsoft Graph with the user's own Entra app (slice 6) and background sync (slice 7):
`neomail daemon`, with desktop notifications, IMAP IDLE push and a one-minute Outlook check through
Graph's delta query. Triage in the CLI (slice 8) has begun: `neomail mark read|unread`, the first
actions, then `neomail archive` and `neomail delete` for IMAP and Outlook accounts, then
`neomail folder list` and `neomail move` for IMAP and Outlook accounts.

## Crates

| Crate | What it holds | Why it is a crate |
| ----- | ------------- | ----------------- |
| `crates/neomail` | The binary: CLI (clap), config, passwords (command or OS keyring), OAuth sign-in, the daemon and its locks, output, wiring. | The composition root. Nothing depends on it. |
| `crates/nm-core` | Domain types, the `MailSource`, `IdSource` and `MailWriter` traits, the SQLite cache, the sync engines, the actions, mail parsing, and building outgoing mail (`compose`). | The seam: providers and front ends depend on it, and it never touches the network, so its tests run offline. |
| `crates/nm-imap` | `ImapSource`: TCP, TLS or STARTTLS (rustls), certificate pinning, IMAP (async-imap); its two writes, `store_seen` and `move_to`, and `list_folders`, with modified UTF-7 names decoded; `append_sent`, and SMTP submission (`smtp`) over the same TLS and pinning. | The first `MailSource`. It keeps the TLS and IMAP dependencies away from the core; `nm-graph` is its sibling. |
| `crates/nm-graph` | `GraphSource`: the INBOX through Microsoft Graph's REST API, over a `Transport` port; its two writes, `set_read` and `move_to`, and `folders`, the read behind `folder list`; `send`, through sendMail. | The first `IdSource`. It holds Graph's URLs, JSON and error rules and no network code, so its tests run offline. |
| `crates/nm-redact` | The PII redaction rules, how each placeholder is spelled, `Excerpt`, the text classification sends, and `Outgoing`, that text with the signals that go with it. Pure: no I/O, no network, no config; `regex` is its only dependency. | One owner for what may leave the machine: the classifier's request (slice 10) takes an `Outgoing` and nothing else, and the rules change without touching it. What MCP clients receive is decided when MCP lands, through the same rules. |

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

**`IdSource`** (`nm-core/src/id_source.rs`) is the same read-only view for providers whose
messages have opaque, case-sensitive string ids and no UIDs or UIDVALIDITY (Microsoft Graph,
slice 6): `newest(limit)` and `arrived_since(time)` list ids with their flags and required arrival
time, and `fetch(ids)` returns RFC 822 sources without marking them read. It sits beside
`MailSource` rather than widening it, because no UID-based rule (watermark, UIDVALIDITY reset)
carries over; the cache's `MailboxKind` (`Imap` or `ById`) is the only other place the two meet.
`nm_core::conformance::id_source`, behind the `conformance` feature, checks a source against the
contract, including that `newest` returns the latest arrivals; it needs two messages of different
arrival times and fails rather than pass vacuously. The in-memory reference source in
`nm-core/tests/id_sync.rs` passes it; sources that ignore `since`, list the oldest or list nothing
fail it. No other trait is needed: the cache stays a concrete type with by-id
methods, and the engine reuses `SyncReport`, `SyncError` and `Progress`.

**The cursor seam** is two more `IdSource` methods, for providers that report changes since a
point: `start_changes(from)` starts tracking and returns a whole round (at least every message
there now that arrived at or after `from`) with a `Cursor`, and `changes_since(cursor)` returns
the whole round since it, or `None` when the provider refuses the cursor. A round is a list of
`IdChange::Present { id, flags?, received? }` and `IdChange::Removed(id)`; fields a provider leaves
out of an update stay as cached. Both default to `Ok(None)`, so a source without change tracking
(the reference source, the conformance fakes) is unchanged and the engine lists as before. Graph's
delta link is the cursor. They are methods with
defaults rather than a second trait, because the engine is one engine either way: a separate
`ChangeSource` bound would need a second entry point or specialisation to pick it, and every
source that implements it also implements `IdSource`. `Cursor`'s `Debug` is redacted, so it is
never printed.

**`Transport`** (`nm-graph/src/lib.rs`) is the port `GraphSource` reads and writes Graph through:
one HTTPS GET, a PATCH or POST with a JSON body, or a POST with a text body, with request headers and a body cap, answered
with the status, `Retry-After` and the body.
`neomail/src/graph.rs` implements it with neomail's HTTP client, and `nm-graph/tests/graph.rs` with
a fake Graph: a mailbox model that pages with next page links, plus scripted replies. That fake
passes `nm_core::conformance::id_source`. `GraphSource` sends `Authorization: Bearer`, `Prefer:
IdType="ImmutableId"` (it applies only to the request that carries it) and, for JSON, `Accept:
application/json` on every request. `newest` and `arrived_since` list
`/me/mailFolders/inbox/messages` (`$select=id,isRead,receivedDateTime`, `$orderby=receivedDateTime
desc`, `$top` up to 1000, and for `arrived_since` a `$filter=receivedDateTime ge …` that names the
ordered property first, as Graph requires), and follow `@odata.nextLink` verbatim, but only to
`https://graph.microsoft.com/`, so the token never leaves Graph. `fetch` downloads
`/me/messages/{id}/$value` (the id percent-encoded), four at once (Outlook's limit per app and
mailbox), and leaves out a 404. `isRead` maps to `\Seen`; an item without `receivedDateTime` is
skipped. 429 and 503 wait `Retry-After` (at most 5 minutes), else 1, 2, 4 and 8 s, for at most five
tries; 401 and 403 become "access token refused" and "Microsoft Graph refused access to the
mailbox"; any other status gives Graph's `error.code`, cleaned, never the body or the token. JSON is
capped at 8 MiB, a message at 160 MiB.

The one write, `set_read`, sends `PATCH /me/messages/{id}` with `{"isRead": …}` through the same
`Transport`, which has a `patch_json` beside `get` rather than a second client. It sends the same
headers, at most four at once, and retries throttling as reads do. A 404 is gone; the flags come
from the `isRead` of Graph's answer, which is the whole message, so it is capped at 160 MiB.

`start_changes` and `changes_since` run Graph's delta query
([message: delta](https://learn.microsoft.com/en-us/graph/api/message-delta)). A round's first
request is `/me/mailFolders/inbox/messages/delta?$filter=receivedDateTime ge …&$select=…`: query
options go only there, and Graph encodes them into the next and delta links; `receivedDateTime ge`
is the only filter delta supports
([delta query for messages](https://learn.microsoft.com/en-us/graph/delta-query-messages)). The
round follows `@odata.nextLink` to `@odata.deltaLink`, both only on Graph, with the same headers,
and returns nothing until the delta link is in, so a stopped round leaves no half-round link. An
entry with `@removed` is a removal; others are present, with `isRead` (a boolean, or the string
Graph's own example shows) and `receivedDateTime` when sent. `410 Gone` and `syncStateNotFound`
return `None` ([delta query overview](https://learn.microsoft.com/en-us/graph/delta-query-overview)),
as does a stored link off Graph, which is never sent. A round from a stored link also returns
`None` on 400: the live check found Graph answers a link it cannot use with 400 `BadRequest`. A
400 on a new round's first request stays an error, so a bad filter cannot loop. Microsoft's page
says a filtered delta query can report read changes for messages outside the filter
([message: delta](https://learn.microsoft.com/en-us/graph/api/message-delta)); the live check
found it did not, so the round starts at the window's start and covers every cached message. With
`$filter` a round holds at most 5,000 messages
([delta query for messages](https://learn.microsoft.com/en-us/graph/delta-query-messages)), so
`start_changes` stops at the page that reaches it and returns `None`, and the engine keeps
listing. A stored link carries the same filter, so `changes_since` stops there too and the round
starts over. `IdSource::START_LIMIT` says so up front: a listing that alone holds 5,000 messages
asks for no round, so such a window costs a listing per sync, not a listing and a round. Known
limit: mail moved into the inbox with a `receivedDateTime` older than the window is never seen:
the round's filter and the listing both leave it out.

Graph needs no other trait: `IdSource` is the source seam, `Transport` the network one, and the
sign-in reuses the `Provider` data, the `Http` seam and the keyring entry of slice 3.

**The password** comes from one of two sources, fixed per account when the config is parsed
(`PasswordSource` in `neomail/src/config.rs`): the account's `password_command`, run with `sh -c`,
or, without one, the OS keyring entry with service `neomail` and the account name as its
`username` attribute. An account with a command never opens the keyring. `secret::account_password`
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
`WSL_DISTRO_NAME` is set or `/proc/sys/fs/binfmt_misc/WSLInterop` exists. The classifier's key has
an entry of its own, service `neomail-jev` with the backend's host as the user, so it never meets
a mail account named "jev"; `keyring::jev_key` takes that entry, else `JEV_API_KEY`, and a keyring
that failed to open or to answer counts as having no key, so the variable still works without a
Secret Service.

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
over TLS (OpenID Connect Core 1.0, 3.1.3.7 step 6), falling back to `preferred_username`, which
Microsoft sends when `email` is missing. A **provider is data** (`Provider` in `oauth/provider.rs`:
endpoints, an optional revocation and device code endpoint, `ClientKind` (Google's confidential
client sends its secret, Microsoft's public client never does), the redirect host, the scopes, the
accepted mail grants and their prefix, the refresh scope, `MailApi` (IMAP host and port, or Graph),
numeric error-code hints and the setup text), so `MICROSOFT` is a second value, not a trait. A missing `scope` in a token answer
counts as the scopes requested; a granted scope matches a mail grant after the prefix is stripped,
ignoring case. **Device code** (`oauth/device.rs`, RFC 8628) asks for a code, shows the provider's
`message` (else the URL and code), and polls the token endpoint every `interval` seconds (default
5), adding 5 s for good on `slow_down`, stopping on a declined, expired or unknown code or after
`expires_in` (default 900 s); its tokens pass the same checks as the code exchange. Google has no
device code endpoint here: its device flow does not allow Gmail's scope, so `--device-code` on a
Google account is a usage error that says so. Device code is also chosen on its own over SSH
without a display (`browser::ssh_without_display`, a pure function of the environment). For a
`localhost` redirect (Microsoft) the loopback listener also binds `[::1]` on the same port, best
effort, and accepts on both within the same connection cap. **HTTP** (`neomail/src/http.rs`):
`Https` writes HTTP/1.1 over the same rustls, ring and platform verifier as IMAP, so no HTTP client
crate is needed: a form POST for OAuth (the `Http` seam, which the OAuth tests replace with a
scripted fake), a GET with request headers for Graph and a JSON POST for the classifier, each
with a body cap, reading `Retry-After` (and `retry-after-ms`, which wins), one connection per
request, any port, and plain `http://` only to this machine (`127.0.0.1`, `[::1]`, `localhost`:
a local classifier, and the CLI tests' fake one), and a 60-second bound on each network step rather than
the whole response.
**The browser** is opened by `browser.rs` (`$BROWSER`, else `wslview` or `explorer.exe` on WSL,
else `xdg-open`, never with `--no-browser`); the URL is always printed, and choosing the command
is a pure function, so no test launches anything.

**Where OAuth secrets live:** the client secret (omitted for a public client) and refresh token
are one OS keyring entry, JSON, with the service `neomail-oauth` and the account name as its
`username` attribute, so it can never collide with a password entry (`neomail`). Access tokens are
never stored on disk: the sync reads the entry before it enters `block_on`, refreshes the access
token inside it, and leaves it again to write back a **rotated refresh token** (Microsoft sends a
new one with each refresh; a failed write is a warning, since the old token is not revoked), then
syncs. An `invalid_grant` gives the provider's reason (7 days while a Google app is in Testing; 90
days unused, a password change or revoked access at Microsoft) and says to run
`neomail account login NAME`. Logout skips revocation for a provider without a revocation endpoint
(Microsoft) and says the sign-in was only deleted from the keyring.

The provider work touches three seams and adds none: the `Provider` data, the `Http` form POST
(device code is form POSTs too, so the scripted fake and tokio's paused time test it), and the
keyring entry's JSON, whose `client_secret` became optional so Gmail entries round-trip unchanged.

Every type holding a token or secret (`Secret`, `OAuthEntry`, `Pkce`, `Login`, the HTTP
`Response`) has a redacted `Debug`, and errors never quote a response body.

**The config writer** (`config::append_account`, used by `neomail account add`) appends one
`[[accounts]]` table to the file as text, so existing text and comments stay. It refuses a taken
name, parses the result before writing, and writes atomically (a temporary file in the same
directory, then a rename). A new file gets mode 0600 in a directory created with mode 0700; an
existing one keeps its mode.

**Setup** (`neomail/src/setup.rs`, DEC-2026-10-12-cli-ux): a password **provider is data**, a
`Preset` (`IMAP`, `PROTON`, `GMAIL_APP_PASSWORD`: fixed host, port and security, prompts, an intro,
and whether to pin the certificate), and one flow serves them all. `gather` takes the flags and asks
for the rest only when `Input::can_prompt` (standard input is a terminal and neither `--no-input`
nor `NEOMAIL_NO_INPUT` is set); otherwise a missing value is a `UsageError` (exit 2) naming the
flags. `execute` then runs, in order: `check_append`, before any network or keyring step; for
Proton, `nm_imap::server_certificate` reads Bridge's certificate over a connection that accepts any
certificate and sends nothing, shows its SHA-256 and pins it only when it equals `--cert-sha256` or
the user confirms; the secret (the keyring is opened before it is asked for, or the
`--password-command` is run); `nm_imap::check_login`, a read-only login that opens INBOX with
`EXAMINE` and logs out; and only then the keyring write and `append_account`. A step that fails
before the keyring write writes nothing and stores nothing; if the final `append_account` fails, the
password stays in the keyring and the error says so. The steps that leave the process are fields of
`Deps`, plain function values, so the tests give a mock keyring and fake certificate and login
steps; the login check itself is tested on the wire against the scripted server
(`nm-imap/src/wire_tests.rs`). Gmail and Outlook with OAuth keep their own flow (`account.rs`); `main.rs` picks
the provider, from a numbered menu on a terminal. No new trait: the presets are values, `Deps` has
one real and one fake set, and the seams below it (`CredentialStore`, `Transport`, the config
writer) already exist.

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

- `mailbox (account, name, uid_validity, highest_uid, seen_until, synced, cursor)`: the sync
  state. A NULL `uid_validity` marks a by-id mailbox, whose state is `seen_until` and `cursor`
  instead. `synced` is 0 from the mailbox's creation until a sync of it completes. `cursor` is the
  source's change cursor (Graph's delta link), NULL until a sync stores one;
- `message (mailbox_id, uid, remote_id, flags, received, date, from_*, subject, raw, msg_key)`:
  the message source plus the few fields `list` needs, so listing never parses mail. Exactly one
  of `uid` and `remote_id` is set (a `CHECK`), each unique per mailbox. `msg_key`
  (`parse::msg_key`) is the Message-ID, else `sha256:` and the hex digest of `raw`: filled on
  insert, NULL for a row cached before schema 5 until `classify` reaches it;
- `classification (account, msg_key, choice, confidence, probabilities, model, request_version,
  input_tokens, classified_at)`: one classifier answer per message, as given. It is keyed by
  account and `msg_key`, not by `message.id`, because rebuilding a mailbox deletes its rows and
  gives them new ids, and an answer must outlive that so no message is paid for twice.

The group a message lists under is worked out when the cache is read, in one place:
`GroupRules::effective` (`nm-core/src/group.rs`), which `Cache::list_grouped` applies to each row
before its `GroupFilter`. `spam` stays spam whatever the confidence; below `other_below` a choice
lists as `other`; a `follow-up` older than `follow_up_old_days` lists as `follow-up-old`. Changing
a threshold needs no new answer. `Cache::pending` lists the messages with no stored answer, newest
first, for `classify` to choose from.

Schema 2 added the by-id columns, schema 3 `mailbox.synced`, schema 4 `mailbox.cursor` and schema
5 `message.msg_key` and the `classification` table. The step to 5 parses no row. Each
step from 2 runs in one `BEGIN IMMEDIATE` transaction that reads the version again. A schema 2 cache
gets `synced` with every existing mailbox counted as synced (a build without the column synced it);
a schema 3 cache gets `cursor` empty, so its next sync lists and stores one. A schema 1 cache is
migrated when it is opened, following SQLite's procedure for other table changes: foreign keys off outside any transaction
(inside one the pragma does nothing, and with them on, dropping the old `mailbox` would delete every
message), then one `BEGIN IMMEDIATE` transaction that reads the version again, creates the new
tables, copies every row with its id, drops the old tables, renames the new ones, restores the
`message` id counter (copying would otherwise lower it to the highest id left, and a deleted newest
id would be handed out again), recreates the index, requires an empty `PRAGMA foreign_key_check` and
sets the version, straight to the newest schema with every mailbox synced. Foreign keys go back on
afterwards. Any failure rolls back to the untouched schema 1 (`nm-core/tests/migration.rs`).

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

**Sending** is built in three places, so each rule has one owner:

- `nm_core::compose` turns a `Draft` (from, to, cc, bcc, subject, Markdown) into an `Outgoing`:
  the RFC 5322 bytes (mail-builder), the envelope, the Bcc list and the Message-ID. The body is
  multipart/alternative, the Markdown as text/plain and pulldown-cmark's HTML, with raw HTML
  escaped. Bcc is in the envelope only. `reply` and `forward` build a draft from a cached
  original (`ParsedMessage`, with its `Threading` headers): recipients, `Re:`/`Fwd:`, In-Reply-To
  and References as RFC 5322 section 3.6.4 says, and the quoted text. It is pure, and the TUI and
  agents will reuse it.
- `nm_imap::smtp::submit` sends an `Outgoing` over implicit TLS or STARTTLS (refused when the
  server does not offer it, and data sent ahead of the handshake ends the session), with AUTH
  PLAIN or XOAUTH2. One refused recipient ends the transaction with RSET before DATA, so nobody
  gets the message. `nm_imap::append_sent` stores a copy in the mailbox marked `\Sent`, `\Seen`,
  and never creates one. `nm_graph::GraphSource::send` posts the MIME to sendMail; Graph reads
  recipients from the headers, so it refuses a message with Bcc rather than drop them.
- `config::Account` holds the identity (`address`, `display_name`) and `outbound()`: SMTP from
  `[accounts.smtp]` or a preset (Gmail, Bridge), or Graph. Gmail, Bridge and Graph keep the Sent
  copy themselves; only other servers get `append_sent` (`save_sent`). Microsoft sign-ins ask
  for Mail.Send, and `MailGrant::ReadWriteSend` records it; an older sign-in is told to run
  `account login` before the question, from the grant in its keyring entry, and `graph::send`
  checks it again before any request.
- `neomail/src/send.rs` is the commands `send`, `reply` and `forward`: it picks the account
  (`--from`, else the original's, else the only one), reads the Markdown, builds the draft and
  checks from the config that the account can send it (an SMTP server, no Bcc through Graph).
  Then it either prints it (`--dry-run`, with the summary on standard error, before any keyring
  or connection) or checks an Outlook grant, asks (or takes `--yes`; without either, a usage
  error), takes the login
  through `sync::Logins` as `mark` does, and sends. The copy in Sent comes after the send and
  only warns when it fails. It owns the output: one line, the Message-ID, or the JSON object.

The tests run a scripted SMTP server over a pipe and a TLS one with an rcgen certificate on
127.0.0.1 (`nm-imap/src/smtp/tests.rs`), an APPEND server (`wire_tests.rs`) and the fake Graph.
`neomail/tests/send.rs` drives the binary against a TLS SMTP server on 127.0.0.1 that records
each command and message, and a listener that stands for an IMAP server whose append fails. The
binary cannot reach a fake Graph (its URL is fixed), so Outlook's sending is tested in nm-graph
and `graph.rs`, and the binary's tests stop an Outlook message before any request.

**The action registry** (`nm-core/src/action.rs`) is how mail is changed. It is the typed set of
actions that the TUI's keys, the CLI, scripts and agents share (the first bullet of
[CLI, scripting, agents, extensibility](decisions.md#cli-scripting-agents-extensibility)). It
starts small: `Action` is `MarkRead`, `MarkUnread`, `Archive`, `Delete` or `Move(FolderName)`,
each with a stable name (`mark-read`, `mark-unread`, `archive`, `delete`, `move`).
`Action::change` says what it does: `Change::Seen(bool)` or `Change::Move(Destination)`. A
`Destination` is `Role(Folder)`, a role (`Archive` or `Trash`), or `Name(FolderName)`, the name
the user typed; it stays that until the account's server resolves it to one of its folders.
`action::apply(action, ids, cache, writer)` is the one way to apply an action to cache ids;
`neomail mark`, `archive`, `delete` and `move` call it. For each account of the ids, in the order first asked, it:

1. takes the account's lock (`MailWriter::lock`);
2. reads the ids' locations again under the lock;
3. writes to the server;
4. updates the cache with what the server confirmed, then releases the lock.

An unknown id, a message gone from the server, a change the server did not confirm, and an
account that cannot be locked or written are each a `Failure` in the report. One account's
failure never stops the others, and nothing is queued. There is no Lua, string lookup or plugin
machinery yet.

**`MailWriter`** is the write port that `apply` drives. `Cache::locate` returns each message's
`MessageKey`: `Uid { uid_validity, uid }` or `Remote(RemoteId)`. `apply` hands UIDs, with their
UIDVALIDITY, to `set_seen_by_uid` and Graph ids to `set_seen_by_id`, so a UID never reaches Graph
and a Graph id never reaches IMAP. A writer answers with `Written { done, gone }`. Flags in `done`
are the server's, and replace the cached ones. A confirmation without flags sets or clears
`\Seen` in the cached flags. A key in neither list is not confirmed, and its row is left alone.
An `Err` confirms none of the call's messages: the server may still have changed some of them
(Graph writes one message per request), so the cache is left alone for all of them and the next
sync reads their state.

A move goes through `move_by_uid` and `move_by_id` the same way, with the `Destination`, and the
writer answers with `Moved { folder, moved, gone }`: `folder` is the destination as the user sees
it (an IMAP name decoded), which the report carries to the output. A `Destination::Name` the
account has no folder for, or one naming INBOX or a folder that cannot hold messages, is an `Err`:
nothing of that account moves. A message in `moved` leaves the cache
(`Cache::remove`), as when a sync finds it gone; its id is never handed out again
(AUTOINCREMENT). A gone one stays for the next sync, as for a mark, and a key in neither list is
not confirmed. A done message's `State` is its flags after a mark, or the folder after a move.

neomail's writer (`neomail/src/triage.rs`) does three things:

- it takes the same per-account `flock` as `sync` (`neomail/src/lock.rs`), printing once that it
  waits for the daemon;
- it logs in through `sync::Logins`, which owns the logins of `sync`, `daemon` and the actions;
- it reaches IMAP through `nm_imap::store_seen` and `nm_imap::move_to`.

`store_seen` selects INBOX read-write (every other path keeps `EXAMINE`) and refuses to store when
UIDVALIDITY changed. It sends `UID STORE +FLAGS (\Seen)` or `-FLAGS (\Seen)` (RFC 3501, section
6.4.6) and takes each UID's flags from the server's FETCH responses. A UID that got no FETCH
response is fetched with `UID FETCH (UID FLAGS)`, and a UID in neither is gone. The session ends
with LOGOUT, never CLOSE, which expunges (RFC 3501, section 6.4.2). The wire tests check that no
`EXPUNGE`, `CLOSE`, `MOVE`, `COPY` or `\Deleted` is ever sent.

`move_to` checks the MOVE capability ([RFC 6851](https://www.rfc-editor.org/rfc/rfc6851.html)),
then finds the destination with `LIST "" *`. By role, from the special-use attributes
([RFC 6154, section 2](https://www.rfc-editor.org/rfc/rfc6154.html#section-2)): the archive is
the first selectable mailbox with `\Archive`, else the one with `\All` (Gmail's All Mail); the
trash is the one with `\Trash`. By name, the one mailbox whose decoded name is exactly the
`FolderName`; INBOX in any case, a `\Noselect` or `\NonExistent` mailbox, a missing name and a
name two mailboxes share are errors (`ImapError::IntoInbox`, `NotSelectable`, `NoSuchFolder`,
`SharedName`), before INBOX is selected. Servers send these on a plain LIST without the SPECIAL-USE
capability: Proton Mail Bridge does, and so does Gmail
([IMAP extensions](https://developers.google.com/workspace/gmail/imap/imap-extensions#special-use_extension_of_the_list_command)).
A server without MOVE, or without the folder, is an error and nothing moves; neomail never creates
a mailbox and never falls back to COPY, `\Deleted` and EXPUNGE. Then it selects INBOX read-write
after the same UIDVALIDITY check as `store_seen` and confirms the move by asking INBOX twice with
`UID FETCH (UID FLAGS)`:

1. before: a UID INBOX does not have is gone (RFC 3501, section 6.4.8, ignores it) and is not sent;
2. `UID MOVE` the rest to the destination;
3. after: a UID INBOX no longer has moved; one it still has is not confirmed. A MOVE "might fail
   partway through" (RFC 6851, section 3.3), so its tagged OK alone is not trusted per message.

A message another client expunged between the two fetches counts as moved; its row would leave
the cache at the next sync anyway. The wire tests check the commands, the destination for each
LIST, which UIDs are confirmed, and that no `EXPUNGE`, `CLOSE`, `COPY`, `CREATE`, `RENAME`,
`DELETE`, `STORE` or `\Deleted` is ever sent.

**Folders.** nm-imap keeps each LIST answer as a private `Mailbox`: `wire`, the name exactly as
the server sent it (modified UTF-7,
[RFC 3501 section 5.1.3](https://www.rfc-editor.org/rfc/rfc3501#section-5.1.3)); `name`, that
name decoded (`nm-imap/src/utf7.rs`, or `wire` when it does not decode); whether it can hold
messages; and its roles. Matching and output use `name`, and `UID MOVE` sends `wire`, so a user's
name never reaches the wire and an encoded name never leaves nm-imap. `nm_imap::list_folders` is
the read behind `neomail folder list` (`neomail/src/folders.rs`): one `LIST "" *`, then LOGOUT,
nothing selected. It returns nm-core's `ListedFolder { name: FolderName, role: Option<Role> }`,
leaving out mailboxes that cannot hold messages; INBOX gets `Role::Inbox`. Listing is a read, not
an action, so it is not in the action set and takes no account lock: it does not touch the cache.
On Outlook, `GraphSource::folders` is that read (`neomail/src/graph.rs` `folders`): the folder tree
through `GET /me/mailFolders` and `.../childFolders`, every page, named by paths of display names,
with roles from one `GET` per well-known name. A `Destination::Name` reaches
`GraphSource::move_to`, which resolves the path with `find`: it lists only the levels whose
folder names start the rest of the path, never the whole tree, and refuses a missing, shared or
search-folder path and the inbox (by its id). The moves then send the folder's id as
`destinationId`. A Graph folder id never leaves nm-graph, as an encoded mailbox name never leaves
nm-imap.

For Outlook, the writer calls `graph::set_read`, which calls `GraphSource::set_read`. Changing mail
needs Mail.ReadWrite, which new sign-ins ask for along with Mail.Read. A sign-in records what it was
granted as `oauth::MailGrant` (`Read` or `ReadWrite`) in its keyring entry (`mail_grant`). An entry
without it comes from an earlier version, which asked only for Mail.Read, and counts as `Read`. The
grant travels with the access token (`sync::AccessToken`), so two things follow from it:

- The refresh asks for no more than the sign-in granted: `Provider::refresh_scope` holds one
  scope per grant, since Microsoft's refresh scope must be the original's or a subset.
- `set_read` refuses a `Read` sign-in before any request, and says to run `neomail account login
  NAME`.

For an Outlook move, the writer's `move_by_id` calls `graph::move_to`, which refuses a `Read`
sign-in the same way and calls `GraphSource::move_to`. That resolves the role to Graph's well-known
folder name (`nm_graph::well_known`: `archive` or `deleteditems`), so the role stays a role until
the account's server side turns it into a name. It reads that folder first
(`GET /me/mailFolders/{name}?$select=id`): a 404 there is `GraphError::NoFolder` and nothing moves,
so a 404 on a move can only mean the message is gone. Then it sends `POST /me/messages/{id}/move`
with `{"destinationId": name}` through `Transport::post_json`, at most four at once and retried as
reads are: 201 is moved, 404 is gone, and anything else fails the account. Both Graph writes share
the writer's one Graph login helper (`triage::Writer::graph`), as the IMAP ones share `imap`.

The action tests (`nm-core/tests/action.rs`) drive `apply` with a fake writer.

**`Excerpt`** (`nm-redact/src/lib.rs`) is the only text that may leave the machine for
classification. `Excerpt::new(subject, body, &Known)` redacts the subject whole and the body whole,
then cuts the body to its first 1,500 characters without splitting a placeholder. Its fields are
private and nothing else makes one, so a request that takes an `Excerpt` can only carry redacted
text. `Known` is what the caller already knows and wants masked, the display names from the headers
and the owner's addresses; the caller gathers it (from the parsed message and the config) and
nm-redact decides how it is matched. A `Redacted` text keeps where each placeholder sits, so the
output styles masks without parsing placeholders back out, and `MaskCounts` has a count for every
`Kind`, zeros included. The rules, their placeholders and what they keep are in
`nm-redact/src/lib.rs`; the tests (`nm-redact/tests/`) check each rule and, over tables of inputs,
that no digit run of 4 or more (outside a link host or a date), no address's local part and no known name survives.

The caller's side is one function, `redact::excerpt` (`neomail/src/redact.rs`): it takes a
`ParsedMessage` and the owner's addresses (every account's login that is an address, from the
config), gathers the display names of From, To, Cc and Reply-To into a `Known`, and makes the
`Excerpt` from the subject and the body as `neomail show` renders it. `redact::outgoing` wraps
that `Excerpt` and the header flags into an `Outgoing`; `neomail redact` prints it
(`output::RedactItem`, `output::write_redacted`, the signals as one line or a `signals` object)
and `neomail classify` builds its request from the same call, so what the user previews is what
is sent.

**`Outgoing`** (`nm-redact/src/outgoing.rs`) is everything classification may send for one
message: its `Excerpt` and its `Signals`. `Outgoing::new(excerpt, from_address, HeaderFlags)`
keeps only the From address's domain, lower-cased and only when it is a domain name, and whether
the local part names a role (`noreply`, `orders` … on a short exact list); the local part and
display names are dropped there. `HeaderFlags` is three booleans the caller reads from
`parse::Headers` (`nm-core/src/parse.rs`): a mailing list (List-Unsubscribe or List-Id), automated
(Auto-Submitted other than `no`, or Precedence bulk, list or junk) and a reply (In-Reply-To).
`Signals` has no public constructor, so a request that takes an `Outgoing` holds redacted text and
these fields only.

**jev's wire format** (`neomail/src/jev.rs`) is pure: `request_body(model, &Outgoing)` writes
`POST {url}/v1/systemone`'s body, one choice question `group` with the ten options in a fixed
order, `main` first, written straight to the output so no map sorts them; `parse_answer` checks a
2xx answer (the choice is an option, every number a probability) and never quotes it in an error;
`verdict` sorts a status into answer, retry (408, 429, 5xx, waiting as the server says up to 30 s,
else 0.5 s doubling to 5 s with the caller's jitter), skip (422) and stop (401 and 403 with the key
hint; anything else with at most 2 KiB of the body, control characters replaced). A message that
uses its 3 tries on a retry status fails and stops the run, as an outage would cost every other
message as many requests; a worker waiting to retry gives up once another has stopped the run.
`REQUEST_VERSION` is stored with each answer and bumped whenever the question changes. Every
backend speaks this one format; the base URL, `classify.url`, is the seam.

**Classifying** (`neomail/src/classify.rs`) is the `classify` and `key` commands. `plan` picks the
messages: the ids given (one that has a result is printed, not asked again, unless `--again`), else
`Cache::pending` cut to the window (`--days`) and the cap (`--limit`), the rest counted for the
estimate. Reading a message builds its `Outgoing` and request and records its `msg_key` (a row
cached before schema 5 has none). The estimate's tokens per request are the average stored
`input_tokens`, else the requests' characters divided by 4. Consent is one host per line in
`classify-consent` in the data directory, outside the cache so a rebuild keeps it. The key is
read before the tokio runtime starts (keyring.rs says why). `send` runs 4 workers on one
current-thread runtime, each taking the next request off a shared queue, one connection per
request; the main loop stores each answer with `Cache::store_classification` as it arrives,
then prints it, so a stopped run resumes where it left off. A worker's `Stop` (401, 403, an
unexpected status, an unreachable backend) stops the others taking new requests.

Debug builds open a JSON file named by `NEOMAIL_TEST_KEYRING` instead of the OS keyring
(`keyring/test_store.rs`), so the CLI tests can store a key in one process and read it in the
next; release builds have no such path. The CLI tests (`neomail/tests/classify.rs`) run a fake
jev on 127.0.0.1; `tests/live_jev.rs` is the one live check, off by default.

## How sync works

`neomail sync` and `neomail daemon` sync an account through one function, `sync::Syncer::sync`
(`neomail/src/sync.rs`), which picks the engine from the account's server (`config::Server`, fixed
at parse: a password account is always IMAP, an OAuth account's provider decides): `Imap` runs
`sync_inbox` over `ImapSource`, `Graph` runs `sync_inbox_by_id` over `GraphSource`. An OAuth
account's access token comes from the same refresh and rotated-token write-back either way, and both
print the same line and progress. `sync::Logins`, which `neomail mark` shares, keeps each access
token in memory (`sync::Tokens`) until 5 minutes before the `expires_in` of the token answer (RFC
6749, section 5.1), for the account settings it was issued under; a token without a lifetime is used
once, and any failure drops the account's token. The times are wall-clock (`SystemTime`): Linux's
`Instant` is `CLOCK_MONOTONIC`, which stops during suspend, so a token would outlive its expiry
across a laptop's sleep; a clock set back before the token's issue drops it. `neomail sync` starts
with none, so it refreshes every run, as before.

## The daemon

`neomail daemon` (`neomail/src/daemon.rs`) runs rounds: `run_rounds` starts one at once, then
each `interval` after the previous one started, or at once when a round ran longer, without making
up missed ones (`next_round`). A round reads the config again (an invalid one is reported once and
the last good one kept), then syncs each account in turn through the same `Syncer`, keeping the
keyring and the tokens between rounds. `Reports` decides what is printed: a change, a failure that
differs from the account's last one, and the success after a failure, each after the local time.

The rounds run outside the runtime and wait inside it (`Stop::wait_until`), so keyring calls stay
outside `block_on`. SIGINT and SIGTERM (signal-hook) set a flag that the round checks between
accounts, and write a byte to a socket pair that ends the wait at once; a second signal takes the
default action, as in the pager. A signal ignored at start stays ignored. The tests drive
`run_rounds` on tokio's paused clock and a round's output through in-memory writers.

**Locks** (`neomail/src/lock.rs`) are `flock(2)` through rustix (std's `File::lock` needs Rust
1.89): the daemon holds `DATA_DIR/daemon.lock` for its whole life, so a second daemon exits 1;
`neomail sync` and the daemon hold `DATA_DIR/locks/ACCOUNT.lock` while they sync that account
(the name with bytes other than letters, digits, `_`, `-` and a non-leading `.` as `%XX`). `sync`
waits for it and says so once; the daemon skips the account for that round. A lock dies with its
process, so a crash leaves nothing to clean up, and lock files are never deleted. `round` and
`on_wake` take the lock as a closure (`lock::try_account` in `run`), so the tests sync without a
real one: a process that a concurrent test forks can keep a lock held until its exec.

**Where push plugs in:** what the daemon does next is a `daemon::Wake`: a round (`Timer`, or
`Resumed` when only the wall clock says it is due, after a suspend), `Account(name)`, a watcher's
other news (`Watch`), or `Stop`. `daemon::on_wake` handles each, outside the runtime as before.

- **The watcher** (`nm_imap::Watcher`, `nm-imap/src/watch.rs`) is one extra connection per IMAP
  account. It connects as the sync does (TLS or STARTTLS, the certificate pin), logs in, and gives
  up with `Ok(None)` when CAPABILITY has no IDLE. Otherwise it runs `EXAMINE INBOX` and IDLE (RFC
  2177), and calls back on the first IDLE and on every untagged EXISTS, EXPUNGE or FETCH. It sends
  DONE and IDLE again every 29 minutes on the same connection. Every wait has a deadline, DONE's
  reply included, so a connection that died silently ends it within 30 minutes. The wire tests
  hold it to the sync's read-only command list.
- **The login hand-over:** a `Syncer` built with `push_logins` returns, in `Synced::watch`, the
  `WatchLogin` of each successful IMAP sync (a copy of its password or access token). The watcher
  never touches the keyring or a `password_command`, which cannot run inside `block_on`, and drops
  the secret once logged in.
- **`push::Push`** owns the watchers, as tasks on the daemon's runtime. They run while the daemon
  waits or syncs inside `block_on`. `Push::start` runs after each sync that handed over a login. It
  starts a watcher only when none runs, the server was not found without IDLE, and the last start
  was at least a minute ago (`START_EVERY`), so a server that drops every connection cannot make a
  loop. A start held back by that limit sends the account a `push::Kind::WakeAt` for when the
  minute is up, on the watchers' channel, so that sync starts the watcher then rather than at
  the next round. A resumed round ends every watcher and lifts that limit (`replace_all`), and a
  stop ends them all (`stop_all`). Each start gets a generation, so a replaced watcher's late news
  is ignored. `Push::event` turns news into the `push on`, `push off` and `push failing` lines,
  printed when the account's state changes.
- **The account wake:** a watcher sends `push::Kind::Wake` on the first IDLE, which covers mail
  that came in between the sync and the IDLE. It sends one on each change, and one when it ends
  after idling, so that sync catches up and starts the next watcher. `Stop::wait_until` keeps
  woken accounts in `Wakes` for `SETTLE` (2 s), so a burst makes one sync. Wakes that arrive
  during the account's sync are read after it, so they make one more. A round drops the waiting
  wakes, since it syncs every account, and an account wake never moves the next round. The sync
  goes through `round`'s per-account path, so the account lock, `Reports` and the notifications
  are the same.

- **The Outlook check:** Graph sends change notifications only to a public HTTPS endpoint, Azure
  Event Hubs or Event Grid
  ([webhook delivery](https://learn.microsoft.com/en-us/graph/change-notifications-delivery-webhooks)),
  so a Graph account gets no watcher. After each of its syncs, or a skip because `neomail sync`
  holds its lock, `Push::checked` sends a `push::Kind::CheckAt` for a minute later (`CHECK_EVERY`)
  on the watchers' channel and says `checked every minute` the first time. `Wakes` keeps the
  latest one (unlike the start limit's `WakeAt`, which keeps the earliest), so a check that ran
  into a round is replaced by the round's. When it falls due it is an account wake, through the
  same path. `on_wake` runs it from `round`'s `after`, per account. `Push::check_between` turns
  checks off when the rounds are a minute apart or less. Each check is a normal sync, which the
  cursor seam makes one delta request when nothing changed.

No trait: there is one watcher, and the tests give `Push` a closure that plays scripted events on
tokio's paused clock, as `round` and `notify_round` take closures.

**Notifications** (`neomail/src/notify.rs`) come after a round, never inside a sync. Which mail is
new comes from the sync: `SyncReport::new_ids` holds the cache ids of the rows it added (the cache's
stores return them, telling an added row from an updated one by `AUTOINCREMENT`'s counter, since ids
are never reused; each store reads the counter inside a `BEGIN IMMEDIATE` transaction, so another
connection cannot commit between the read and the first write), and `SyncReport::initial` marks a
sync of a mailbox it created (the first sync, or a rebuild) until one completes, whose mail is not
new: `mailbox.synced` is set only at the end of a sync that succeeds, so the syncs that finish an
interrupted first sync are initial too. A first sync in which a message came back without a body is
not complete either: it leaves `mailbox.synced` unset, so the sync that later fetches that old
message is initial. The cost: while such a body stays missing, mail that arrives meanwhile is
fetched by an initial sync and never announced. That is the lesser error, since the rule is never to
announce old mail. The engines fill a `&mut SyncReport` batch by batch, so a sync that fails still
hands back the ids it stored (`Synced::partial`): the next round finds them cached and would never
announce them. `daemon::round` returns each account's `NewMail`; `daemon::notify_round` reads the
newest three of them (`Cache::newest_of`), formats one `Notification` per account and hands it to a
closure, as `round` hands accounts to `sync`. The daemon's closure is `Notifier::send`: on Linux the
freedesktop `Notify` call over the session bus with zbus (on tokio, as the keyring store already
uses it), each call inside its own `block_on` with a 5-second timeout, so it fits between the syncs
as they do. `Notifier::start` decides whether notifications are on (`--no-notify`, a service that
owns `org.freedesktop.Notifications` or is activatable, the platform). If no service was found,
`Notifier::ready` looks again before each round's notifications, since a desktop can start its
service after the daemon, and the daemon prints `notifications on` when it finds one. A failure, a
cache read included, goes through `Reports` under the repeated-error rule and ends the round's
notifications, so a service that does not answer costs one timeout per round, not one per account
(two in the round where `ready` finds it: the lookup and the first `Notify` each have their own); a
stop request ends them too. No trait: there is one notifier per platform, the tests pass a closure,
and the D-Bus call is tested against a zbus server on a socket pair.

`sync_inbox` (`nm-core/src/sync.rs`) runs once per account:

1. `EXAMINE INBOX` (read-only). If UIDVALIDITY differs from the cached one, or the cached mailbox
   is a by-id one, the mailbox's cache is dropped and the sync starts as a first sync; the summary
   says so.
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

`sync_inbox_by_id` (`nm-core/src/id_source.rs`) syncs an `IdSource` into a by-id mailbox; a
missing or IMAP mailbox is reset to one first. With nothing cached and no `seen_until` it lists
the `--limit` newest; otherwise everything that arrived since the oldest cached message or
`seen_until`, whichever is earlier, so the window covers every cached row: cached ids not listed
are dropped and changed flags updated. Listed ids not cached are fetched oldest arrival first, 50
per request and transaction. With nothing missing, `seen_until` becomes the newest listed arrival,
else the oldest missing one, saved with the batch it was missing from, so a missing body is asked
for again even if a later batch fails, and an interrupted first sync is finished by the next run.

With a stored cursor, the engine asks `changes_since` instead of listing and applies the round; a
refused cursor is cleared and the sync lists. After a listing with nothing missing, it calls
`start_changes` from the window's start (the oldest cached arrival or `seen_until`, whichever is
earlier), so every cached message is inside the round and a source need not report changes to
older ones. It applies that round and stores its cursor; a source that cannot cover the window
returns `None` (and the engine skips asking when the listing alone holds the source's
`START_LIMIT`), and the engine lists again next time. Applying a round adds, updates and removes
what it names (the last change per id wins, so replays are harmless). A later round never drops a
message it leaves out. A starting round is complete from its start on, so it also drops a cached
message from there on that it leaves out: one deleted or moved out between the listing and the
round, whose removal no later round reports ("resources that were deleted before the initial delta
query aren't returned",
[delta query overview](https://learn.microsoft.com/en-us/graph/delta-query-overview)). A source
never returns a starting round it cut short (Graph's 5,000 with `$filter`), so this cannot empty
the cache. A present message that is not cached is fetched when it arrived
inside the window and ignored otherwise. The cursor is stored only after the whole round is
applied with no body missing; when one is missing, it is cleared so the next sync lists from that
message, and a failed sync keeps the old cursor, whose round the next sync replays. Known limit:
mail moved into the inbox with an arrival time older than the window is never seen: rounds and
listings both leave out what arrived before the window.

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

- **More providers:** none planned. Gmail stays on IMAP; a slice that needs what IMAP can't do
  adds the Gmail API for that part only
  ([DEC-2026-10-12-gmail-imap](decisions.md#dec-2026-10-12-gmail-imap)).
- **More mail:** folders beyond INBOX, and CONDSTORE/QRESYNC to refresh flags cheaply.
- **Actions:** marking read and unread works for IMAP and Outlook through `MailWriter`, and archive
  and delete for IMAP and Outlook (`neomail archive`, `neomail delete`), and listing folders and
  moving to a folder the user names for IMAP and Outlook (`neomail folder list`, `neomail move`).
  Later: moving by role across accounts (to each account's junk, say), creating folders, folders
  beyond INBOX in the cache and in `list`, a fallback for servers without MOVE, a per-account
  archive or trash for servers without special-use attributes, and undo. Everything is online only:
  an offline queue would record pending changes in the cache.
- **The TUI** builds on the same cache and actions as the CLI.
