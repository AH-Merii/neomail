<h1>
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/logo/neomail-logo-dark.svg">
    <img alt="neomail" src="assets/logo/neomail-logo-light.svg" height="48">
  </picture>
</h1>

A fast, modal terminal email client for Outlook, Gmail and Proton Mail.

> **Status:** early development. What works today: the INBOX in the command line
> (`neomail sync`, `neomail list`, `neomail show`): over IMAP with passwords (an app password,
> Proton Mail Bridge) or, for Gmail, OAuth sign-in through your own Google client; and Outlook or
> Hotmail through Microsoft Graph, signed in through your own Microsoft Entra app. `neomail mark`
> marks mail read or unread, and `neomail archive`, `neomail delete` and `neomail move` move mail
> out of the INBOX; otherwise nothing changes on the server. There is no TUI yet.

## Try it

### Install

Everything is pre-alpha. The builds come from the
[edge pre-release](https://github.com/AH-Merii/neomail/releases/tag/edge), which is rebuilt from
`main` on every merge, so expect breaking changes. Prebuilt builds exist for:

- Linux x86_64 (WSL included) and Linux on 64-bit ARM (aarch64: Raspberry Pi OS 64-bit, ARM
  laptops and servers): static musl binaries that need no particular glibc;
- macOS on Apple silicon, when its build passed.

Anything else, 32-bit ARM (armv7l) included, builds from source (below).

The installer downloads the edge build for your platform, checks it against the release's
`SHA256SUMS` and puts `neomail` in `~/.local/bin` (or `$XDG_BIN_HOME`, `$NEOMAIL_INSTALL_DIR`,
`--to DIR`). It edits no shell startup file: it prints the `PATH` line to add if one is needed.

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://raw.githubusercontent.com/AH-Merii/neomail/main/install.sh | sh
```

Run it again to update, or pass `--uninstall` to remove the binary
(`curl ... | sh -s -- --uninstall`); `sh install.sh --help` lists the options.

With [mise](https://mise.jdx.dev/dev-tools/backends/github.html):

```sh
mise use -g github:AH-Merii/neomail@edge
mise install --force github:AH-Merii/neomail@edge   # fetch a newer edge; the tag name never changes
```

With mise's lockfile on, run `mise lock github:AH-Merii/neomail` before that `mise install --force`:
edge is republished under the same tag, so the checksum in `mise.lock` goes out of date.

Or from source, with Rust 1.88 or newer:

```sh
cargo install --locked --git https://github.com/AH-Merii/neomail neomail
```

Each tarball carries `neomail`, this README, `LICENSE-MIT`, `LICENSE-APACHE`, the licences of the
crates built into it (and, in the Linux builds, of musl libc) in `THIRD-PARTY-LICENSES`, and the
Rust standard library's notices in `COPYRIGHT-library.html`.

### Add an account

```sh
neomail account add                # a menu: gmail, outlook, gmail-app-password, proton, imap
neomail sync && neomail list
```

`neomail account add` asks for what it needs, logs in read-only (it opens INBOX with `EXAMINE`,
or for Outlook reads the INBOX's message count through Graph) and only then writes the account to
the config file. Every question has a flag, so scripts can
run it with `--no-input` (or `NEOMAIL_NO_INPUT=1`), which turns any missing value into a usage
error. Secrets are never flags: they are prompted for without echo, read as one line of standard
input, or come from `--password-command`. `--no-check` skips the login. `neomail account add
--help` lists the flags with examples.

- **Proton Mail**, through Proton Mail Bridge (paid Proton plans), on
  127.0.0.1 port 1143 with STARTTLS:

  ```sh
  neomail account add proton
  ```

  It asks for the username and the password Bridge shows, never your Proton password. It reads
  Bridge's self-signed certificate, shows its SHA-256 and asks before pinning it. Off a terminal,
  pass the SHA-256 you expect with `--cert-sha256`; a different certificate is refused. To get
  it, run `neomail account add proton` on a terminal once, or, with Bridge running, read it with
  `openssl s_client -connect 127.0.0.1:1143 -starttls imap </dev/null | openssl x509 -noout
  -fingerprint -sha256 | cut -d= -f2`.

  Bridge's IMAP LIST shows your folders under `Folders/` and your labels under `Labels/`. Proton has
  you create folders under the Folders group and labels under the Labels group
  (["This computer only" folders in Outlook](https://proton.me/support/this-computer-only-folder-outlook)),
  and says "your existing labels will appear as folders within the Labels folder"
  ([Labels and folders in Bridge](https://proton.me/support/labels-in-bridge)). So
  `neomail move Folders/Receipts 42` moves a message to the folder Receipts. Moving into a label
  applies the label, and "the message will appear in both the Labels and the Folder it resides in"
  (the same page): `neomail move Labels/Work 42` labels the message, but it stays in the INBOX, so
  neomail reports it as not confirmed (exit 1), keeps it in the cache, and the next sync shows it
  still there. Moving it there again changes nothing; archive it, or move it to a folder under
  `Folders/`, to take it out of the INBOX. Both rest on Proton's pages: the test account has no
  folder or label of its own, so a live check has tried neither.
- **Gmail with an app password:** `neomail account add gmail-app-password` takes you through it
  one step at a time: whether your account can have an app password (Google may offer none with
  2-Step Verification only for security keys, for work or school accounts, or with Advanced
  Protection: [Google's help](https://support.google.com/accounts/answer/185833)), turning on 2-Step
  Verification, creating the app password, then the name and address. Spaces in the pasted app
  password are removed, and both the IMAP login and an SMTP login that sends nothing are checked
  before anything is written. See [docs/gmail-app-password.md](docs/gmail-app-password.md).
- **Gmail with your own OAuth app:** `neomail account add gmail`, below.
- **Outlook or Hotmail with your own Entra app:** `neomail account add outlook`, below.
- **Any IMAP server:** `neomail account add imap --name work --username you@example.com --host
  imap.example.com` (`--port`, `--security tls|starttls`, `--cert-sha256`).

```sh
neomail account list               # name, login, username, host, port, security, pinned
neomail config path                # where the config file is
neomail config check               # parse it offline: PATH:LINE:COL: message, or "ok: N accounts"
```

### Edit the config by hand

The config is `~/.config/neomail/config.toml` (or `$XDG_CONFIG_HOME/neomail/config.toml`;
`--config PATH` or `NEOMAIL_CONFIG` point elsewhere, and an empty `NEOMAIL_CONFIG` counts as
unset). `account add` appends to it and keeps your text and comments:

```toml
[[accounts]]
name = "work"
username = "user@mail.example"
# Optional: who mail is sent as. `address` defaults to `username`.
display_name = "Your Name"
imap = { host = "imap.mail.example", port = 993, security = "tls" }
# Where mail is sent: implicit TLS on 465 by default, or security = "starttls" (port 587).
# save_sent appends a copy to the mailbox marked \Sent; it defaults to true here.
smtp = { host = "smtp.mail.example" }

[[accounts]]
name = "proton"
username = "user@proton.example"
# Optional: run through `sh -c`; it prints the password. One trailing newline is removed.
password_command = "pass show mail/proton-bridge"
[accounts.imap]
host = "127.0.0.1"
port = 1143
security = "starttls"
# Bridge uses a self-signed certificate: pin its SHA-256 instead of trusting a CA.
cert_sha256 = "AB:CD:…"   # 64 hex digits
```

- `security` is `"tls"` (default, port 993) or `"starttls"` (port 143). There is no plaintext
  option and no way to skip certificate checks.
- Without `cert_sha256` the certificate is verified against the system's trust store. With it,
  only that exact certificate is accepted.
- `smtp` takes the same `security` and `cert_sha256`, and `save_sent`. Gmail and Proton Mail
  Bridge need none: neomail sends through smtp.gmail.com (port 465, TLS) and through Bridge's
  127.0.0.1 port 1025 ([Proton's port list](https://proton.me/support/port-already-occupied-error)),
  and both keep the copy in Sent themselves, so neomail adds none. Proton does not say how
  Bridge's SMTP is secured; neomail uses STARTTLS with the IMAP pin. The live check sent through
  Bridge's SMTP that way, from a hand-written `smtp` table, as its Bridge listened on other ports
  and the preset only matches IMAP on port 1143. `account add proton` and `gmail-app-password`
  write this out. An `smtp` table naming smtp.gmail.com also defaults to `save_sent = false`.
  Any other server needs its `smtp` before it can send: `account add imap --smtp-host HOST`
  writes it (`--smtp-port 587` uses STARTTLS, any other port TLS; `--smtp-security starttls|tls`
  says otherwise, on port 587 or 465 unless `--smtp-port` names one).

An optional `[classify]` table sets up classification with jev; every key has a default:

```toml
[classify]
url = "https://api.typesafe.ai"   # the backend's base URL; requests go to {url}/v1/systemone
model = "jev-1.13.0"              # pinned, so stored confidences stay comparable
other_below = 0.4                 # below this confidence a message lists as `other`
follow_up_old_days = 90           # a `follow-up` older than this lists as `follow-up-old`
daemon = false                    # true: `neomail daemon` classifies new mail in the background
label_prefix = "nm-"              # groups on the server are named nm-newsletters and so on
labels = true                     # false: write no group to the server
junk = true                       # false: never move spam to Junk
junk_at = 0.9                     # spam at or above this confidence moves to Junk
```

- `url` must be `https://`, except for a backend on this machine: `http://127.0.0.1`,
  `http://[::1]` or `http://localhost`, with any port.
- `other_below` (0 to 1) and `follow_up_old_days` (at least 1) apply when mail is listed, so
  changing them needs no new answers. `spam` stays `spam` whatever its confidence.
- `label_prefix` must not be empty or hold `/`, which nests labels on Gmail and Proton. neomail
  never shows it: `list --group` takes and prints the bare group.
- `junk_at` must be above 0 and at most 1. `junk = false` moves nothing.

### Passwords

An account without `password_command` reads its password from the OS keyring: the Secret Service
on Linux (GNOME Keyring, KeePassXC and others), the login keychain on macOS. Store it once:

```sh
neomail account set-password work      # prompts without echo
pass show mail/work | neomail account set-password work   # or one line on stdin
neomail account delete-password work
```

The entry has the service `neomail` and the account name as its `username` attribute. An account
with `password_command` runs the command instead and never touches the keyring. Either way the
password is never written to the config, the cache or a log, and never printed.

- **WSL** often has no Secret Service. Use `password_command` (for example with `pass`), or run a
  Secret Service such as gnome-keyring.
- **Proton Mail:** store the password Bridge shows for the account, never your Proton account
  password.

### Gmail

Gmail signs in with OAuth through a Google OAuth client you create yourself; neomail ships none.
On a terminal, `account add gmail` takes you one step at a time from no Google Cloud project to a
signed-in Desktop client; each step says why it exists and links the console page. It checks the
client ID and secret with Google before you sign in, and logs in to Gmail's IMAP and SMTP after,
before it saves the account. [docs/gmail.md](docs/gmail.md) lists the steps:

```sh
neomail account add gmail          # step by step to a signed-in client, then IMAP and SMTP checks
neomail sync --account gmail
neomail account login gmail        # sign in again (needed every 7 days while the app is in Testing)
neomail account logout gmail       # revoke the sign-in and delete it from the keyring
```

The account is added to the config file without secrets; the client secret and refresh token stay
in the OS keyring (service `neomail-oauth`). `--no-browser` only prints the sign-in URL. On WSL the
Windows browser opens (`wslview` or `explorer.exe`), or the one in `$BROWSER`.

### Outlook

Outlook, Outlook.com and Hotmail sign in with OAuth through a Microsoft Entra app registration you
create yourself, and mail is read through Microsoft Graph. On a terminal, `account add outlook`
takes you one step at a time from no app to a signed-in one, showing only the steps your account
type and sign-in need. It checks the client ID with Microsoft before you sign in, and reads the
inbox through Graph after, before it saves the account. [docs/outlook.md](docs/outlook.md) lists
the steps:

```sh
neomail account add outlook        # step by step: app, client ID, sign-in, then a Graph check
neomail sync --account outlook
neomail account login outlook --device-code   # sign in with a code on another device
neomail account logout outlook     # delete the sign-in from the keyring
```

There is no client secret. Over SSH without a display, sign-in uses a device code on its own.

The cache lives in `~/.local/share/neomail/` (`$XDG_DATA_HOME/neomail/`, or `NEOMAIL_DATA_DIR`).

### Use

```sh
neomail sync                 # fetch new mail for every account: "work: 12 new, 1 updated, 0 removed"
neomail list                 # newest first; * marks unread
neomail show 42              # headers, body, attachments
neomail attachment list 42   # message 42's attachments: index, name, real type, size
neomail redact 42            # what classification sends for a message, personal data masked
neomail key set              # store your jev key in the OS keyring (or set JEV_API_KEY)
neomail classify             # sort the last year's mail into groups with jev, at most 1,000
neomail list --group follow-up   # list one group's mail
neomail search invoice from:ada  # cached mail matching every word, listed as `list` does
neomail mark read 42 43      # mark read on the server, then in the cache
neomail archive 42           # move to the archive on the server, then drop from the cache
neomail delete 43            # move to the trash; nothing is deleted for good
neomail folder list          # each account's folders: name, account, role
neomail move Receipts 44     # move to the folder Receipts on the server, then drop from the cache
neomail send --to ada@example.com --subject Hi --body note.md   # send Markdown, after asking
neomail reply 42 --body answer.md                                # reply, threaded, original quoted
neomail forward 42 --to ada@example.com                          # forward its text inline
```

- `neomail sync --account work --limit 100`: the first sync takes the 500 messages that arrived
  last unless `--limit` says otherwise. Later syncs fetch all mail that is new since, however
  much, refresh flags and drop deleted mail. Messages are fetched without marking them read.
- `neomail list --account work --limit all` lists every cached message; `--limit` takes a number
  or `all` on `list` and `sync`.
- `neomail search WORD...` searches the cache, offline: the INBOX mail synced so far, not the
  server. Each word matches the start of a word in the subject, the sender, the To and Cc
  recipients or the body, and every word must match; case, accents and Arabic short vowels are
  ignored. `"quoted words"` match as a phrase, and `subject:`, `from:` or `to:` before a word or
  phrase look only there. `--account NAME`, `--group GROUP` and `--thread ID` (message ID's
  thread) narrow it; the output, `--limit` and the formats are `list`'s. Mail is indexed as it
  syncs; mail cached before is indexed by the first search.
- `neomail show 42 43` shows several messages; `neomail show 42 --raw` prints one message's source.
  On a terminal `show` goes through `$NEOMAIL_PAGER`, `$PAGER` or `less`; `--no-pager` turns it off.
- `neomail attachment list 42` lists a message's attachments from the cache, offline: an index,
  the name it saves under, the type told from its content, the size, and notes for a type that
  contradicts the declared one or the name (`mismatch`), an image the HTML shows (`inline`) and
  an executable or script (`executable`). `neomail attachment save 42 [INDEX...] [--dir DIR]`
  saves them under safe names, mode 0600, never replacing a file without `--overwrite`.
  `neomail attachment open 42 1` hands a private copy to `xdg-open` (`open` on macOS, or
  `NEOMAIL_OPENER`), and asks first for an executable or script; without a terminal it needs
  `--yes`. neomail never runs a file itself. Output shapes: [docs/json.md](docs/json.md).
- `neomail redact ID...` prints exactly what `neomail classify` sends for each message: the
  subject and the body's first 1,500 characters, with email addresses, links, phone, card and IBAN
  numbers, amounts, one-time codes, runs of 4 or more digits and the names in the headers replaced
  by placeholders such as `<email>@acme.example` or `<name>`, and a line of signals (the sender's
  domain, whether the address is a role such as `orders@`, and whether the mail came through a
  mailing list, was automated or is a reply). It reads the cache only and sends nothing. What it
  masks and what it misses: [docs/privacy.md](docs/privacy.md).
- `neomail key set` stores the classifier's key in the OS keyring (service `neomail-jev`, one entry
  per backend host): typed without echo, or one line of standard input
  (`pass show jev | neomail key set`). `neomail key status` says whether it comes from the
  keyring or `JEV_API_KEY`, never the key; `neomail key delete` removes it. `--backend URL` names
  another backend than `classify.url`.
- `neomail classify` sends each cached message without a group, received in the last 365 days,
  newest first and at most 1,000 (`--days N`, `--limit N`; `--all` takes every one), to jev and
  stores its group. Before the first request to a backend it says what is sent and asks once;
  `--yes` consents in a script. It prints each result as it arrives, then what was left out and
  what the `neomail classify --all` it suggests would send and cost: every message still without
  a group, or with `--again` every message again (TypeSafe charges $0.042 per million input
  tokens).
  `--estimate` prints that and sends nothing. A stored result is never asked for again, even after
  a mailbox rebuild (a UIDVALIDITY change), unless `--again`; deleting `cache.sqlite3` deletes the
  stored results too. Stopping a run during a request, or running it while the daemon classifies,
  can pay for one message twice ([docs/daemon.md](docs/daemon.md#classifying)).
  `neomail classify 42 57` classifies those messages.
  Then each classified message gets its group on its server, up to 1,000 per account: an
  Outlook category or a Gmail or Proton label (`nm-newsletters`), keeping your own categories and
  labels. Spam at or above `junk_at` then moves to the account's Junk folder, once: a message you
  move back stays. Other IMAP servers get no labels, only the move to Junk. A line such as
  `neomail: work: 37 labels written, 2 moved to Junk` says what was done. neomail writes again
  only when a message's group changes, and exits 1 when a write, a move or an account's sign-in
  failed ([docs/outlook.md](docs/outlook.md#groups-as-categories),
  [docs/labels.md](docs/labels.md)).
- `neomail list --group G` lists one group: `main`, `follow-up`, `follow-up-old`, `recruitment`,
  `receipts-invoices`, `orders-shipping`, `newsletters`, `promotions`, `notifications`, `spam`,
  `other`, or `unclassified`. On a terminal `list` shows each message's group once any is
  classified; `--json` gives it as `group`.
- `neomail mark read ID...` and `neomail mark unread ID...` change the read state on the server
  first, then in the cache, so `neomail list` shows it at once. Ids may be of several accounts.
  It needs the server: nothing is queued, and an account that cannot be reached keeps its
  messages as they were. It waits for a sync of the same account (`neomail daemon`'s) to finish.
  An Outlook account added before neomail could change mail needs `neomail account login NAME`
  once ([docs/outlook.md](docs/outlook.md)).
- `neomail archive ID...` moves messages from the INBOX to their account's archive, and
  `neomail delete ID...` to its trash: neomail never deletes a message for good and never empties
  the trash. On IMAP, neomail finds the folder from the special-use attributes in the server's LIST
  answer (`\Archive`, else `\All` such as Gmail's All Mail, and `\Trash`); an account without one
  fails and nothing moves. The server moves the message first, then it leaves the cache, so
  `neomail list` no longer shows it. Like `mark`, it needs the server, works across accounts and
  waits for a sync of the same account. Outlook archives to the folder Outlook's own Archive uses
  and deletes to Deleted Items. Where each puts a message:
  [docs/gmail.md](docs/gmail.md#archive-and-delete) and
  [docs/outlook.md](docs/outlook.md#where-archive-and-delete-put-a-message).
- `neomail folder list [--account NAME]` lists each account's folders from its server, online:
  the name `move` takes, the account, and the role (inbox, archive, trash, junk, sent, drafts,
  all, flagged, or none) from the server's special-use attributes on IMAP, and from Graph's
  well-known folder names on Outlook ([docs/outlook.md](docs/outlook.md#folders-and-move)). A
  folder that cannot hold
  messages is left out. An account that fails is one `neomail:` line after the others, and the
  exit status is 1.
- `neomail move FOLDER ID...` moves messages from the INBOX to the folder named FOLDER in each
  message's own account, exactly as `folder list` prints it (on IMAP the server's own hierarchy
  delimiter: `Folders/Receipts` on Proton Mail Bridge, `[Gmail]/Spam` or a label on Gmail). It works
  as `archive` does: server first, then the cache, across accounts, nothing queued. INBOX is
  refused, and so is a folder that cannot hold messages; an account without a folder of that name
  moves none of its messages and says to run `neomail folder list --account NAME`. neomail never
  creates, renames or deletes a folder. On Outlook a folder's name is its path of display names
  (`Inbox/Receipts`): [docs/outlook.md](docs/outlook.md#folders-and-move). What a move does on
  Gmail: [docs/gmail.md](docs/gmail.md#move-to-a-folder-or-label).
- `neomail send --to ADDR --subject TEXT --body PATH|-` sends a message written in Markdown: the
  HTML rendered from it, with the Markdown itself as the plain text part (`--text-only` sends that
  alone). HTML typed into the Markdown is sent as text, never as HTML. `--to`, `--cc` and `--bcc`
  repeat or take commas (a comma inside a quoted name, as in `"Doe, Jane" <jane@example.com>`,
  does not split); Bcc recipients get the message but no header names them. `--from` picks the
  account by its name or address; with one account it is that one. When standard input is a
  terminal neomail shows who it goes to and asks; when it is not (for example a body piped to
  `--body -`), pass `--yes`, or nothing is sent (exit 2). `--dry-run` prints the message, and on
  standard error who it goes to, Bcc included; it connects to nothing. If the server refuses any
  recipient, nobody gets it (exit 1). If the connection is lost after the whole message went
  out, it also exits 1, but the error says the message may have been sent: check Sent before
  sending it again. Gmail, Proton Mail Bridge and Outlook keep the copy in Sent themselves; for
  any other server neomail appends one to the mailbox marked `\Sent`, and a copy that fails is a
  warning, not a failure, so a retry never sends it twice. `--plain` prints the Message-ID alone
  even on a terminal. Attachments come later.
- `neomail reply ID [--all] --body PATH|-` answers a cached message from the account it is in:
  to its Reply-To, else its From (its To when the original is yours), and with `--all` its To
  and Cc too, never you. If any of those addresses cannot be sent to yet (it is not ASCII),
  nothing is sent (exit 1); no one is left out quietly. The subject gets `Re: `, your text
  goes above the original quoted with `> `, and In-Reply-To and References thread it (RFC 5322
  section 3.6.4). `neomail forward ID --to ADDR [--body PATH|-]` sends the original's headers and
  text below your note, with `Fwd: `; it warns that the original's attachments are not forwarded
  yet. An Outlook account added before neomail could send needs `neomail account login NAME`
  once ([docs/outlook.md](docs/outlook.md#accounts-added-before-neomail-could-send)). No Outlook
  account can send Bcc yet.
- `--json` and `--jsonl` print JSON for scripts; see [docs/json.md](docs/json.md) for the shapes,
  the plain columns and the exit codes.

On a terminal, listings are aligned columns. In a pipe, or with `--plain`, they are tab-separated:
one message per line, the id first, nothing cut off. `show -`, `mark read|unread -`, `archive -`,
`delete -` and `move FOLDER -` read ids from standard input:

```sh
neomail list | fzf -m | neomail show -                          # pick messages, read them
neomail list | fzf -m | neomail mark read -                     # pick messages, mark them read
neomail list | fzf -m | neomail archive -                       # pick messages, archive them
neomail list | fzf -m | neomail move Receipts -                 # pick messages, move them to Receipts
neomail list --color always | fzf --ansi -m | neomail show -    # the same, in colour
neomail list | cut -f1,6                                        # id and subject
neomail list --jsonl | jq -ac 'select(.unread) | .subject'      # unread subjects, ASCII-escaped
```

Colour follows `--color auto|always|never`, then `NO_COLOR` (no styling at all), then
`CLICOLOR_FORCE` and `CLICOLOR`. JSON and `--raw` output are never coloured. Help follows the
same rules, and on a terminal the top-level help (`neomail --help`, `neomail help`, bare
`neomail`) opens with the logo, the mark alone on a terminal narrower than 59 columns, and no
logo below 21.

Shell completions:

```sh
neomail completions bash > ~/.local/share/bash-completion/completions/neomail
neomail completions zsh > ~/.zfunc/_neomail    # with fpath+=(~/.zfunc) before compinit in ~/.zshrc
neomail completions fish > ~/.config/fish/completions/neomail.fish
```

### Background sync

`neomail daemon` syncs every account now and then every 5 minutes (`--interval 1m`, from 30s to
24h) until Ctrl-C, printing a line only when an account changed or failed, or its push state
changed. `neomail sync` still works alongside it. On Linux it shows a desktop notification when
new mail arrives, naming the sender and subject (`--no-notify` turns them off). IMAP accounts
(Gmail and Proton included) get push through IDLE: their new mail is synced seconds after it
arrives. Outlook accounts are checked every minute through Graph's delta query, since Graph has no
push for a program on your computer. To run it as a systemd, OpenRC or runit user service, and for
what its output means, see [docs/daemon.md](docs/daemon.md).

The daemon classifies nothing unless you switch it on with `daemon = true` under `[classify]` in
the config ([docs/daemon.md](docs/daemon.md#classifying)). Once it is on, and you have agreed to
classification through `neomail classify` and stored a key, it classifies new mail as it
arrives, with the same request, at most 100 messages each time it syncs (a round, a push sync or
an Outlook check), so `neomail list --group` stays current. It never asks for consent itself, and
leaves an account's first sync to `neomail classify`. After classifying it writes each account's
groups to its server as categories or labels and moves confident spam to Junk, at most 100 per
account each time ([docs/daemon.md](docs/daemon.md#groups-on-the-server)); `classify.labels =
false` and `classify.junk = false` turn those off. While the switch is off it writes and moves
nothing either.

How it fits together: [docs/architecture.md](docs/architecture.md).

## Planned features

- **Modal, vim-style UI** with mouse support and on-screen keybinding hints.
- **Outlook, Gmail and Proton Mail**, with secure OAuth sign-in and tokens kept in the OS keyring.
- **Local SQLite cache** for instant search, sorting and offline reading.
- **AI classification into groups**, stored as Outlook categories, Gmail labels or Proton labels.
- **Full-featured CLI and scripting**, so agents and scripts can do anything the TUI can.
- **Themes and configurable keybindings.**
- **XDG config locations.**

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
