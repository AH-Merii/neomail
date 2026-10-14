# Output for scripts

`neomail list`, `neomail account list`, `neomail folder list`, `neomail show`, `neomail redact`,
`neomail mark`, `neomail archive`, `neomail delete`, `neomail move`, `neomail send`,
`neomail reply` and `neomail forward` print JSON for scripts and agents with `--json`, and JSON
Lines (one compact object per line) with `--jsonl`. `--jsonl` gives the same objects as `--json`:
`--json` prints them as an array, except for `send`, `reply` and `forward`, which print one
object. The shapes below are stable: later versions may add fields but will not rename, remove
or retype these ones, and a field that can be `null` says so. Ignore fields you do not know.
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
    "flags": [],
    "group": "follow-up"
  }
]
```

`date` is the message's Date header, or the server's arrival time when the header is missing.
`from` and `subject` are `null` when the message has none. `group` is the message's group once
`neomail classify` has stored a result for it, else `null`: one of `main`, `follow-up`,
`follow-up-old`, `recruitment`, `receipts-invoices`, `orders-shipping`, `newsletters`,
`promotions`, `notifications`, `spam` and `other`. It is worked out when the list is read: `spam`
when jev chose spam; `other` when jev's confidence is below `classify.other_below`; `follow-up-old`
for a `follow-up` received more than `classify.follow_up_old_days` ago; else jev's choice.
`--group G` lists only that group, and `--group unclassified` only the messages with `null`.

## `neomail search QUERY... --json`

The same objects as `neomail list --json`, for the cached messages that match, newest first, at
most `--limit` (50 by default, or `all`). `--jsonl` prints them one per line. Empty (`[]`, exit 0)
when nothing matches or nothing is cached; in the human and plain formats standard error then
says `neomail: no cached messages match`.

It searches the local cache only: each account's INBOX as synced so far, never the server. The
query's words are joined with spaces and read as:

| Form | Matches |
| ---- | ------- |
| `word` | a word starting with `word` in the subject, the sender (name and address), the To and Cc recipients or the body text (an HTML body as its text; attachments are not searched) |
| `"some words"` | those whole words, next to each other in that order |
| `subject:word`, `from:word`, `to:word` | `word` (or a quoted phrase after the colon) in the subject, the sender, or To and Cc only; the name is matched in any case |

Every word and phrase must match. Case, Latin accents, Arabic short vowels and other marks, the
tatweel, and hamza on alef are ignored; `ي` and `ى`, and `ة` and `ه`, are not. Any other
`name:word` is ordinary text, and a word with no letter or digit (`-`, `*`) is left out. An address
is split into words, so `ada@mail` matches `ada@mail.example`.

`--account NAME`, `--group GROUP` (or `unclassified`) and `--thread ID` narrow the search as
`list`'s flags do; they combine. `--thread ID` is message ID's thread in ID's account: the
messages that share its first References id (the thread's first message), else its first
In-Reply-To id, else its own Message-ID. With `--thread`, the query may be empty, which lists
the whole thread.

Exit 2 when there is neither a query nor `--thread`, when every word was left out, or when
`subject:`, `from:` or `to:` has no word after it. Exit 1 for an account with no cached mail or a
`--thread` id that is not cached.

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
| `search` | the same as `list` |
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

Exactly what `neomail classify` sends to jev for each message, with personal data masked
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
  },
  "signals": {
    "domain": "shop.example", "kind": "role",
    "mailing_list": false, "automated": true, "reply": false
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
- `signals` is what is sent besides the text. `domain` is the From address's domain, lower-cased,
  or `null` without one. `kind` is `role` when the part before the @ is one of a short list
  (`noreply`, `orders`, `support` ... [privacy.md](privacy.md) has it), `personal` for any other
  address, and `unknown` without one. `mailing_list` is `true` when List-Unsubscribe or List-Id
  is present; `automated` when Auto-Submitted is present and not `no`, or Precedence is `bulk`,
  `list` or `junk`; `reply` when In-Reply-To is present.

Without `--json` or `--jsonl`, `redact` prints `Subject: ` and the subject, a line
`Signals: sender shop.example (role) · mailing list: no · automated: yes · reply: no`, a blank
line, then the body; `(no text body)` stands for an empty one, and a line
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

## `neomail classify`

One object per message asked about, in the order the answers arrive; with ids given, a message
that already has a result is printed from the cache first and not asked again. `--json` prints an
array at the end, `--jsonl` one object per line as each answer arrives.

```json
{
  "id": 42,
  "account": "work",
  "group": "orders-shipping",
  "choice": "orders-shipping",
  "confidence": 0.93,
  "probabilities": { "main": 0.01, "follow-up": 0.0, "orders-shipping": 0.95, "other": 0.01 },
  "model": "jev-1.13.0"
}
```

- `choice` is jev's answer, one of the ten options (every group but `follow-up-old`), and
  `confidence` jev's confidence from 0 to 1 ([jev's docs](https://docs.typesafe.ai/confidence.md)).
- `group` is the group the message lists under, worked out as for `list`'s `group`.
- `probabilities` holds the options jev gave a probability, in the request's order.
- `model` is the model id the answer named.
- A message that failed has `error` (one line), and `null` for `group`, `choice`, `confidence`,
  `probabilities` and `model`.

On a terminal each result is one line, `ID ACCOUNT GROUP CONFIDENCE SUBJECT`, with progress on
standard error; `--plain` (the default off a terminal) prints `ID`, `ACCOUNT`, `GROUP` and
`CONFIDENCE` (two decimals) as TSV. A failed message is a line on standard error there. After a
run, standard error says how many were classified and, without `--all`, how many messages were
left outside the window or the cap, and what `neomail classify --all` would then send: every
message without a group, those of the run that failed or were never asked included (with
`--again`, `neomail classify --all --again`, which asks about every message again). It gives that
command's messages, requests (before any retry) and estimated input tokens, the cost for
api.typesafe.ai, and how the tokens were estimated.

`--estimate` sends nothing. With `--json` it prints one object:

```json
{
  "messages": 1000, "requests": 1000, "input_tokens": 520000, "cost_usd": 0.02184,
  "tokens_from": "measured",
  "left": { "messages": 2400, "requests": 2400, "input_tokens": 1248000, "cost_usd": 0.052416 }
}
```

`tokens_from` is `measured` (the average `input_tokens` of stored results) or `characters` (the
requests' characters divided by 4), `null` when there is nothing to estimate. `cost_usd` is `null`
for a backend other than api.typesafe.ai. `left` is what is outside the window or the cap; the
terminal and plain forms add what `neomail classify --all` (or `--all --again`) would send
instead of this run, both parts together.

## `neomail key set|delete|status`

`key set` and `key delete` print one line saying what was stored or deleted, and where.
`key status` prints `HOST: the key comes from the OS keyring` or `... from JEV_API_KEY`, never
the key, and exits 1 with `HOST: no jev key: ...` when there is none.

## `neomail send`, `neomail reply ID` and `neomail forward ID`

Each sends one message. With `--json` it prints one object (not an array), with `--jsonl` the
same object on one line:

```json
{
  "account": "work",
  "from": {"name": "Your Name", "address": "you@example.com"},
  "to": [{"name": null, "address": "ada@example.com"}],
  "cc": [],
  "bcc": [{"name": null, "address": "eve@example.com"}],
  "subject": "Re: Plans",
  "message_id": "5f0c3a1e9b2d4c7a8e6f1d2c3b4a5968.neomail@example.com",
  "in_reply_to": "three@example.com",
  "attachments": [{"name": "report.pdf", "content_type": "application/pdf", "size": 48213}],
  "sent_copy": "server"
}
```

- `account` is the account it was sent from, `from` the address it was sent as.
- `bcc` lists who got a blind copy; no header of the message names them.
- `message_id` is the new message's Message-ID, and `in_reply_to` the one it answers (`null` for
  `send` and `forward`), both without angle brackets.
- `attachments` lists the files sent with `--attach`, in order: the name the recipient sees,
  the type neomail found from the content, and the size in bytes before encoding. It is `[]`
  without files.
- `sent_copy` says where the copy in Sent is: `"server"` when the server keeps it (Gmail, Proton
  Mail Bridge, Outlook), `"appended"` when neomail stored it in the mailbox marked `\Sent`, or
  `"failed"` when that append failed. The message was sent in every case: a `"failed"` copy
  exits 0 with a warning on standard error, so a retry never sends it twice.

Without `--json` or `--jsonl`, a terminal gets one line, `sent from work to ada@example.com,
eve@example.com (<MESSAGE-ID>)`, and a pipe, or `--plain`, the Message-ID alone. `--dry-run`
prints the whole message (RFC 5322, CRLF line ends) instead, with who it goes to on standard
error (Bcc too, which no header names), and sends nothing; it does not combine with `--json`,
`--jsonl` or `--plain`. A dry run checks what the config alone shows: an account with no SMTP
server, or Bcc from Outlook, exits 1 as the real send would. It reads no keyring and connects to
nothing, so a login or an Outlook sign-in without `Mail.Send` shows only when sending. Exit 0
means the server accepted the message, not that it was delivered.

## `neomail attachment list|save|open`

The attachments of one cached message, read from the cache: nothing is fetched. `list` and `save`
print listings (an array with `--json`, one object per line with `--jsonl`, TSV off a terminal);
`open` prints one object with `--json`. A message without attachments lists `[]`.

`neomail attachment list ID --json`:

```json
[
  {
    "id": 42,
    "account": "work",
    "index": 1,
    "name": "report.pdf",
    "safe_name": "report.pdf",
    "declared_type": "application/pdf",
    "real_type": "application/pdf",
    "type_mismatch": false,
    "size": 5120,
    "inline": false,
    "content_id": null,
    "executable": false
  }
]
```

- `index` counts from 1 in the message's MIME order; `save` and `open` take it.
- `name` is the name the sender gave, as sent, or `null`; `safe_name` is the name `save` and
  `open` write: no directories, control, bidi or invisible characters, leading dots or dashes, at
  most 200 bytes, `attachment-INDEX` when nothing is left, `.eml` added to a nested message's,
  and `-INDEX` (then `-2`, `-3` and so on) added when two attachments would share it.
- `declared_type` is the part's Content-Type, lower case (`application/octet-stream` without
  one). `real_type` is told from the content, `application/octet-stream` when neomail cannot
  tell, and `message/rfc822` for a nested message, which is one attachment.
- `type_mismatch` is `true` when the real type contradicts the declared type or the safe name's
  extension. `executable` is `true` for an executable, installer, shortcut or script, by real
  type or by extension. `inline` is `true` when an HTML body shows the part through a `cid:`
  reference to its `content_id` (without angle brackets, or `null`).
- `size` is the decoded size in bytes: the bytes as sent, with only the transfer encoding
  undone and no charset conversion.

`neomail attachment save ID [INDEX...] --json` (all attachments without an index):

```json
[
  { "id": 42, "index": 1, "path": "/home/you/report.pdf", "size": 5120 }
]
```

`path` is absolute, in `--dir` or the current directory. Files get mode 0600 and never replace
one that exists unless `--overwrite`.

`neomail attachment open ID INDEX --json`:

```json
{ "id": 42, "index": 1, "path": "/home/you/.local/share/neomail/attachments/42-1/report.pdf", "opener": "xdg-open" }
```

`opener` is the program run on the file: `NEOMAIL_OPENER`'s first word when set, else `open`
(macOS), `wslview` (WSL) or `xdg-open`.

Plain output:

| Command | Fields |
| ------- | ------ |
| `attachment list` | index, safe name, real type, declared type, size in bytes, notes (`inline`, `mismatch`, `executable`, comma-separated, or empty) |
| `attachment save` | index, path |

Without `--json`, `open` prints the file's path on standard output and `opened NAME with
PROGRAM`, and that the copy is removed after a day, on standard error.

Exit codes: 1 for an unknown id, an unreadable message or an unknown index; for `save`, also a
file that exists without `--overwrite` or cannot be written, after saving the others; for
`open`, a no at its question about an executable or script, or an opener that failed. 2 when
`open` meets an executable or script without `--yes` and nothing can be asked (standard input is
not a terminal, or `--no-input`): nothing is opened.

## Exit codes

| Code | Meaning |
| ---- | ------- |
| 0 | Success. `list` with nothing cached also succeeds, printing `[]` with `--json`. |
| 0 | Also when the reader of standard output goes away early (`neomail list \| head -1`, quitting the pager): neomail stops quietly. |
| 0 | `daemon` stopped by SIGINT or SIGTERM. A failing account does not stop it. |
| 1 | Failure: no config, a bad config (`config check` too), an unknown account or id, a network, login or cache error. `sync` exits 1 if any account failed, after syncing the others, or when the classify run its first-sync question started fails, consent refused at the prompt included, or the question could not be set up. `show` exits 1 if any id was missing or unreadable, after showing the others; `redact` the same, after printing the others. `mark` exits 1 if any id was unknown, gone from the server, not confirmed or of an account that failed, after marking the others. `archive` and `delete` exit 1 the same way, after moving the others; an account without an archive or trash folder is an account that failed. `move` exits 1 the same way; an account without the folder named is an account that failed. `folder list` exits 1 if any account failed, after listing the others. A second `daemon` for the same data directory exits 1. `classify` exits 1 when any message failed or an id was unknown, when the run stopped (a refused key, a status it cannot retry, the backend out of reach; the results received are kept), without a key, or when consent was refused at the prompt; it exits 0 when everything asked was classified or there was nothing to do. `key status` exits 1 when there is no key; `key set` exits 1 when the key read is empty or blank, and stores nothing. `send`, `reply` and `forward` exit 1 when the server did not confirm the message. Nothing was sent when the server refused a recipient (then nobody gets the message), the message or the login; the account has no SMTP server or cannot send Bcc (Outlook); an Outlook sign-in lacks `Mail.Send`; a reply would go to an address neomail cannot send to yet (one that is not ASCII); or the question was answered no or standard input closed at it. But when the connection was lost or timed out after the whole message went out, or Microsoft Graph failed with a 5xx or its connection was lost, the message may have been sent: the error says so (`may have been sent`), so check Sent before sending it again ([RFC 1047](https://www.rfc-editor.org/rfc/rfc1047)). A failed copy in Sent is not a failure, nor is standard output that cannot be written once the server has accepted the message: a warning on standard error names its Message-ID and says it was sent. |
| 2 | Bad usage: an unknown option or an invalid value; output flags that conflict (`--plain`, `--json`, `--jsonl`, `--raw`); `--raw` with more than one id; a line of `show -`'s, `redact -`'s, `classify -`'s, `mark -`'s, `archive -`'s, `delete -`'s or `move -`'s input without an id; `-` mixed with ids (`show`, `redact`, `classify`, `mark`, `archive`, `delete`, `move`); `move` without a folder or without ids; a value that would be asked for when nothing can be asked (standard input is not a terminal, or `--no-input`); an option that does not apply to the provider; bare `neomail`, which prints the start lines or help; `daemon` without a config file or accounts (the start lines) or with an invalid config; `classify` that would send to a backend it has no consent for, without a terminal or with `--no-input`, and without `--yes`; `list --group` with an unknown group; `classify` ids with `--account`, `--days`, `--limit` or `--all`; `key set --no-input` with nothing on standard input; `key` with a `--backend` that is not a usable URL; `send`, `reply` or `forward` without `--yes` when nothing can be asked (nothing is sent), without a required flag (`--to`, `--subject`, `--body`), with an address that is not one, with a `--from` that names no account, or without `--from` when there are several accounts and nothing to choose by. |
| 0 | `search` with no match, printing `[]` with `--json`. |
| 1 | `search --account NAME` with no cached mail for NAME, or `search --thread ID` with an id that is not cached. |
| 2 | `search` without a query or `--thread`, with no letter or digit in its query, or with `subject:`, `from:` or `to:` and no word after it. |
| 1 | With `--attach`, nothing was sent either when a file cannot be read or is not a regular file, when the files pass the provider's limit (25 MB for Gmail, Proton Mail and Outlook) or the server's SIZE, or when Outlook's draft, upload session or the draft's send failed in a way that says it was not sent; the error says when a draft is, or may be, left in Drafts. |
| 2 | `send`, `reply` or `forward` whose own subject or text says attach, attached or attachment with no `--attach`, when nothing can be asked or with `--yes`, and without `--no-attachment-check` (nothing is sent). |

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
