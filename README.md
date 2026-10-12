<h1>
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/logo/neomail-logo-dark.svg">
    <img alt="neomail" src="assets/logo/neomail-logo-light.svg" height="48">
  </picture>
</h1>

A fast, modal terminal email client for Outlook, Gmail and Proton Mail.

> **Status:** early development. What works today: a read-only INBOX over IMAP in the command
> line (`nmail sync`, `nmail list`, `nmail show`), with passwords (an app password, Proton Mail
> Bridge) or, for Gmail, OAuth sign-in through your own Google client. Nothing changes on the
> server. There is no TUI yet, and no Outlook sign-in.

## Try it

### Install

From source, with Rust 1.88 or newer:

```sh
cargo install --locked --git https://github.com/AH-Merii/neomail nmail
```

Or download a build of the latest `main` from the
[edge pre-release](https://github.com/AH-Merii/neomail/releases/tag/edge) (Linux x86_64, a
static binary that needs no particular glibc, and macOS on Apple silicon), unpack it, and put
`nmail` on your `PATH`. Edge is rebuilt on every merge, so expect changes. Each tarball carries
the licences of the crates built into it in `THIRD-PARTY-LICENSES`.

### Configure

Create `~/.config/neomail/config.toml` (or `$XDG_CONFIG_HOME/neomail/config.toml`; `--config PATH`
or `NMAIL_CONFIG` point elsewhere, and an empty `NMAIL_CONFIG` counts as unset):

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
  only that exact certificate is accepted. Get it with
  `openssl s_client -connect 127.0.0.1:1143 -starttls imap </dev/null | openssl x509 -noout -fingerprint -sha256`.

### Passwords

An account without `password_command` reads its password from the OS keyring: the Secret Service
on Linux (GNOME Keyring, KeePassXC and others), the login keychain on macOS. Store it once:

```sh
nmail account set-password work      # prompts without echo
pass show mail/work | nmail account set-password work   # or one line on stdin
nmail account delete-password work
```

The entry has the service `neomail` and the account name as the user. An account with
`password_command` runs the command instead and never touches the keyring. Either way the password
is never written to the config, the cache or a log, and never printed.

- **WSL** often has no Secret Service. Use `password_command` (for example with `pass`), or run a
  Secret Service such as gnome-keyring.
- **Proton Mail:** store the password Bridge shows for the account, never your Proton account
  password.

### Gmail

Gmail signs in with OAuth through a Google OAuth client you create yourself; neomail ships none.
[docs/gmail.md](docs/gmail.md) walks through the five console steps and why each exists, and the
wizard prints them too:

```sh
nmail account add gmail          # asks for the address, client ID and secret, then opens the browser
nmail sync --account gmail
nmail account login gmail        # sign in again (needed every 7 days while the app is in Testing)
nmail account logout gmail       # revoke the sign-in and delete it from the keyring
```

The account is added to the config file without secrets; the client secret and refresh token stay
in the OS keyring (service `neomail-oauth`). `--no-browser` only prints the sign-in URL. On WSL the
Windows browser opens (`wslview` or `explorer.exe`), or the one in `$BROWSER`.

The cache lives in `~/.local/share/neomail/` (`$XDG_DATA_HOME/neomail/`, or `NMAIL_DATA_DIR`).

### Use

```sh
nmail sync                 # fetch new mail for every account: "work: 12 new, 1 updated, 0 removed"
nmail list                 # newest first; * marks unread
nmail show 42              # headers, body, attachments
```

- `nmail sync --account work --limit 100`: the first sync takes the 500 messages that arrived
  last unless `--limit` says otherwise. Later syncs fetch all mail that is new since, however
  much, refresh flags and drop deleted mail. Messages are fetched without marking them read.
- `nmail list --account work --limit 20 --json` and `nmail show 42 --json` print JSON for
  scripts; see [docs/json.md](docs/json.md) for the shapes and exit codes.
- `nmail show 42 --raw` prints the message source.

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
