# Output for scripts

`neomail list`, `neomail account list` and `neomail show` print JSON for scripts and agents with
`--json`, and JSON Lines (one compact object per line, the same objects as `--json`'s array) with
`--jsonl`. The shapes below are stable: later versions may add fields but will not rename, remove
or retype these ones, and a field that can be `null` says so. Ignore fields you do not know.
Listings also have a plain format, below.

Common rules:

- Times are RFC 3339 in UTC (`"2026-10-10T08:30:00Z"`), or `null` when unknown.
- `id` is neomail's id for the message, the one `neomail show` takes. It stays the same across syncs,
  unless the server's UIDVALIDITY changes and the cache is rebuilt. An id is never reused: once
  its message is gone, `neomail show` reports it as not found rather than showing another message.
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

## Exit codes

| Code | Meaning |
| ---- | ------- |
| 0 | Success. `list` with nothing cached also succeeds, printing `[]` with `--json`. |
| 0 | Also when the reader of standard output goes away early (`neomail list \| head -1`, quitting the pager): neomail stops quietly. |
| 0 | `daemon` stopped by SIGINT or SIGTERM. A failing account does not stop it. |
| 1 | Failure: no config, a bad config (`config check` too), an unknown account or id, a network, login or cache error. `sync` exits 1 if any account failed, after syncing the others. `show` exits 1 if any id was missing or unreadable, after showing the others. A second `daemon` for the same data directory exits 1. |
| 2 | Bad usage: an unknown option or an invalid value; output flags that conflict (`--plain`, `--json`, `--jsonl`, `--raw`); `--raw` with more than one id; a line of `show -`'s input without an id; a value that would be asked for when nothing can be asked (standard input is not a terminal, or `--no-input`); an option that does not apply to the provider; bare `neomail`, which prints the start lines or help; `daemon` without a config file or accounts (the start lines) or with an invalid config. |

Errors go to standard error, in these forms:

- An error from the argument parser (exit 2: an unknown option, an invalid value, conflicting
  output flags) starts with `error:` and may be followed by a usage hint on further lines.
- neomail's own usage errors (exit 2) are one line starting with `neomail:`: a value that cannot
  be asked for (standard input is not a terminal, or `--no-input`), a line of `show -`'s input
  without an id, `--raw` with several ids, `-` mixed with ids, an option that does not apply to
  the provider. They look like failures; only the exit code tells them apart.
- Bare `neomail` (exit 2) prints the start lines (`neomail: no accounts yet. Start with:` and the
  commands) without a config, or the help with one.
- A failure (exit 1) prints a line starting with `neomail:`; `sync` prints one per failed account.
  A failed `config check` prints `PATH:LINE:COL: message` for a syntax error or `PATH: message`
  for an invalid value, without the `neomail:` prefix; a missing config file is an ordinary
  `neomail:` failure.

`jq -r` decodes the strings in JSON, so it prints any control character in them raw, ESC and bidi
controls included: a subject can then move the cursor or reorder the text on a terminal. `jq -c`
is not enough either: it escapes only the C0 controls, and prints bidi controls and C1 controls
(such as U+009B, a one-byte CSI) as raw UTF-8. Print untrusted fields with `jq -ac`, which escapes
every character outside ASCII as `\uXXXX`; or use the plain listings, which turn those characters
into spaces (see Plain output); or keep the output off the terminal.
