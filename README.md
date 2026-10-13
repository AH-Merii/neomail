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
> marks mail read or unread, and `neomail archive` and `neomail delete` move mail out of the INBOX;
> otherwise nothing changes on the server. There is no TUI yet.

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
- **Gmail with an app password:** `neomail account add gmail-app-password`. Google: "To create an
  app password, you need 2-Step Verification on your Google Account"
  ([Google's help](https://support.google.com/accounts/answer/185833)).
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
imap = { host = "imap.mail.example", port = 993, security = "tls" }

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
[docs/gmail.md](docs/gmail.md) walks through the five console steps and why each exists, and the
wizard prints them too:

```sh
neomail account add gmail          # asks for the address, client ID and secret, then opens the browser
neomail sync --account gmail
neomail account login gmail        # sign in again (needed every 7 days while the app is in Testing)
neomail account logout gmail       # revoke the sign-in and delete it from the keyring
```

The account is added to the config file without secrets; the client secret and refresh token stay
in the OS keyring (service `neomail-oauth`). `--no-browser` only prints the sign-in URL. On WSL the
Windows browser opens (`wslview` or `explorer.exe`), or the one in `$BROWSER`.

### Outlook

Outlook, Outlook.com and Hotmail sign in with OAuth through a Microsoft Entra app registration you
create yourself, and mail is read through Microsoft Graph. [docs/outlook.md](docs/outlook.md) walks
through the five steps and why each exists; the wizard prints them too:

```sh
neomail account add outlook        # asks for the address and the Application (client) ID, then signs in
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
neomail mark read 42 43      # mark read on the server, then in the cache
neomail archive 42           # move to the archive on the server, then drop from the cache
neomail delete 43            # move to the trash; nothing is deleted for good
```

- `neomail sync --account work --limit 100`: the first sync takes the 500 messages that arrived
  last unless `--limit` says otherwise. Later syncs fetch all mail that is new since, however
  much, refresh flags and drop deleted mail. Messages are fetched without marking them read.
- `neomail list --account work --limit all` lists every cached message; `--limit` takes a number
  or `all` on `list` and `sync`.
- `neomail show 42 43` shows several messages; `neomail show 42 --raw` prints one message's source.
  On a terminal `show` goes through `$NEOMAIL_PAGER`, `$PAGER` or `less`; `--no-pager` turns it off.
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
- `--json` and `--jsonl` print JSON for scripts; see [docs/json.md](docs/json.md) for the shapes,
  the plain columns and the exit codes.

On a terminal, listings are aligned columns. In a pipe, or with `--plain`, they are tab-separated:
one message per line, the id first, nothing cut off. `show -`, `mark read|unread -`, `archive -`
and `delete -` read ids from standard input:

```sh
neomail list | fzf -m | neomail show -                          # pick messages, read them
neomail list | fzf -m | neomail mark read -                     # pick messages, mark them read
neomail list | fzf -m | neomail archive -                       # pick messages, archive them
neomail list --color always | fzf --ansi -m | neomail show -    # the same, in colour
neomail list | cut -f1,6                                        # id and subject
neomail list --jsonl | jq -ac 'select(.unread) | .subject'      # unread subjects, ASCII-escaped
```

Colour follows `--color auto|always|never`, then `NO_COLOR` (no styling at all), then
`CLICOLOR_FORCE` and `CLICOLOR`. JSON and `--raw` output are never coloured.

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
