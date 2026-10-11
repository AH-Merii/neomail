# Architecture

This page follows the code: it describes what exists, not what is planned. Product decisions live
in [decisions.md](decisions.md); this slice is the first one from
[DEC-2026-10-11-build-first](decisions.md#dec-2026-10-11-build-first): a read-only INBOX in the
CLI.

## Crates

| Crate | What it holds | Why it is a crate |
| ----- | ------------- | ----------------- |
| `crates/nmail` | The binary: CLI (clap), config, the password command, output, wiring. | The composition root. Nothing depends on it. |
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

**The secret** comes from the account's `password_command`, run with `sh -c`
(`nmail/src/secret.rs`). The password lives in a `Password` value whose `Debug` is redacted, is
passed to `ImapSource::connect` and dropped after login. Errors never quote the command's output
or the command. Nor do config errors quote the file: a syntax error gives its message, line
and column, never the offending line, which could hold a pasted secret. The OS keyring and OAuth (slices 2 to 4) add other ways to get a credential; they replace
this function, not the source.

**The cache** (`nm-core/src/cache.rs`) is one SQLite file, `cache.sqlite3`, in the data
directory (created with mode 0700). On Unix, `nmail` makes the file and SQLite's `-wal` and
`-shm` files mode 0600 each time it opens the cache, even in a directory that already existed
with looser modes (`nmail/src/main.rs`, `restrict_cache_files`). Tables:

- `mailbox (account, name, uid_validity, highest_uid)`: the sync state;
- `message (mailbox_id, uid, flags, received, date, from_*, subject, raw)`: the message source
  plus the few fields `list` needs, so listing never parses mail.

`PRAGMA user_version` holds the schema version. A new cache gets its schema and version in one
`BEGIN IMMEDIATE` transaction that reads the version again, so two first syncs at once or a
crash cannot leave half a schema. A newer version than the binary knows is refused rather than
guessed at. `nmail list` and `show` read only the cache and work offline.

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

`list` prints one line per message within 80 display columns, measured per grapheme cluster
(unicode-segmentation and unicode-width), so emoji sequences count as the two columns a terminal
gives them. `show` prints the headers, then the text/plain part, or the HTML part converted to
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
`rust-version` in `Cargo.toml`. Each push to `main` then builds `nmail` for x86_64 Linux (musl,
statically linked, so it does not depend on the system's glibc) and Apple-silicon macOS, and
publishes them as the rolling `edge` pre-release
([DEC-2026-10-11-main-usable](decisions.md#dec-2026-10-11-main-usable)). Each tarball holds a
`THIRD-PARTY-LICENSES` file that cargo-about generates from the dependency tree (`about.toml`
lists the accepted licences; a crate under any other fails the build). Once the release exists,
publishing uploads the assets, then updates the notes, which name the commit, and moves the
`edge` tag last, so a cancelled run never leaves the tag on a commit whose assets are missing.
The very first publish has to push the tag before it can create the release; if that run is
cancelled, the next one finishes it. Only the publishing job
can write to the repository, and a newer push cancels an older run.

## What comes next

The order of the slices is in [roadmap.md](roadmap.md). What they change in the code:

- **More providers:** Microsoft Graph and the Gmail API become new `MailSource` crates; Graph's
  delta sync may widen the trait (a sync token next to UIDs).
- **More mail:** folders beyond INBOX, CONDSTORE/QRESYNC to refresh flags cheaply, and IDLE.
- **Actions:** flags, moves and deletes need a write side on the source and a way to record
  pending changes in the cache.
- **The TUI** builds on the same cache and actions as the CLI.
