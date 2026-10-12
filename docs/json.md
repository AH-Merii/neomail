# `--json` output

`neomail list --json` and `neomail show ID --json` print JSON for scripts and agents. The shapes below
are stable: later versions may add fields but will not rename, remove or retype these ones.
Ignore fields you do not know.

Common rules:

- Times are RFC 3339 in UTC (`"2026-10-10T08:30:00Z"`), or `null` when unknown.
- `id` is neomail's id for the message, the one `neomail show` takes. It stays the same across syncs,
  unless the server's UIDVALIDITY changes and the cache is rebuilt. An id is never reused: once
  its message is gone, `neomail show` reports it as not found rather than showing another message.
- `uid` is the IMAP UID in the account's INBOX.
- `flags` are the IMAP flags, sorted, with system flags spelled `\Seen`, `\Flagged` and so on (`"\\Seen"` in JSON).
  `unread` is `true` when `\Seen` is absent.
- An address is `{"name": string | null, "address": string | null}`.
- Text is passed through as is; escape sequences are only neutralised in the human output.

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

## `neomail show ID --json`

One object.

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
- `body_type` is `"text/plain"` (the plain part), `"text/html"` (the HTML part converted to
  text) or `"none"` (no text body; `body` is `""`). HTML larger than 1 MiB or nested more than
  100 levels deep is not laid out: its tags are stripped, and `body` starts with a line
  beginning `[neomail: this HTML is too large or too deeply nested`.
- `attachments` are listed, not saved. `size` is the decoded size in bytes; `name` may be `null`.

## Exit codes

| Code | Meaning |
| ---- | ------- |
| 0 | Success. `list` with nothing cached also succeeds, printing `[]` with `--json`. |
| 1 | Failure: no config, a bad config (`config check` too), an unknown account or id, a network, login or cache error. `sync` exits 1 if any account failed, after syncing the others. |
| 2 | Bad usage: an unknown option or an invalid value; a value that would be asked for when nothing can be asked (standard input is not a terminal, or `--no-input`); an option that does not apply to the provider; bare `neomail`, which prints the start lines or help. |

Errors go to standard error. A failure (exit 1) prints one line starting with `neomail:`. A usage
error (exit 2) comes from the argument parser: it starts with `error:` and may be followed by a
usage hint on further lines.
