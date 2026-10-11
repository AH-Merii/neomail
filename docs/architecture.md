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
(`nm-core/tests/sync.rs`); the IMAP source is checked live against Proton Bridge.

**The secret** comes from the account's `password_command`, run with `sh -c`
(`nmail/src/secret.rs`). The password lives in a `Password` value whose `Debug` is redacted, is
passed to `ImapSource::connect` and dropped after login. Errors never quote the command's output
or the command. The OS keyring and OAuth (slices 2 to 4) add other ways to get a credential; they replace
this function, not the source.

**The cache** (`nm-core/src/cache.rs`) is one SQLite file, `cache.sqlite3`, in the data
directory (created with mode 0700). Tables:

- `mailbox (account, name, uid_validity, highest_uid)`: the sync state;
- `message (mailbox_id, uid, flags, received, date, from_*, subject, raw)`: the message source
  plus the few fields `list` needs, so listing never parses mail.

`PRAGMA user_version` holds the schema version. A newer version than the binary knows is refused
rather than guessed at. `nmail list` and `show` read only the cache and work offline.

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
3. Compare with the cache: changed flags are updated, cached UIDs missing from the listing were
   expunged (or, on a first sync, fell out of the chosen set) and are dropped.
4. Fetch the chosen messages (first sync) or every UID above `highest_uid` (later syncs, no cap,
   so no new mail is skipped) with `UID FETCH … (UID FLAGS INTERNALDATE BODY.PEEK[])`, 50 at a
   time, oldest UID first. `BODY.PEEK` leaves `\Seen` alone. Each batch is stored in one
   transaction. On a later sync it raises `highest_uid`, so an interrupted sync resumes where it
   stopped. A first sync sets `highest_uid` to the highest UID in its listing only when it
   completes; an interrupted one runs again as a first sync and skips what it already stored.
5. Print `name: N new, N updated, N removed`.

A second sync with no changes on the server fetches no message bodies and prints `0 new, 0 updated,
0 removed`.

## Output

`list` prints one line per message within 80 columns; `show` prints the headers, then the
text/plain part, or the HTML part converted to text (html2text), then the attachments. Control
characters and bidirectional overrides in mail are neutralised before they reach the terminal.
`--json` shapes and exit codes are in [json.md](json.md) and only grow by adding fields.

## CI and edge builds

`.github/workflows/ci.yml` runs `cargo fmt --check`, clippy with `-D warnings` and the tests on
Linux and macOS for every pull request and push to `main`. Each push to `main` then builds
`nmail` for x86_64 Linux and Apple-silicon macOS and publishes them as the rolling `edge`
pre-release: the `edge` tag moves to the commit, the assets are replaced and the notes name the
commit ([DEC-2026-10-11-main-usable](decisions.md#dec-2026-10-11-main-usable)). Only the
publishing job can write to the repository, and a newer push cancels an older run.

## What comes next

The order of the slices is in [roadmap.md](roadmap.md). What they change in the code:

- **More providers:** Microsoft Graph and the Gmail API become new `MailSource` crates; Graph's
  delta sync may widen the trait (a sync token next to UIDs).
- **More mail:** folders beyond INBOX, CONDSTORE/QRESYNC to refresh flags cheaply, and IDLE.
- **Actions:** flags, moves and deletes need a write side on the source and a way to record
  pending changes in the cache.
- **The TUI** builds on the same cache and actions as the CLI.
