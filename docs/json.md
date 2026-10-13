# Output for scripts

`neomail list`, `neomail account list`, `neomail folder list`, `neomail show`, `neomail redact`,
`neomail mark`, `neomail archive`, `neomail delete` and `neomail move` print JSON for scripts and agents with
`--json`, and JSON Lines (one compact object per line, the same objects as `--json`'s array) with
`--jsonl`. The shapes below are stable: later versions may add fields but will not rename, remove or
retype these ones, and a field that can be `null` says so. Ignore fields you do not know.
Listings also have a plain format, below.

Common rules:

- Times are RFC 3339 in UTC (`"2026-10-10T08:30:00Z"`), or `null` when unknown.
- `id` is neomail's id for the message, the one `neomail show` takes. It stays the same across syncs,
  unless the server's UIDVALIDITY changes and the cache is rebuilt. An id is never reused: once
  its message is gone (deleted on the server, or moved out of the INBOX by `archive`, `delete` or
  `move`),
  `neomail show` reports it as not found rather than showing another message.
- `uid` is the IMAP UID in the account's INBOX, or `null` for a message read through Microsoft
  Graph (Outlook), which has no UIDs.
- `flags` are the IMAP flags, sorted, with system flags spelled `\Seen`, `\Flagged` and so on (`"\\Seen"` in JSON).
  `unread` is `true` when `\Seen` is absent.
- An address is `{"name": string | null, "address": string | null}`.
- Text is passed through as is; escape sequences are only neutralised in the human and plain
  output.
- JSON, JSON Lines and `--raw` output never contain colour codes, whatever `--color` says.

## `neomail list --json`

An array, newest first. Empty when nothing is cached.

```json
[
  {
    "id": 42,
    "account": "work",
    "uid": 1187,
    "date": "2026-10-10T08:30:00Z",
    "from": { "name": "Ada Lovelace", "address": "ada@mail.example" },
    "subject": "Hello there",
    "unread": true,
    "flags": []
  }
]
```

`date` is the message's Date header, or the server's arrival time when the header is missing.
`from` and `subject` are `null` when the message has none.

## `neomail account list --json`

An array, in config order. Empty when there are no accounts. Secrets and password commands are
never included.

```json
[
  {
    "name": "work",
    "login": "password",
    "username": "you@example.com",
    "host": "imap.example.com",
    "port": 993,
    "security": "tls",
    "cert_pinned": false,
    "server": "imap"
  }
]
```

`login` is `"password"` (the OS keyring), `"password_command"` or `"oauth"`; `security` is `"tls"`
or `"starttls"`. `server` is `"imap"` or `"graph"`. A Graph account (Outlook) reads its mail from
Microsoft Graph over HTTPS, so its `host`, `port` and `security` are `"graph.microsoft.com"`, `443`
and `"tls"`.

## Plain output

Listings print tab-separated values when standard output is not a terminal, or with `--plain`:
one record per line, fields separated by one tab, no header, no padding, nothing cut off. Every
control character in a field (tab, CR, LF, ESC and the rest of C0 and C1) and every bidi control
becomes one space, so each record has the same number of fields. With `--color always` the fields
carry colour codes (for `fzf --ansi`). So they do with `CLICOLOR_FORCE` set (to anything but
empty or `0`) when `NO_COLOR` is not set and `--color` is not given or `auto`. `--color never`
makes sure they carry none.

| Command | Fields |
| ------- | ------ |
| `list` | id, unread (`*` or empty), date (RFC 3339 UTC as in `--json`, empty when unknown), account, from (`Name <address>`, the address, or empty), subject |
| `account list` | name, login, username, host, port, security, `pinned` or empty |
| `folder list` | name (exactly what `move` takes), account, role or empty |

## `neomail show ID --json`

One object for one id given on the command line. For several ids, or `-`, an array of these
objects in the order asked, even when `-` brings one id; `--jsonl` prints one object per line in
every case. Missing messages are left out of the output and reported on standard error.

```json
{
  "id": 42,
  "account": "work",
  "uid": 1187,
  "date": "2026-10-10T08:30:00Z",
  "received": "2026-10-10T08:30:04Z",
  "from": [{ "name": "Ada Lovelace", "address": "ada@mail.example" }],
  "to": [{ "name": null, "address": "user@mail.example" }],
  "cc": [],
  "subject": "Hello there",
  "unread": true,
  "flags": [],
  "body_type": "text/plain",
  "body": "Plain body.\n",
  "attachments": [
    { "name": "report.pdf", "content_type": "application/pdf", "size": 5120 }
  ]
}
```

- `date` is the Date header; `received` is when the server got the message.
- `subject` is `null` when the message has none.
- `body_type` is `"text/plain"` (the plain part), `"text/html"` (the HTML part converted to
  text) or `"none"` (no text body; `body` is `""`). HTML larger than 1 MiB or nested more than
  100 levels deep is not laid out: its tags are stripped, and `body` starts with a line
  beginning `[neomail: this HTML is too large or too deeply nested`.
- `attachments` are listed, not saved. `size` is the decoded size in bytes; `name` may be `null`.

## `neomail show -`

`-` reads the ids from standard input, one per line, so `neomail list | fzf -m | neomail show -`
works. A line's id is its first field: after leading whitespace and colour codes, the digits up to
the first tab or space. Blank lines are skipped; a line without an id is a usage error (exit 2)
naming its line number, and nothing is shown. `--raw` takes exactly one id.

## `neomail redact ID...`

The text neomail will send to jev for classification, with personal data masked
([privacy.md](privacy.md) lists the rules and placeholders). It reads the cache only. Ids are
taken as `neomail show` takes them, `-` included. `--json` prints one object for one id given on
the command line, else an array in the order asked; `--jsonl` prints one object per line.
Unknown or unreadable messages are left out and reported on standard error.

```json
{
  "id": 42,
  "account": "work",
  "subject": "Order <number> for <name>",
  "body": "Hi <name>, call <phone> or see <link host=shop.example>.\n",
  "truncated": false,
  "masks": {
    "email": 0, "link": 1, "iban": 0, "card": 0, "amount": 0,
    "code": 0, "phone": 1, "number": 1, "name": 2
  }
}
```

- `subject` is the whole subject, redacted; `""` when the message has none.
- `body` is the body as `neomail show` renders it (the plain part, else the HTML part as text),
  redacted, then cut to its first 1,500 characters (Unicode scalar values), never inside a
  placeholder. It is `""` when the message has no text body.
- `truncated` is `true` when the body was cut.
- `masks` counts the placeholders in `subject` and `body` by kind. Every kind is present, zeros
  included; the keys are `email`, `link`, `iban`, `card`, `amount`, `code`, `phone`, `number` and
  `name`.

Without `--json` or `--jsonl`, `redact` prints `Subject: ` and the subject, a blank line, then the
body; `(no text body)` stands for an empty one, and a line
`[neomail: the body is cut here, at the 1500-character limit]` follows a cut one; a mask that would
cross the limit is left out whole, so the body can end a little before it. Several messages are
separated by a `── message ID ──` line as in `neomail show`. On a terminal the placeholders are
coloured; off one, or with `--plain`, the text is the same without colour (unless `--color always`
or `CLICOLOR_FORCE` asks for it). Escape sequences and other control characters are neutralised as
in `neomail show`; JSON keeps the text exactly.

## `neomail mark read|unread ID...`

`mark` takes ids as `show` does: on the command line, or `-` for standard input with the same
rules as `show -` (`neomail list | fzf -m | neomail mark read -`). It changes the read state on
the server first, then in the cache. Standard output carries only the messages now in the asked
state, a message already in it included, in the order asked:

- on a terminal, one line: `Marked 2 messages read.`;
- plain (in a pipe, or `--plain`): their ids, one per line, so the output pipes into another
  command;
- `--json`: an array of objects, `[]` when none was marked; `--jsonl`: one object per line.

```json
[
  { "id": 42, "account": "work", "unread": false, "flags": ["\\Flagged", "\\Seen"] }
]
```

`id`, `account`, `unread` and `flags` mean what they mean in `list --json`. `flags` are as now
cached: the flags the server reported after the change (for Outlook, `\Seen` from Graph's
`isRead`) or, when the server confirmed the change without them, the cached flags with `\Seen`
set or cleared.

Each failure is one `neomail:` line on standard error:

- an unknown id;
- a message gone from the server: the next sync removes it from the cache;
- a message the server did not confirm in the asked state: run `neomail sync` to see its state,
  then try again;
- an account that failed, in one line naming all its ids, which "may not be marked": the server
  may have changed some of them before the error, and the next sync reads their state. For
  example, its server could not be reached or refused the login, its INBOX's UIDVALIDITY changed
  (run `neomail sync`), or an Outlook sign-in may only read mail (run
  `neomail account login NAME`, see [outlook.md](outlook.md)).

Nothing is queued. The cache keeps a failed message's flags, unless the server reported other
ones for it.

## `neomail archive ID...` and `neomail delete ID...`

`archive` moves messages from the INBOX to their account's archive, `delete` to its trash. Neither
deletes a message for good, and neomail never empties the trash. They take ids as `mark` does
(`neomail list | fzf -m | neomail archive -`), on the server first: a message the server confirmed
moved leaves the cache, as when a sync finds a message gone, so `neomail list` no longer shows it
and its id is never used again. Standard output carries only the moved messages, in the order
asked:

- on a terminal, one line: `Archived 2 messages.` or `Moved 2 messages to the trash.`;
- plain: their ids, one per line;
- `--json`: an array of objects, `[]` when none moved; `--jsonl`: one object per line.

```json
[
  { "id": 42, "account": "gmail", "folder": "[Gmail]/All Mail" },
  { "id": 43, "account": "proton", "folder": "Archive" }
]
```

`id` and `account` mean what they mean in `list --json`. `folder` is the destination: an IMAP
mailbox name as LIST reports it, decoded from modified UTF-7
([RFC 3501 section 5.1.3](https://www.rfc-editor.org/rfc/rfc3501#section-5.1.3)) as
`folder list` shows it; for Outlook, the path `folder list` shows for `move`, and Graph's
well-known folder name, `archive` or `deleteditems`, for `archive` and `delete`
([mailFolder](https://learn.microsoft.com/en-us/graph/api/resources/mailfolder)). On IMAP the
archive is the mailbox with the `\Archive` attribute or, when there is none, the one with `\All`
(Gmail's All Mail); the trash is the one with `\Trash`
([RFC 6154](https://www.rfc-editor.org/rfc/rfc6154.html#section-2)).

Each failure is one `neomail:` line on standard error, as for `mark`: an unknown id; a message
gone from the INBOX (the next sync removes it from the cache); a message the server did not
confirm moved (run `neomail sync` to see where it is, then try again); or an account that
failed, in one line naming all its ids, which "may not be archived" or "may not be moved to the
trash". An account fails, and none of its messages moves, when its server has no archive or
trash folder, does not support IMAP MOVE
([RFC 6851](https://www.rfc-editor.org/rfc/rfc6851.html)), or its INBOX's UIDVALIDITY changed
(run `neomail sync`). An Outlook account fails the same way when its mailbox has no such folder,
its sign-in lets neomail read mail but not change it (run `neomail account login NAME`), or Graph
refuses the move ([docs/outlook.md](outlook.md#where-archive-and-delete-put-a-message)). A
failed message stays in the cache.

## `neomail folder list`

Lists each account's folders, read from its server: every account in config order, or only the
one `--account` names. Only folders a message can be in: a mailbox the server marks `\Noselect`
or `\NonExistent` ([RFC 5258 section 3](https://www.rfc-editor.org/rfc/rfc5258#section-3)) is
left out, and INBOX is listed with the role `inbox`. An account's folders come in the order its
server lists them, which can change from one run to the next (a live check saw Proton Mail
Bridge's change), so sort them when a script needs a stable order.

- on a terminal: aligned columns, the name, the account and the role;
- plain: name, account and role or empty, tab-separated (see Plain output);
- `--json`: an array of objects; `--jsonl`: one object per line.

```json
[
  { "name": "INBOX", "account": "proton", "role": "inbox" },
  { "name": "Folders/Receipts", "account": "proton", "role": null },
  { "name": "[Gmail]/All Mail", "account": "gmail", "role": "all" }
]
```

`name` is the name `neomail move` takes: on IMAP the mailbox name LIST reports, decoded from
modified UTF-7, with the server's hierarchy delimiter as it is (`Folders/Receipts` on Proton Mail
Bridge, `[Gmail]/Spam` on Gmail). A name that is not valid modified UTF-7 is shown as the server
sent it. `role` is `null` or one of `inbox`, `archive`, `trash`, `junk`, `sent`, `drafts`, `all` and
`flagged`, from the mailbox's special-use attribute (`\Archive`, `\Trash`, `\Junk`, `\Sent`,
`\Drafts`, `\All`, `\Flagged`,
[RFC 6154 section 2](https://www.rfc-editor.org/rfc/rfc6154#section-2)). More roles may be added
later.

On Outlook, `name` is the folder's path of display names from the top of the mailbox, joined with
`/` (`Inbox`, `Inbox/Receipts`, `Receipts`), and `role` comes from Graph's well-known folder names:
`inbox`, `archive`, `deleteditems` (trash), `junkemail` (junk), `sentitems` (sent) and `drafts`
([mailFolder](https://learn.microsoft.com/en-us/graph/api/resources/mailfolder)). Graph has no
folder for all mail or flagged mail, so `all` and `flagged` never appear there. Search folders and
hidden folders are left out ([docs/outlook.md](outlook.md#folders-and-move)).

An account that fails is one `neomail:` line on standard error, after the other accounts'
folders are printed.

## `neomail move FOLDER ID...`

Moves messages from the INBOX to the folder named FOLDER in each message's own account, as
`archive` does: the same ids, `-` for ids on standard input
(`neomail list | fzf -m | neomail move Receipts -`), the server first, then the cache. Its output
is `archive`'s: on a terminal `Moved 2 messages to Receipts.`; plain, the ids; JSON, the same
objects, with `folder` the folder's name as `folder list` shows it.

FOLDER matches a name of `folder list` exactly. On IMAP only INBOX matches in any case
([RFC 3501 section 5.1](https://www.rfc-editor.org/rfc/rfc3501#section-5.1)), and it is refused:
the messages are in it already. On Outlook the Inbox is refused by its id, under the path
`folder list` shows for it, whatever its display name. A folder that cannot hold messages (an
IMAP mailbox marked `\Noselect` or `\NonExistent`, an Outlook search folder) is refused too. An
account without a folder of that name fails, none of its messages moves, and its line says to run
`neomail folder list --account NAME`, as does the line for a folder that cannot hold messages. An
account where two mailboxes decode to that same name, or an Outlook account where two folders have
that path (a display name can hold a `/`), fails the same way; its line says to use another mail
app (on Outlook, Outlook on the web) for that folder.
neomail never creates, renames or deletes a folder. The other failures, and their lines ("may
not be moved to Receipts"), are `archive`'s, except that a message the server did not take out of
the INBOX says only to run `neomail sync`, not to try again: a move into a label, such as Proton
Bridge's `Labels/...`, may only add the label, so it would stay in the INBOX again.

## Exit codes

| Code | Meaning |
| ---- | ------- |
| 0 | Success. `list` with nothing cached also succeeds, printing `[]` with `--json`. |
| 0 | Also when the reader of standard output goes away early (`neomail list \| head -1`, quitting the pager): neomail stops quietly. |
| 0 | `daemon` stopped by SIGINT or SIGTERM. A failing account does not stop it. |
| 1 | Failure: no config, a bad config (`config check` too), an unknown account or id, a network, login or cache error. `sync` exits 1 if any account failed, after syncing the others. `show` exits 1 if any id was missing or unreadable, after showing the others; `redact` the same, after printing the others. `mark` exits 1 if any id was unknown, gone from the server, not confirmed or of an account that failed, after marking the others. `archive` and `delete` exit 1 the same way, after moving the others; an account without an archive or trash folder is an account that failed. `move` exits 1 the same way; an account without the folder named is an account that failed. `folder list` exits 1 if any account failed, after listing the others. A second `daemon` for the same data directory exits 1. |
| 2 | Bad usage: an unknown option or an invalid value; output flags that conflict (`--plain`, `--json`, `--jsonl`, `--raw`); `--raw` with more than one id; a line of `show -`'s, `redact -`'s, `mark -`'s, `archive -`'s, `delete -`'s or `move -`'s input without an id; `-` mixed with ids (`show`, `redact`, `mark`, `archive`, `delete`, `move`); `move` without a folder or without ids; a value that would be asked for when nothing can be asked (standard input is not a terminal, or `--no-input`); an option that does not apply to the provider; bare `neomail`, which prints the start lines or help; `daemon` without a config file or accounts (the start lines) or with an invalid config. |

Errors go to standard error, in these forms:

- An error from the argument parser (exit 2: an unknown option, an invalid value, conflicting
  output flags) starts with `error:` and may be followed by a usage hint on further lines.
- neomail's own usage errors (exit 2) are one line starting with `neomail:`: a value that cannot be
  asked for (standard input is not a terminal, or `--no-input`), a line of `show -`'s, `redact -`'s, `mark -`'s,
  `archive -`'s, `delete -`'s or `move -`'s input without an id, `--raw` with several ids, `-` mixed
  with ids, an option that does not apply to the provider. They look like failures; only the exit
  code tells them apart.
- Bare `neomail` (exit 2) prints the start lines (`neomail: no accounts yet. Start with:` and the
  commands) without a config, or the help with one.
- A failure (exit 1) prints a line starting with `neomail:`; `sync` and `folder list` print one per
  failed account, and `mark`, `archive`, `delete` and `move` one per unknown, gone or unconfirmed id
  and one per failed account, naming its ids.
  A failed `config check` prints `PATH:LINE:COL: message` for a syntax error or `PATH: message`
  for an invalid value, without the `neomail:` prefix; a missing config file is an ordinary
  `neomail:` failure.

`jq -r` decodes the strings in JSON, so it prints any control character in them raw, ESC and bidi
controls included: a subject can then move the cursor or reorder the text on a terminal. `jq -c`
is not enough either: it escapes only the C0 controls, and prints bidi controls and C1 controls
(such as U+009B, a one-byte CSI) as raw UTF-8. Print untrusted fields with `jq -ac`, which escapes
every character outside ASCII as `\uXXXX`; or use the plain listings, which turn those characters
into spaces (see Plain output); or keep the output off the terminal.
