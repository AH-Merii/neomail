# neomail — product decisions

The source of truth for what neomail must do. Decisions are made by the project owner; agents
implement them. The bullets below say what is decided now; keep related decisions together.

Bullets that link to an entry in the [decision log](#decision-log) were added with the log. The
entry gives the ruling an ID (`DEC-YYYY-MM-DD-slug`), the owner's own words and a status. Bullets
without a link predate the log. Docs cite the ID instead of copying the ruling. Log entries are
append-only: a change is a new entry that supersedes the old one, and the bullet is updated to link
the new entry.

## Identity

- **Name:** neomail (inspired by Neovim). **Binary:** `neomail`
  ([DEC-2026-10-12-binary-neomail](#dec-2026-10-12-binary-neomail)).
- **What it is:** a fast, lightweight, secure terminal email client (TUI + CLI) for viewing, sorting,
  filtering, grouping, sending, archiving and deleting mail.
- **Language:** Rust.
- **Repository:** <https://github.com/AH-Merii/neomail> (public). Dual-licensed MIT OR Apache-2.0.

## Accounts and providers

- **Providers from day one:** Outlook (incl. Outlook.com / Hotmail personal accounts), Gmail, and
  Proton Mail.
- **Multiple accounts, managed together.** Any number of accounts across providers in one neomail:
  unified views (e.g. one inbox / one group across all accounts), per-account views, cross-account
  search, choosing the sending identity, and per-account settings.
- **No built-in OAuth client IDs (owner decision, liability).** neomail ships with *no* Google or
  Microsoft client registrations. Each user registers their own OAuth app ("bring your own client"),
  guided by an in-app wizard (`neomail account add`) and step-by-step docs that explain *why* each
  step exists. Never embed or borrow another application's client ID (e.g. Thunderbird's).
- **Setup wizards** ([DEC-2026-10-13-wizards](#dec-2026-10-13-wizards)): `neomail account add`
  takes the user through setup one step at a time for Gmail (their own OAuth client, or an app
  password), Outlook and Proton Mail. Other IMAP servers get no wizard and keep today's
  questions. A wizard may check a client ID or secret with Google or Microsoft before the
  sign-in, once a live check has shown that their answers tell a wrong one apart. How the steps
  work is proposed in [DEC-2026-10-13-wizard-steps](#dec-2026-10-13-wizard-steps).
- **Gmail sign-in modes:** (1) the user's own Google OAuth client, (2) app password over
  IMAP/SMTP (requires Google 2-Step Verification). Both read mail over IMAP, with IDLE push. A
  slice that needs what IMAP can't do adds the Gmail API for that part only, in an entry of its
  own ([DEC-2026-10-12-gmail-imap](#dec-2026-10-12-gmail-imap)).
- **Outlook:** the user's own Microsoft Entra app registration (multi-tenant + personal accounts,
  public client, `http://localhost` redirect, public client flows enabled for device code).
  Microsoft Graph is the primary API (categories, delta sync, send). IMAP/SMTP is an optional
  fallback. Note for docs: personal Microsoft accounts cannot register apps without a directory —
  the user must sign up for an Azure (free) account to get one (card required for identity
  verification only). The registration survives cancelling the Azure subscription
  ([DEC-2026-10-12-outlook-defaults](#dec-2026-10-12-outlook-defaults), proposed).
- **Proton:** via Proton Mail Bridge (local IMAP/SMTP; paid Proton plans only). Never ask for or
  store the Proton account password. No native Proton API client in v1.
- **Authentication must be OAuth where the provider supports it, and very secure:** PKCE + loopback
  redirect (device code flow for headless/SSH), tokens and passwords only in the OS keyring (or a
  user-provided password command) — never in config files, SQLite, logs, or command output.

## Classification ("jev")

- Emails are sorted into groups by **jev** — TypeSafe AI's "System One" decision model
  (`POST {base}/v1/systemone`, Bearer key). The backend is **pluggable**: any model that speaks
  jev-style (System One) input must work (e.g. Cloudflare Clef, Ollama `/v1/systemone`, Kev,
  OpenRouter, Liquid d1; adapters for near-compatible APIs).
- **API key:** `JEV_API_KEY` environment variable as a fallback; the OS keyring is the primary store
  (service `neomail-jev`, one entry per backend host:
  [DEC-2026-10-13-classify](#dec-2026-10-13-classify), proposed). Users bring their own key;
  neomail never ships one.
- **Each email belongs to exactly one group.**
- **Default groups:**

| Group | Meaning |
|---|---|
| `main` | The main inbox: worth your attention but doesn't necessarily need a reply |
| `follow-up` | Needs follow-up |
| `follow-up-old` | Needs follow-up and is more than 3 months old |
| `recruitment` | Recruiting / hiring mail |
| `receipts-invoices` | Receipts and invoices |
| `orders-shipping` | Order confirmations, tracking |
| `newsletters` | Subscribed newsletters |
| `promotions` | Marketing and sales |
| `notifications` | Automated alerts and notices |
| `spam` | Spam |
| `other` | Fallback when jev is unsure |

- **`follow-up-old` is automatic by age:** jev only decides `follow-up`; neomail moves a `follow-up`
  email to `follow-up-old` once it is older than 3 months (threshold configurable).
- **`other`** is also where low-confidence classifications land (confidence threshold configurable).
- **Spam:** always tagged `spam`; additionally moved to the provider's Junk folder only when jev's
  confidence is high (default ≥ 0.9, configurable). Lower-confidence spam stays in place, tagged.
- **Where groups are stored:** as provider-native tags — Outlook categories, Gmail labels, Proton
  labels (`Labels/<name>` over Bridge IMAP). Mail stays where it is; nothing gets moved (the spam
  rule above is the only exception).
- **Groups as Outlook categories** ([DEC-2026-10-13-categories](#dec-2026-10-13-categories),
  proposed): the label prefix and the group name, written by `neomail classify` and the daemon,
  never touching the user's own categories or asking for a new permission.
- **Groups as Gmail and Proton labels, and spam to Junk**
  ([DEC-2026-10-14-labels-junk](#dec-2026-10-14-labels-junk), proposed): Gmail labels over IMAP
  and Proton labels over Bridge, never touching the user's own labels; confident spam moves to the
  account's Junk folder once, and a message the user moves back stays.
- **Each email is classified once** (results cached locally with model id, probabilities and
  timestamp so thresholds can be re-tuned without re-calling the model).
- **Classify only when asked**
  ([DEC-2026-10-14-classify-when-asked](#dec-2026-10-14-classify-when-asked)): mail goes to the
  classifier only when the user asks for it, by running `neomail classify`, by a yes to a question
  neomail asks first, or through the daemon once background classifying is switched on.
  `neomail classify` takes cached mail from the last 365 days, newest first, at most 1,000
  messages (`--days` and `--limit` change both), and estimates what the rest would cost (count,
  requests, time, money); `--all` does the rest.
- **Redaction uses our own rules** ([DEC-2026-10-11-pii-rules](#dec-2026-10-11-pii-rules)): the PII
  redaction that runs before text goes to jev is neomail's own rule redactor. No model is part of
  the default.
- **The redaction preview** is `neomail redact ID...`: the subject and the first 1,500 characters
  of the body, redacted as they will go to jev, offline
  ([DEC-2026-10-13-redact](#dec-2026-10-13-redact), proposed).
- **Redaction aims at common mail** ([DEC-2026-10-13-redaction-aim](#dec-2026-10-13-redaction-aim)):
  the rules cover the mail people most commonly get, not every possible input. A gap that only a
  contrived input shows is no reason for another round of fixes.
- **The sender's domain is sent** with each message's redacted subject and body excerpt, always
  ([DEC-2026-10-13-classify-send](#dec-2026-10-13-classify-send)).
- **Consent per backend host** ([DEC-2026-10-13-classify-send](#dec-2026-10-13-classify-send)):
  before the first request to a host, `neomail classify` says on a terminal what is sent and
  where, and asks once. The answer is remembered per host; a script consents with `--yes`. Without
  consent, nothing is sent.
- **The live check of classification** sends at most 20 jev requests, made-up messages only, each
  request recorded in a spend ledger before it is sent
  ([DEC-2026-10-13-classify-send](#dec-2026-10-13-classify-send)).
- **Classifying from the CLI** is `neomail classify`, the first of slice 10's four parts; results are
  stored per message and survive a mailbox rebuild
  ([DEC-2026-10-13-classify](#dec-2026-10-13-classify), proposed).
- **Group labels** ([DEC-2026-10-13-labels](#dec-2026-10-13-labels)): a label neomail creates for a
  group is the group's name after a prefix, `nm-` by default and configurable
  (`nm-receipts-invoices`). neomail never shows the prefix. Slice 10d's live check may create
  these labels on the test accounts and keeps them.
- **The daemon classifies new mail** as it syncs, only after the user switches it on with
  `classify.daemon = true`. It is off by default, and no answer to a question switches it on
  ([DEC-2026-10-14-classify-when-asked](#dec-2026-10-14-classify-when-asked)). It also needs a
  key and the user's consent for the backend host. Mail from a first sync is left to
  `neomail classify` ([DEC-2026-10-13-daemon-classify](#dec-2026-10-13-daemon-classify), proposed).

## Data and storage

- **Local SQLite cache with sync:** sorting, filtering and grouping are instant, mail is readable
  offline, and each email is classified once.
- **Config file in XDG locations** (`$XDG_CONFIG_HOME`, `$XDG_DATA_HOME`, `$XDG_CACHE_HOME`,
  `$XDG_STATE_HOME`, …).

## UX

- **Mouse support.**
- **Modal, vim-style controls** for navigating *and* writing. Modes wherever they make sense; it
  must feel intuitive.
- **Keybinding hints** on screen without taking much space; each mode shows only its own bindings.
- **Keybindings are configurable and easy to learn**, with a quick way to look them up and change
  them.
- **Theming** (the TUI; CLI output follows the terminal's own palette:
  [DEC-2026-10-12-cli-ux](#dec-2026-10-12-cli-ux)).
- **Live sorting animation:** when jev sorts/tags mail in real time, the TUI plays a really cool
  animation that shows each message being sorted into its group. On by default; can be turned off
  in the configuration. It must degrade gracefully (reduced motion, slow terminals or SSH, small
  windows) and never delay the actual sorting.
  - **Chosen style (owner, 2026-10-10):** *Decode*, combined with *Scan & stamp*'s confidence bar.
    - While jev works, the message decodes: a short scramble sweeps the subject.
    - The group tag then resolves left to right in the group's colour.
    - The confidence bar fills in alongside it.
  - The other prototyped concepts (Sorting machine, Scan & stamp) are not required for v1. The
    animation-style seam keeps them possible later. Prototypes: PR #1, `prototypes/sort-animation`.
- **Refined motion** ([DEC-2026-10-10-motion](#dec-2026-10-10-motion)): small, tasteful animations
  and transitions across the whole experience. The design is delegated: there are no mock-ups for
  the owner to choose from.
- **Desktop notifications for new mail**
  ([DEC-2026-10-12-daemon-notifications](#dec-2026-10-12-daemon-notifications)): `neomail daemon`
  shows new mail as desktop notifications (freedesktop notifications over D-Bus on Linux), one per
  account per round, with up to three messages as "Sender: Subject" and then "and N more". They are
  on when a notification service is running, and can be turned off.
- **Thoughtful sound** ([DEC-2026-10-11-sound](#dec-2026-10-11-sound)): a few tasteful sounds for
  meaningful moments, such as sending an email or receiving a notification. Never on navigation or
  other frequent actions.
- **Drafting feels like Neovim.**
  - Full modal editing with motions, counts and text objects.
  - Absolute, relative and hybrid line numbers (`number` / `relativenumber`), so jumps like `5j`, `12k` and `3dd` are quick.
  - `$EDITOR` remains an option.
- **Markdown drafts:**
  - Write in Markdown with live, incremental syntax highlighting. Quotes, signatures, links and addresses are highlighted too.
  - Drafts are sent as `multipart/alternative`: the rendered HTML, plus the Markdown source as the plain-text part.
  - A per-message toggle sends plain text only.
- **Spelling and grammar checks, fully offline:**
  - Dictionary spellcheck for any language, plus English grammar hints. Nothing leaves the machine.
  - Problems appear as diagnostics, with an underline and a sign. Navigate them like Neovim: `]s` / `[s` to jump, `z=` for suggestions, `zg` to add a word.
  - A floating window explains the problem under the cursor. Completion popups (addresses, words) and hover info use the same floating-window mechanism.

## Search and pickers

- **Fuzzy search is a core, extensible building block**, inspired by snacks.nvim's picker, Telescope and Helix.
  - A picker is a *source* (what to list), a *scope* (where to look), a fast matcher, a preview and actions.
  - New pickers are added by registering a source, from Rust or Lua. Agents and the CLI can query the same sources.
  - This mirrors how Neovim extends search to files, grep, keymaps, diagnostics and more.
- **Scopes, like buffer vs project in Neovim:** the current draft or message, then the thread, the folder or group, one account (e.g. only Outlook), and everything. You can switch scope inside the picker without retyping the query.
- **First pickers:**
  - search inside the draft being written;
  - live text search over all mail, returning the matching emails;
  - senders, ranked by frecency.
- **Next pickers:** subjects, folders/labels/groups, accounts, keymaps, actions/commands, errors/log, themes, drafts and attachments.
- **Very fast:**
  - Results start streaming within a frame of typing, even at 100k+ messages per account.
  - Matching is incremental and multi-threaded, and never blocks the UI. Study how Helix's nucleo matcher achieves this.

## Images and attachments

- **Images in messages render in the terminal.** Embedded images (logos, screenshots, inline `cid:` parts) are shown inline.
  - They use the terminal's graphics protocol (kitty graphics, Sixel, iTerm2), falling back to coloured half-blocks.
  - Decoding uses memory-safe decoders with size limits, never blocks the UI, and can be switched off.
  - *Provisional (Claude's default, owner to confirm):* on by default.
- **Remote images are blocked by default.** This is for privacy: tracking pixels, read receipts and IP leaks.
  - One key loads them for this message, or always for this sender.
  - The daemon fetches them without cookies or referrer, with size and time limits. Known tracking pixels are never loaded.
- **Open the full message in a browser** when a newsletter needs it. The browser gets a sanitised copy, with remote content still blocked unless allowed.
- **Attachments:**
  - Each is listed with its name, its real type and its size. The type is detected from the content, with a warning if it doesn't match the extension.
  - Attachments download on demand and are cached.
  - Save one or all, to the downloads folder (configurable) or a chosen path.
  - Open with the system's default app. This never happens automatically, and executables and scripts get a warning.
  - Preview in the terminal where possible: images, text, PDF text.
  - Filenames are sanitised: no path traversal, and no control or bidirectional characters. Saved files are never executable, and macOS quarantine flags are kept.
- **Attachments are searchable.**
  - An attachments picker searches all accounts by name, type, sender and date.
  - The CLI and agents get the same commands (`neomail attachment list|save|open --json`). Agents read attachment text as data, never as instructions.
- **Attachments in the CLI** are `neomail attachment list|save|open`, read from the cached message offline, with real types, safe names and a question before an executable opens ([DEC-2026-10-14-attachments](#dec-2026-10-14-attachments), proposed).
- **Drafting with files and images:**
  - Attach with `:attach` and a fuzzy file picker, by pasting or dropping a file path, or by pasting an image from the clipboard.
  - Markdown images (`![alt](path)`) are sent as inline images. Per-identity signatures can include a logo. The composer previews images inline.
  - Before sending, neomail warns about a forgotten attachment (the draft says "attached" but has none) and about provider size limits.
  - Large files use the provider's upload sessions.

## CLI, scripting, agents, extensibility

- **The CLI can do everything the TUI can**, modelled on Superlogical's Rex
  ("everything you can do in the GUI, you can do via the CLI"): one typed action registry underneath
  TUI keys, CLI commands, scripts and agents; `--json` everywhere; documented exit codes; an event
  stream.
- **Easy scripting and scheduled tasks.**
- **The CLI for people and pipes** ([DEC-2026-10-12-cli-ux](#dec-2026-10-12-cli-ux)):
  - Getting started: bare `neomail` with no config prints a few start lines and a guide URL, and
    never prompts; the TUI, once it lands, opens its add-account screen instead. Every "no config"
    message names the next command.
  - Setup is `neomail account add`, the in-app wizard: a provider menu on a terminal, a flag for
    every prompt except secrets (they come from a prompt without echo, stdin or a password
    command), `--no-input` for scripts, and a read-only login before the config is written
    (`--no-check` skips it). `neomail account list` and `neomail config path|check` show and check
    the config. No server autodiscovery for IMAP for now.
  - Listings: human on a terminal; plain TSV (one record per line, id first, no truncation) when
    stdout is not a terminal or with `--plain`; `--json` and `--jsonl` for data. stdout carries
    only results; prompts, progress and errors go to stderr. A broken pipe exits 0 silently.
  - Colour from the 16 ANSI colours, so the terminal theme picks the shades: on a terminal by
    default, following `--color auto|always|never`, NO_COLOR and CLICOLOR. `--color always` also
    colours plain output, for `fzf --ansi`; nothing ever colours `--json`, `--jsonl` or `--raw`.
    NO_COLOR removes all styling, bold and underline included: stricter than no-color.org, which
    covers colour only, so one rule holds. Colour never carries meaning alone.
- **Marking read and unread** is `neomail mark read|unread`: online only, with the output rules
  above ([DEC-2026-10-13-mark](#dec-2026-10-13-mark), proposed).
- **Archiving and deleting** is `neomail archive` and `neomail delete`: a move to the account's
  archive or trash, never a deletion for good, online only
  ([DEC-2026-10-13-archive-delete](#dec-2026-10-13-archive-delete), proposed).
- **Moving to a folder** is `neomail move FOLDER ID...`, with a name exactly as
  `neomail folder list` prints it: never a folder created, online only
  ([DEC-2026-10-13-move](#dec-2026-10-13-move), proposed).
- **Sending from the CLI** is `neomail send`, `neomail reply ID` and `neomail forward ID`, with a
  Markdown body from a file or standard input, asked before sending on a terminal
  ([DEC-2026-10-13-send](#dec-2026-10-13-send), proposed).
- **Easy to extend** with new functionality and UI later.
- **Agentically customizable, like Pi** (Mario Zechner's agent harness): agents can extend and
  customize neomail themselves (extensions, docs written for agents, hot reload).
- **Ask an agent about your email / have an agent act on it:** the CLI/API must be easy for AI agents
  to drive (machine-readable output, self-describing schema, agent skill/docs, MCP).
- **Extension language: Lua** (embedded, Neovim/Rex style) for custom actions, hooks and UI.
  Plain settings live in a TOML config file.
- **Building blocks first (the owner's design principle).** Design neomail the way Neovim and Helix are designed: a small set of fast, general primitives, with every feature built from them. The primitives:
  - **buffers:** every view is a buffer (message list, message, thread, draft, picker, help, log);
  - **windows**, including floating windows;
  - **decorations:** highlights, virtual text, signs;
  - **events** (autocommands);
  - **the action registry**;
  - **keymaps** per mode and per buffer;
  - **pluggable providers:** picker sources, completion sources, diagnostics, previewers, animation styles.

  Extensions use exactly the same primitives as built-in features. The performance of these primitives is a hard requirement; everything else follows from them.

## Platforms

- **First release: Linux, macOS, and WSL (WSL2)** — CI, prebuilt binaries, OS keyring on Linux and
  macOS. Native Windows later.
- **Linux first** ([DEC-2026-10-12-linux-first](#dec-2026-10-12-linux-first)): for now all work
  targets Linux; the first goal is neomail working there. macOS stays in the first release, but no
  slice waits on it and a macOS-only CI failure doesn't block a merge.
- **More of Linux** ([DEC-2026-10-12-daemon-notifications](#dec-2026-10-12-daemon-notifications)):
  edge builds for aarch64 Linux as well as x86_64, and a daemon guide for OpenRC and runit beside
  systemd.
- **WSL specifics are first-class:** detect WSL (`WSL_DISTRO_NAME` / WSLInterop) and handle it —
  secrets fall back when no Secret Service is available (password command such as `pass`, Windows
  Credential Manager via interop, or an encrypted file store), OAuth opens the Windows browser
  (`wslview` / `explorer.exe`) while the loopback redirect still works via WSL2 localhost
  forwarding, and clipboard/notifications go through Windows interop (or OSC 52).

## Project and delivery

- **GitHub repo** (public) managed with the **GitButler CLI (`but`)** so parallel agents don't step on
  each other's toes.
- **CI/CD** for builds and tests.
- **Clear, simple installation** ([DEC-2026-10-12-install-channels](#dec-2026-10-12-install-channels)):
  an `install.sh` one-liner and mise from the `edge` pre-release for now; Homebrew and the AUR
  later.
- **A simple website with docs hosted on GitHub Pages**, including a first-time setup guide
  (why and how to register OAuth apps, Proton Bridge, app passwords, keyring).
- **A neomail logo** ([DEC-2026-10-11-logo](#dec-2026-10-11-logo)): variation 05, "Split and
  statusline", in `assets/logo/`.
- **Help in colour, with the logo** ([DEC-2026-10-14-help-logo](#dec-2026-10-14-help-logo)):
  help follows the same colour rules as the rest of neomail's output. The top-level help
  (`neomail --help`, `neomail help`, bare `neomail`) opens with the terminal lockup of
  `assets/logo/logo.txt` above the help, on a terminal only: piped help has no logo, and neither
  has a command's own help. A terminal narrower than the lockup gets the mark alone, never a
  wrapped logo.
- **Agent kit** ([DEC-2026-10-11-agent-kit](#dec-2026-10-11-agent-kit)): paused by
  [DEC-2026-10-11-build-first](#dec-2026-10-11-build-first). When it resumes, the workflow helpers
  (the script linter, the wait helpers, run continuation and the rest) become a project-agnostic
  Claude Code plugin with nothing specific to neomail, kept in the owner's public plugin
  marketplace, AH-Merii/claude-plugins. Its skill carries only the few practices that give most of
  the gains ([DEC-2026-10-11-agent-kit-skill](#dec-2026-10-11-agent-kit-skill)).
- **Autonomy:** agents work on GitButler branches; Claude creates, updates and merges PRs without
  asking ([DEC-2026-10-11-pr-autonomy](#dec-2026-10-11-pr-autonomy)); changes are merged to `main`
  once the CI run passes (a macOS-only failure leaves it green:
  [DEC-2026-10-12-linux-first](#dec-2026-10-12-linux-first)) and a review agent has passed them
  ([DEC-2026-10-11-merge-gate](#dec-2026-10-11-merge-gate), proposed); the docs site is published to GitHub Pages; pre-release builds may be cut without
  asking.
- Commits in this repository are unsigned (local `commit.gpgsign=false`) while agents work
  unattended.
- **`main` is always usable** ([DEC-2026-10-11-main-usable](#dec-2026-10-11-main-usable)): every
  merge to `main` builds and publishes `neomail`, so whatever has landed can be tried while the rest
  is still being built. A merged CLI command works before the TUI exists, and independent features
  become usable as each one merges.
- **Build first** ([DEC-2026-10-11-build-first](#dec-2026-10-11-build-first)): neomail grows in
  thin slices, each usable on its own and merged to `main` when it works. The first slice is a
  read-only inbox in the CLI: `neomail` syncs one account's inbox, lists its messages and shows one.
  Design docs stay short and follow the code. A review runs at most two rounds, and what is left
  becomes an issue. The agent kit and the docs lint wait until the app needs them.
- **CLI first** ([DEC-2026-10-11-cli-first](#dec-2026-10-11-cli-first)): every CLI feature is
  built before the TUI starts. A feature with both a command and a screen ships its command first;
  the TUI then puts keys and screens on actions the CLI already has.
- **Live tests** ([DEC-2026-10-11-test-accounts](#dec-2026-10-11-test-accounts)): the three
  dedicated test accounts may send and receive freely. The owner's personal accounts are never used.
- **Architecture rules** ([DEC-2026-10-11-architecture-retro](#dec-2026-10-11-architecture-retro)),
  applied to code as it lands ([DEC-2026-10-11-build-first](#dec-2026-10-11-build-first)):
  - each slice lists the seams it touches before it is built;
  - a capability with two or more consumers, or a realistic alternative, gets a port with a contract
    and a conformance suite;
  - each cross-cutting concern has one owner section, which others link to and never redefine;
  - invariants live in types, not comments;
  - reviews are adversarial and stop after two rounds;
  - when requirements change, the affected sections are re-derived, not patched;
  - a walking skeleton comes early: the first slice is one;
  - the machine-checkable architecture (a manifest, generated tables and a docs lint) waits until
    there is code for it to check.
- **Benchmarks** ([DEC-2026-10-11-bench-sample](#dec-2026-10-11-bench-sample)): a benchmark runs
  on a representative sample, just large enough to measure the metrics, never on whole datasets.
  The choice weighs speed against accuracy.

## Decision log

Statuses:

- **proposed:** made on the owner's behalf while they were away; followed until the owner accepts
  or rejects it.
- **recorded:** the owner ruled; the bullet the entry names states the ruling.
- **paused:** recorded, but on hold until the entry that paused it is superseded.
- **superseded** or **rejected.**

Never edit an entry's text. To change a ruling, add an entry that supersedes it.

### <a id="dec-2026-10-11-first-run"></a>DEC-2026-10-11-first-run: classify the last year on first run

- **Status:** superseded by [DEC-2026-10-14-classify-when-asked](#dec-2026-10-14-classify-when-asked).
  **Source:** owner, chat, 2026-10-11.
- **Supersedes:** `PROV-07`, the plan's provisional default P7, which asked before classifying more
  than 5,000 messages.
- **Owner's words:** "yes do the first 365 automatically and the limit is capped to the first 1000
  emails. with an estimate on how much it would cost to do all of them."
- **Ruling:** the "First run" bullet under [Classification](#classification-jev).
- **Retires:** the key `classify.ask_above`, the value "5,000 messages", and the phrases "asks
  above" and "cost confirmation above".

### <a id="dec-2026-10-11-main-usable"></a>DEC-2026-10-11-main-usable: `main` is always usable

- **Status:** recorded. **Source:** owner, chat, 2026-10-11.
- **Owner's words:** "we should have ci/cd nmail always gets built on merge to main that makes it
  to se we can try it while it is being built. for example if the cli is built and the interface is
  not, we can still use the cli and do stuff with it, even for the cli, if we are developing
  parallel independant features we can use try them out when they get merged. think of how you
  would set up the cicd and versioning if you hvae not already thought of that"
- **Ruling:** the "`main` is always usable" bullet under [Project and delivery](#project-and-delivery).
  Its first CI/CD design, [DEC-2026-10-11-edge-channel](#dec-2026-10-11-edge-channel), was
  superseded by [DEC-2026-10-11-build-first](#dec-2026-10-11-build-first); CI/CD is now built with the first slice.

### <a id="dec-2026-10-11-test-accounts"></a>DEC-2026-10-11-test-accounts: test accounts send and receive freely

- **Status:** recorded. **Source:** owner, chat, 2026-10-11.
- **Supersedes:** the architecture brief's provisional D4 (sends only between test accounts, at
  most 3 per run), which was never recorded here.
- **Owner's words:** "As for the emails, they can send and recieve, we don't need to be worried
  since these are brand new emails and they have not been used for anything."
- **Ruling:** the "Live tests" bullet under [Project and delivery](#project-and-delivery).

### <a id="dec-2026-10-10-motion"></a>DEC-2026-10-10-motion: refined motion, design delegated

- **Status:** recorded. **Source:** owner, chat, 2026-10-10.
- **Owner's words:** "I also want us to think of the small animations here and there that add a
  touch of refinements to the entire experience. Some tasteful animations and transitions. I will
  leave that to you though, no need to mock and have me decide, just know that I am expecting a
  refined experience"
- **Ruling:** the "Refined motion" bullet under [UX](#ux).

### <a id="dec-2026-10-11-sound"></a>DEC-2026-10-11-sound: a few thoughtful sounds

- **Status:** recorded. **Source:** owner, chat, 2026-10-11.
- **Owner's words:** "also since we thought of the small animations it would also be useful to
  think of any sounds that go with them, I don't want everything to have a sound, but it would be
  nice to have sounds thoughtfully added, for example when sending an email, when receiving a
  notification, I would not want a sound for example on each navigation. thoughtful, tasteful, the
  small things that add je ne se quoi."
- **Ruling:** the "Thoughtful sound" bullet under [UX](#ux).

### <a id="dec-2026-10-11-consistency-process"></a>DEC-2026-10-11-consistency-process: how the design stays consistent

- **Status:** superseded by [DEC-2026-10-11-build-first](#dec-2026-10-11-build-first).
  **Source:** owner, chat, 2026-10-11, delegating the decision to Claude.
- **Owner's words:** "based on this how would avoid that from happening down the line? think of
  how you would structure your workflows to catch this or to routinely check I will leave the
  decision to you." (8e661b24:761)
- **Ruling:** a process in `docs/process/consistency/`, withdrawn before it merged.

### <a id="dec-2026-10-11-walking-skeleton"></a>DEC-2026-10-11-walking-skeleton: walking skeleton, then a thin slice

- **Status:** superseded by [DEC-2026-10-11-build-first](#dec-2026-10-11-build-first).
  Proposed by Claude on 2026-10-11 while the owner was hands-off. It was D1 in the architecture
  brief.
- **Proposal:**
  - Wave 0 ends with every crate compiling behind its real traits, and with the composition roots
    (`nm-daemon`, `nm-tui`, `nmail`) running end to end on stubs and fakes (`nmail --demo`, one
    action over IPC).
  - Work is then ordered by a thin-slice milestone, 0.1.0-alpha.1: read, search and triage
    (archive, delete, move, classify) real mail in the TUI from Proton (via Bridge) and Gmail.
  - Everything else follows that milestone.

### <a id="dec-2026-10-11-crate-reasons"></a>DEC-2026-10-11-crate-reasons: every crate has a reason

- **Status:** superseded by [DEC-2026-10-11-build-first](#dec-2026-10-11-build-first).
  Proposed by Claude on 2026-10-11. It was D2 in the architecture brief.
- **Proposal:** every crate states its reason in the manifest, from a fixed list:
  - an interface;
  - an adapter for a port with a realistic alternative;
  - isolation of a heavy or optional dependency;
  - a licence boundary;
  - a process or sandbox boundary;
  - a composition root;
  - test-only;
  - a work package that would otherwise be larger than XL.

  A crate with no reason merges into a neighbour. There is no target crate count.

### <a id="dec-2026-10-11-jev-offline"></a>DEC-2026-10-11-jev-offline: tests never call jev

- **Status:** proposed by Claude on 2026-10-11. It was D3 in the architecture brief.
- **Proposal:** CI and automated tests never call jev; they use fakes and recorded fixtures. Live
  calls happen only in gated live tests, capped at 20 requests per run and recorded in a spend
  ledger.

### <a id="dec-2026-10-11-merge-gate"></a>DEC-2026-10-11-merge-gate: how work merges

- **Status:** proposed by Claude on 2026-10-11. It was D5 in the architecture brief.
- **Proposal:** one PR per work package, squash-merged once CI is green and an independent review
  agent has passed it. There is no owner review gate.
- **Open question for the owner:** does this refine the "Autonomy" bullet (CI must pass, and a
  review agent must also pass) or replace it? Claude follows "refine" until the owner says
  otherwise.

### <a id="dec-2026-10-11-agent-send"></a>DEC-2026-10-11-agent-send: agents draft freely, send with approval

- **Status:** proposed by Claude on 2026-10-11.
- **Proposal:** agents may draft freely. This covers MCP clients and the CLI under an agent profile.
  Sending and deleting need the owner's approval for each action.
- **Why:** this settles a conflict in the draft architecture, which was not kept: its permission
  profiles for actions and its rules for agents disagreed.

### <a id="dec-2026-10-11-redaction-default"></a>DEC-2026-10-11-redaction-default: redact in the daemon; the benchmark picks the detectors

- **Status:** proposed by Claude on 2026-10-11.
- **Proposal:** text leaves the daemon, for jev or for MCP clients, only after redaction inside the
  daemon. `privacy.detectors` defaults to `["rules"]` until the PII benchmark reports, and the
  benchmark's result then sets the default.
- **Owner's words on the benchmark:** "you will benchmark it later and that will enforce your
  decision."

### <a id="dec-2026-10-11-edge-channel"></a>DEC-2026-10-11-edge-channel: the edge channel and versioning

- **Status:** superseded by [DEC-2026-10-11-build-first](#dec-2026-10-11-build-first).
  Proposed by Claude on 2026-10-11 as the concrete design for
  [DEC-2026-10-11-main-usable](#dec-2026-10-11-main-usable).
- **Proposal:**
  - The composition roots (`nmail`, `nm-daemon`, `nm-tui`) exist from wave 0 and call every
    crate's frozen constructor. A crate that is not built yet returns a typed
    `NotImplemented { wp }` error, so a merge lights up its feature without re-wiring.
  - Each crate reports its status: stub, partial or ready. `nmail features` lists what works, and
    `nmail --demo` runs on fixtures.
  - Every PR runs a smoke test of the binary. Every merge to `main` publishes a rolling `edge`
    pre-release, like Neovim's nightly.
  - Versions are `0.1.0-dev.N+gSHA` on edge, with `v0.1.0-alpha.N` tags at milestones. The storage
    schema and the IPC protocol carry their own version numbers.
  - Edge installs as `nmail-edge`, with its own data directory and keyring namespace, and can
    update itself. Tagged releases do not update themselves.
- **Refines:** [DEC-2026-10-11-walking-skeleton](#dec-2026-10-11-walking-skeleton) (D1 in the
  brief), which had stewards re-wire the roots at each phase.

### <a id="dec-2026-10-11-bench-sample"></a>DEC-2026-10-11-bench-sample: benchmarks decide on a representative sample

- **Status:** recorded. **Source:** owner, chat, 2026-10-11.
- **Context:** the PII benchmark had run every detector over the whole corpus (8,629 documents) for
  hours. The owner stopped it.
- **Owner's words:** "forget it, stop the tests for pii, you didn't have to run it on the entire
  dataset, we just needed a representitive set to get an idea and measure the metrics and compare
  speed vs accuracy metrics and decide base on the tradeoffs..."
- **Ruling:** the "Benchmarks" bullet under [Project and delivery](#project-and-delivery).

### <a id="dec-2026-10-11-logo"></a>DEC-2026-10-11-logo: the Split and statusline logo

- **Status:** recorded. **Source:** owner, chat, 2026-10-11.
- **Context:** six variations were drafted for PR #2 (`assets/logo/variations/`), next to the
  original cursor-n mark. The art director ranked 05 first.
- **Owner's words:** "i like the split and status line logo"
- **Ruling:** the "A neomail logo" bullet under [Project and delivery](#project-and-delivery):
  variation 05 replaces the original mark in `assets/logo/`.

### <a id="dec-2026-10-14-help-logo"></a>DEC-2026-10-14-help-logo: help in colour, opening with the logo

- **Status:** recorded. **Source:** owner, chat, 2026-10-13 and 2026-10-14 (f679d889:13187,
  f679d889:13226, f679d889:15885), for issue #64.
- **Owner's words:**
  - "create an issue to add colors to the help output in neomail"
  - "also have the neomail logo show as output when help is printed just like mise also has a
    logo, that is where I got the inspiration from mise --help"
  - Asked where the logo sits, with a preview of each layout: "Lockup above the help
    (Recommended)", over the mark beside a new intro paragraph as mise lays it out.
  - Asked whether help sent to a pipe or a file keeps the logo: "Terminal only (Recommended)".
- **Ruling:** the "Help in colour, with the logo" bullet under
  [Project and delivery](#project-and-delivery).

### <a id="dec-2026-10-11-architecture-retro"></a>DEC-2026-10-11-architecture-retro: rules against coupling and contradictions

- **Status:** recorded. **Source:** owner, chat, 2026-10-11.
- **Context:** the architecture had buried PII redaction inside `nm-classify` while MCP claimed to
  reuse it. Asked how to avoid such coupling and contradictions, Claude wrote a retro with seven
  rules, and the owner asked to keep them.
- **Owner's words:** "looking back in retrospective how would you avoid the "mistakes" you made when
  it comes to coupling or contradictory design?", then, after the answer: "okay make note of this
  retro so you can avoide that and refer to this down the line."
- **Ruling:** the "Architecture rules" bullet under [Project and delivery](#project-and-delivery).

### <a id="dec-2026-10-11-pii-rules"></a>DEC-2026-10-11-pii-rules: redaction uses our own rules

- **Status:** recorded. **Source:** owner, chat, 2026-10-11.
- **Context:** on the PII benchmark's sample (200 email excerpts of 1,500 characters, as jev
  receives them), the rules take 0.036 ms per email and 54.5 MB. They catch 97.6% of email-address
  characters, 71.4% of phone numbers and 73.1% of IDs, but almost no names (0.3%), and leave 58.6%
  of PII characters unmasked. The detectors that catch names cost far more: presidio-lg takes 56 ms
  per email but needs Python and about 1 GB of memory, and the small models that could ship in Rust
  take 130–148 ms and 420–580 MB. Paired with the rules, those models leave 8–10% unmasked.
- **Owner's words:** "okay so ai say we go with our own rules for pii then, seems to be the most
  pragmatic thing to do agree or no?"
- **Settles:** the detector default that
  [DEC-2026-10-11-redaction-default](#dec-2026-10-11-redaction-default) left to the benchmark.
- **Ruling:** the "Redaction uses our own rules" bullet under [Classification](#classification-jev).

### <a id="dec-2026-10-11-redaction-gaps"></a>DEC-2026-10-11-redaction-gaps: close the rules' gaps without a model

- **Status:** proposed by Claude on 2026-10-11.
- **Proposal:**
  - Mask the names the daemon already knows, as whole words: the display names in From, To and Cc,
    the owner's contacts and the owner's own name.
  - Add patterns for the identifier formats the rules miss (they caught 73% of IDs on the
    benchmark sample).
  - Say plainly at consent that, at `standard`, other people's and organisations' names in the
    body still go to jev.
  - Keep models out of the default. A model detector may come later as an opt-in level, if its
    speed keeps up with the sorter.
- **Builds on:** [DEC-2026-10-11-pii-rules](#dec-2026-10-11-pii-rules).

### <a id="dec-2026-10-11-pr-autonomy"></a>DEC-2026-10-11-pr-autonomy: Claude creates and merges PRs

- **Status:** recorded. **Source:** owner, chat, 2026-10-11.
- **Context:** a push to update PR #2 was held back, because pushing had not been allowed in that
  session.
- **Owner's words:** "I give you full permission for creating prs and merging prs!"
- **Ruling:** the "Autonomy" bullet under [Project and delivery](#project-and-delivery). It keeps
  the bullet's merge condition: changes reach `main` once CI passes.

### <a id="dec-2026-10-11-agent-kit"></a>DEC-2026-10-11-agent-kit: a project-agnostic agent kit

- **Status:** paused by [DEC-2026-10-11-build-first](#dec-2026-10-11-build-first).
  **Source:** owner, chat, 2026-10-11 (ef6ee949:812, ef6ee949:1610).
- **Owner's words:** "yes but what I am saying is that this linter and wait skill and continue
  etc.. can be abstracted for any workflow right? we can just remove anything specific to neomail,
  so that any workflow can use it and tweak it as a base" and "okay for the agent-spec-kit, it
  should go in @~/Projects/claude-plugins/"
- **Ruling:** the "Agent kit" bullet under [Project and delivery](#project-and-delivery).

### <a id="dec-2026-10-11-agent-kit-skill"></a>DEC-2026-10-11-agent-kit-skill: the kit's skill follows 80/20

- **Status:** paused by [DEC-2026-10-11-build-first](#dec-2026-10-11-build-first).
  **Source:** owner, chat, 2026-10-11 (ef6ee949:1992).
- **Owner's words:** "for the skill agent skill it doesn't need to cover absolutely eveerything
  think of the 80/20 what is the least amount I can include that will give me the most gains"
- **Builds on:** [DEC-2026-10-11-agent-kit](#dec-2026-10-11-agent-kit).
- **Ruling:** the "Agent kit" bullet under [Project and delivery](#project-and-delivery).

### <a id="dec-2026-10-11-build-first"></a>DEC-2026-10-11-build-first: build usable slices, not more design

- **Status:** recorded. **Source:** owner, chat, 2026-10-11 (ef6ee949:2139, ef6ee949:2168,
  ef6ee949:2208, ef6ee949:2216, ef6ee949:2249).
- **Context:** no code existed yet. The draft architecture had grown to 4,807 lines and the plan to
  1,065, each review round of them still found new problems, and the design and tooling runs kept
  going without finishing.
- **Owner's words:** "maybe this is to do with trying to perfect everything instead of building",
  then "if you look at it, nothing is finishing..". Asked whether to change course, the owner chose
  "Stop polishing, build now" and, for the first slice, "Read-only inbox in the CLI". Then: "forget
  about agent kit now, let us focus on what matters" and "we need to simplify what we have, and fix
  this mess..."
- **Ruling:** the "Build first" bullet under [Project and delivery](#project-and-delivery).
- **Supersedes:** [DEC-2026-10-11-consistency-process](#dec-2026-10-11-consistency-process), and
  the proposals [DEC-2026-10-11-walking-skeleton](#dec-2026-10-11-walking-skeleton),
  [DEC-2026-10-11-crate-reasons](#dec-2026-10-11-crate-reasons) and
  [DEC-2026-10-11-edge-channel](#dec-2026-10-11-edge-channel), which assumed the whole crate design
  up front.
- **Pauses:** [DEC-2026-10-11-agent-kit](#dec-2026-10-11-agent-kit) and
  [DEC-2026-10-11-agent-kit-skill](#dec-2026-10-11-agent-kit-skill).
- **Refines:** [DEC-2026-10-11-architecture-retro](#dec-2026-10-11-architecture-retro). Its rules
  apply to code as it lands: seams are listed per slice, reviews stop after two rounds, and the
  manifest and docs lint wait until there is code to check. These details are Claude's.

### <a id="dec-2026-10-11-cli-first"></a>DEC-2026-10-11-cli-first: finish the CLI before the TUI

- **Status:** recorded. **Source:** owner, chat, 2026-10-11 (ef6ee949:3427).
- **Context:** the roadmap had the read-only TUI as slice 6, after sign-in and background sync but
  before triage, redaction, jev and sending in the CLI.
- **Owner's words:** "I had a look at the roadmap and I would like to propose that we finish all
  the cli features before begining with the tui"
- **Ruling:** the "CLI first" bullet under [Project and delivery](#project-and-delivery).
  [roadmap.md](roadmap.md) splits each feature that has both a command and a screen into a CLI
  slice and a TUI slice. That split is Claude's.

### <a id="dec-2026-10-12-linux-first"></a>DEC-2026-10-12-linux-first: Linux before macOS

- **Status:** recorded. **Source:** owner, chat, 2026-10-12 (f679d889:502).
- **Context:** every slice so far was built and checked for Linux and macOS (a macOS CI job, a
  keychain store and an Apple-silicon edge build), while the owner and all live checks are on Linux.
- **Owner's words:** "for now let us prioritize all work for linux and not macos, the intial goal
  is to get this wroking on linux"
- **Ruling:** the "Linux first" bullet under [Platforms](#platforms). It changes the order, not the
  scope: the "First release" bullet still lists macOS. WSL runs Linux, so it stays in. Keeping
  macOS CI jobs as non-blocking, rather than removing them, is Claude's call.
- **Refines:** the merge condition in [DEC-2026-10-11-pr-autonomy](#dec-2026-10-11-pr-autonomy)
  ("once CI passes") and [DEC-2026-10-11-merge-gate](#dec-2026-10-11-merge-gate) ("once CI is
  green"): both mean the CI workflow run concludes `success`. A failed macOS job still shows as a
  failed check, but it leaves the run green, so it no longer holds back a merge.

### <a id="dec-2026-10-12-binary-neomail"></a>DEC-2026-10-12-binary-neomail: the binary is `neomail`

- **Status:** recorded. **Source:** owner, chat, 2026-10-12 (f679d889:1343).
- **Context:** `nmail` is already the binary of d99kris/nmail, an established terminal email
  client: a Homebrew core formula and the AUR packages `nmail`, `nmail-bin` and `nmail-git`. Its
  installer writes `~/.local/bin/nmail`, and its uninstaller removes that path. `neomail` was free
  on Homebrew, the AUR and crates.io when checked.
- **Owner's words:** chose "Rename to `neomail` (Recommended)" when asked what the binary should be
  called.
- **Ruling:** the binary, the Cargo package and the release assets are `neomail`, and neomail's own
  variables `NMAIL_CONFIG` and `NMAIL_DATA_DIR` become `NEOMAIL_CONFIG` and `NEOMAIL_DATA_DIR`
  (d99kris/nmail's installer also reads `NMAIL_*` variables); `JEV_API_KEY` keeps its name. The
  asset and variable names are Claude's call. Bullets that name the command now say `neomail`; log
  entries keep `nmail` where they quote or describe the past.
- **Supersedes:** the "Binary" part of the Name bullet under [Identity](#identity).

### <a id="dec-2026-10-12-install-channels"></a>DEC-2026-10-12-install-channels: install.sh and mise first

- **Status:** recorded. **Source:** owner, chat, 2026-10-12 (f679d889:1343).
- **Context:** research on install.sh, mise, the AUR and Homebrew. mise's github backend installs
  from the `edge` pre-release with no registry entry. AUR account registration is closed. A Homebrew
  tap needs versioned releases with fixed checksums.
- **Owner's words:** "install.sh and mise, without the rest for now." and, on the AUR, "no need for
  aur for now"
- **Ruling:** the "Clear, simple installation" bullet under
  [Project and delivery](#project-and-delivery). Homebrew and the AUR wait. The `SHA256SUMS` asset
  on edge, which the installer checks, is Claude's call.

### <a id="dec-2026-10-12-cli-ux"></a>DEC-2026-10-12-cli-ux: getting started, setup and output for people and pipes

- **Status:** recorded. **Source:** owner, chat, 2026-10-12 (f679d889:1537).
- **Context:** the owner asked how a new user configures neomail, where colour makes sense, and
  whether output can be piped to tools like fzf. A review found no onboarding, CLI colour or
  piping contract in the decisions or the roadmap: a new user gets help text and exit 2, accounts
  other than Gmail are written by hand, and `list` is an 80-column format that hides text past
  column 80 from fzf.
- **Owner's words:** chose "New slices 4 and 5 (Recommended)", "Print start lines only
  (Recommended)", "Auto plain TSV in pipes (Recommended)", and "Accept all (Recommended)" for
  "--jsonl next to --json; `account add` checks the login before writing the config, with
  --no-check; CLI colours are the 16 ANSI colours following your terminal theme; NO_COLOR strips
  all styling; a broken pipe exits 0 silently; no server autodiscovery for IMAP for now".
- **Ruling:** "The CLI for people and pipes" under
  [CLI, scripting, agents, extensibility](#cli-scripting-agents-extensibility). The roadmap gets
  two CLI slices before Outlook: "Getting started and setup", then "Output for people and pipes".
  The rest of the bullet is Claude's, from the recommendation shown with the questions: the
  provider menu, flags and `--no-input`, `account list` and `config path|check`, the stdout and
  stderr split, `--color` and CLICOLOR, every "no config" message naming the next command, and
  colour never carrying meaning alone.

### <a id="dec-2026-10-12-outlook-defaults"></a>DEC-2026-10-12-outlook-defaults: Outlook's open questions

- **Status:** proposed by Claude on 2026-10-12. The owner was shown these questions with defaults
  in chat before slice 6 was built (f679d889:2426) and has not answered yet; slice 6 follows the
  defaults.
- **Proposal:**
  - `--json` and `--jsonl` give `uid: null` for a message read through Microsoft Graph, which has
    no UIDs ([json.md](json.md)). `account list` keeps its field types: a Graph account shows
    Graph's host, port 443 and `tls`, and `server` says `"graph"`.
  - Device code sign-in is chosen on its own over SSH without a display: `SSH_CONNECTION` or
    `SSH_TTY` set and not empty, and `DISPLAY` and `WAYLAND_DISPLAY` unset or empty. neomail says
    so, and `--device-code` asks for it anywhere.
  - Outlook asks for `Mail.Read` only, until triage (roadmap slice 8) needs to write.
  - The setup docs say an app registration needs a directory, and what Microsoft documents about
    getting one: a free Azure account, with a phone number and a card for identity verification.
    They make no claim about what happens to the registration when the subscription ends.
  - Logout deletes the sign-in from the keyring and says Microsoft has no revocation endpoint; it
    prints no link to the consent page.
  - A tenant override, for an app registered in one organisation only, is a follow-up
    ([#32](https://github.com/AH-Merii/neomail/issues/32)), not part of slice 6.
- **Open question for the owner:** the "Outlook" bullet's note for docs says the registration
  survives cancelling the Azure subscription. Microsoft documents only that "when a subscription
  expires, the trusted instance remains"
  ([Microsoft Entra](https://learn.microsoft.com/en-us/entra/fundamentals/how-subscriptions-associated-directory)),
  which is the directory, not the registration in it. If accepted, this proposal supersedes that
  sentence; Claude follows "no claim" until the owner says otherwise.
- **Builds on:** "Outlook" in [Accounts and providers](#accounts-and-providers) and
  [DEC-2026-10-12-cli-ux](#dec-2026-10-12-cli-ux).

### <a id="dec-2026-10-12-daemon-notifications"></a>DEC-2026-10-12-daemon-notifications: desktop notifications and more of Linux

- **Status:** recorded. **Source:** owner, chat, 2026-10-12 (f679d889:4275 and f679d889:4278,
  answers at f679d889:4334).
- **Context:** the owner asked how `neomail daemon` handles notifications, whether it can use
  system notifications, and whether it works on different Linux systems, Linux mattering more.
  Part 1 of slice 7 only logs. Edge builds are x86_64 Linux and Apple Silicon only, and
  docs/daemon.md covers systemd only.
- **Owner's words:**
  - To "When should the daemon get desktop notifications for new mail?" chose "Right after 7a
    (Recommended)": "A small part of its own next: freedesktop notifications over D-Bus (zbus is
    already in the tree), on when a notification service exists, with a flag to turn them off.
    Push (IDLE) then makes them near-instant."
  - To "What should a new-mail notification show?" chose "Sender and subject (Recommended)": "One
    notification per account per round: up to 3 messages as 'Sender: Subject', then 'and N
    more'. Most useful, but visible on a lock screen or a shared screen."
  - To "Broaden Linux coverage now?" chose "ARM64 build + non-systemd docs (Recommended)": "Add an
    aarch64-unknown-linux-musl edge build (Raspberry Pi, ARM laptops) and OpenRC/runit examples
    in docs/daemon.md."
- **Ruling:** "Desktop notifications for new mail" under [UX](#ux), built as the next part of
  slice 7, before push. "More of Linux" under [Platforms](#platforms), done now, alongside that
  part. Claude's reading: the WSL bullet under Platforms still holds on WSL, where notifications
  go through Windows interop; this ruling covers Linux desktops.

### <a id="dec-2026-10-12-gmail-imap"></a>DEC-2026-10-12-gmail-imap: Gmail stays on IMAP

- **Status:** recorded. **Source:** owner, chat, 2026-10-12 (question at f679d889:6872, answer at
  f679d889:6874).
- **Context:** "Gmail sign-in modes" under [Accounts and providers](#accounts-and-providers) put an
  OAuth Gmail account on the Gmail API, with IMAP as a fallback, and roadmap slice 7 listed Gmail
  history. Gmail syncs over IMAP with XOAUTH2 and has had IDLE push since part 3 of slice 7.
  - The Gmail API allows 6,000 quota units per user per minute, and `messages.get` costs 20, so
    at most 300 message downloads a minute
    ([usage limits](https://developers.google.com/workspace/gmail/api/reference/quota), last
    updated 2026-09-10).
  - `messages.list` returns only each message's `id` and `threadId`
    ([messages.list](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages/list)),
    so labels and arrival times cost a `messages.get` per message too. A first sync of 500
    messages takes about 1,000 calls: over 3 minutes.
  - The API's own push goes through Cloud Pub/Sub
    ([push notifications](https://developers.google.com/workspace/gmail/api/guides/push)), so
    push would have stayed on IMAP IDLE either way.
- **Owner's words:** chose "Stay on IMAP (Recommended)": "Gmail keeps syncing over IMAP, with the
  IDLE push it already has, and slice 7 ends once Outlook (PR #45) merges. I'd record the change
  in decisions.md and the roadmap. Later slices would use the Gmail API only for what IMAP can't
  do, such as labels, decided slice by slice."
- **Ruling:** "Gmail sign-in modes" under [Accounts and providers](#accounts-and-providers). Slice
  7 ends with Graph's delta query for Outlook (PR #45), and its roadmap row no longer lists Gmail
  history. A slice that needs what IMAP can't do adds the Gmail API for that part only, in an
  entry of its own. Claude's reading: Gmail's IMAP fetches, adds and searches labels through
  `X-GM-LABELS`
  ([IMAP extensions](https://developers.google.com/workspace/gmail/imap/imap-extensions)), so
  labels alone do not call for the API.
- **Supersedes:** the "Gmail API, with IMAP as a fallback" part of the "Gmail sign-in modes"
  bullet under [Accounts and providers](#accounts-and-providers).

### <a id="dec-2026-10-13-wizards"></a>DEC-2026-10-13-wizards: setup wizards for Gmail, Outlook and Proton, none for other IMAP servers

- **Status:** recorded. **Source:** owner, chat, 2026-10-13, answering questions about the
  setup-wizard issues #47 to #51 (f679d889:13442).
- **Context:** the issues ask for `neomail account add` to guide a new user through each
  provider's setup. #51 asked the same for other IMAP servers, where the user may not know the
  server settings, while [DEC-2026-10-12-cli-ux](#dec-2026-10-12-cli-ux) rules out server
  autodiscovery for IMAP for now. A wrong OAuth client ID shows today only when the 5-minute
  browser sign-in times out (`SIGN_IN_TIMEOUT`, crates/neomail/src/oauth/mod.rs:29).
- **Owner's words:**
  - Asked what may fill in an IMAP server's settings (a table shipped with neomail, an
    explanation only, or network lookups): "no need for a wizard for imap".
  - Asked whether the Gmail and Outlook wizards may contact Google or Microsoft right after a
    client ID or secret is typed, before the sign-in: "Yes, if a live check confirms it
    (Recommended)".
- **Ruling:** the "Setup wizards" bullet under [Accounts and providers](#accounts-and-providers).
  #51 is closed as not planned. Claude's reading: a third question, which changes the wizards'
  live checks may make to the test accounts (2-Step Verification on the test Gmail, settings on
  the duplicate Entra app, a restart of the isolated Bridge), got no answer, so they make none.
- **Builds on:** [DEC-2026-10-12-cli-ux](#dec-2026-10-12-cli-ux) and "No built-in OAuth client
  IDs" under [Accounts and providers](#accounts-and-providers).

### <a id="dec-2026-10-13-wizard-steps"></a>DEC-2026-10-13-wizard-steps: how the setup wizards work

- **Status:** proposed by Claude on 2026-10-13, after the owner asked to work on the setup-wizard
  issues in parallel with the roadmap (f679d889:13268), following
  [DEC-2026-10-13-wizards](#dec-2026-10-13-wizards).
- **Proposal:**
  - One wizard model, data and a state machine with no I/O, drives every wizard in `neomail
    account add`. The CLI shows it on stderr, and the TUI's add-account screen (roadmap slice 19)
    can show the same steps. Each step says where the user is, why the step exists, where to go
    and what is left, and the user can go back until anything is written.
  - Each check runs right after the answer it tests. A failure names its step and what to change.
    On a terminal that step is asked again; off a terminal neomail prints the step and the fix and
    exits with the failure code. Nothing reaches the keyring or the config until every check
    passes.
  - Every question that changes the result keeps its flag. Secrets still come only from a no-echo
    prompt, standard input or a password command. `--no-input` runs from flags alone and skips
    the steps that only show a provider's page or advice.
  - Links are always printed. A browser opens only when the user asks and a display exists.
  - Three slices: the core with Gmail and an app password (#49) first; then, as soon as the core
    exists, Gmail with your own client and Outlook together (#47, #48), since they share the
    sign-in code; and Proton through Bridge (#50). All follow roadmap slice 12, which adds the
    sending settings to `account add`.
  - Until a live check shows that Google's and Microsoft's answers tell a wrong client ID or
    secret apart, a sign-in that times out names the steps whose mistakes it can mean.
- **Builds on:** [DEC-2026-10-13-wizards](#dec-2026-10-13-wizards) and
  [DEC-2026-10-12-cli-ux](#dec-2026-10-12-cli-ux).

### <a id="dec-2026-10-13-mark"></a>DEC-2026-10-13-mark: marking mail read or unread from the CLI

- **Status:** proposed by Claude on 2026-10-13, while the owner was away ("no more questions for
  now", f679d889:6909). Part 1 of roadmap slice 8 follows it.
- **Proposal:**
  - The command is `neomail mark read|unread ID...`. The ids are the cache's, as `neomail list`
    prints them, and `-` reads them from standard input, as for `neomail show`.
  - It is online only: it writes to the server, then updates the cache with what the server
    reported, and nothing is queued.
  - It takes the account's lock, as `neomail sync` does, so a sync of the same account never runs
    in between.
  - Over IMAP it sets or clears `\Seen` with `UID STORE` in INBOX, and refuses when UIDVALIDITY
    changed since the last sync. Nothing it does deletes, expunges or moves a message.
  - Output follows [DEC-2026-10-12-cli-ux](#dec-2026-10-12-cli-ux): one line on a terminal, the
    marked ids in a pipe, one object per message with `--json` or `--jsonl`. It exits 0, 1 when
    any message failed, and 2 on a usage error.
  - The actions are a typed set, `mark-read` and `mark-unread`, applied through one function that
    the TUI, scripts and agents will share. Archive, delete and move join it in the next part.
  - New Outlook sign-ins ask for `Mail.Read` and `Mail.ReadWrite`. A sign-in made before keeps
    `Mail.Read` and keeps syncing; `mark` on it asks the user to run `neomail account login NAME`
    once.
- **Builds on:** "The CLI can do everything the TUI can" in
  [CLI, scripting, agents, extensibility](#cli-scripting-agents-extensibility),
  [DEC-2026-10-12-cli-ux](#dec-2026-10-12-cli-ux), and "Outlook asks for `Mail.Read` only, until
  triage (roadmap slice 8) needs to write" in
  [DEC-2026-10-12-outlook-defaults](#dec-2026-10-12-outlook-defaults).

### <a id="dec-2026-10-13-categories"></a>DEC-2026-10-13-categories: groups as Outlook categories

- **Status:** proposed by Claude on 2026-10-13, as the third of slice 10's four parts
  ([DEC-2026-10-13-classify](#dec-2026-10-13-classify)).
- **Proposal:**
  - Each classified Outlook message gets one category: the label prefix followed by its effective
    group, the group `neomail list --group` shows (`nm-receipts-invoices`). It is written again
    when the effective group changes (by age, a new threshold, `--again` or a new prefix), and
    only then.
  - Only neomail's own categories change: the prefix followed by a group name, ignoring ASCII
    case, or the exact name neomail last wrote on that message. The user's other categories stay,
    in their order. A category the user removes or changes is left alone until the group changes.
  - Categories are set on the message by name, which needs `Mail.ReadWrite`, already part of
    every new sign-in since slice 8. neomail adds nothing to the master category list, so it asks
    for no new permission; a sign-in without `Mail.ReadWrite` writes nothing and says which
    command fixes it.
  - `neomail classify` writes them after storing its results, and the daemon after it classifies
    ([DEC-2026-10-13-daemon-classify](#dec-2026-10-13-daemon-classify)). What is still to write is
    worked out from the cache, so mail classified before 10c and failed writes need no queue. A
    `neomail classify` run writes at most 1,000 per account, a daemon wake at most 100.
  - `[classify] label_prefix` (default `nm-`, never empty) and `labels` (default true; false
    writes nothing) also serve 10d's Gmail and Proton labels
    ([DEC-2026-10-13-labels](#dec-2026-10-13-labels)).
  - Spam gets its category only; moving confident spam to Junk stays in 10d.
- **Builds on:** [DEC-2026-10-13-labels](#dec-2026-10-13-labels),
  [DEC-2026-10-13-classify](#dec-2026-10-13-classify) and
  [DEC-2026-10-13-mark](#dec-2026-10-13-mark).

### <a id="dec-2026-10-14-labels-junk"></a>DEC-2026-10-14-labels-junk: groups as Gmail and Proton labels, and spam to Junk

- **Status:** proposed by Claude on 2026-10-14, as the last of slice 10's four parts
  ([DEC-2026-10-13-classify](#dec-2026-10-13-classify)).
- **Proposal:**
  - Each classified Gmail or Proton message gets one label, the group's name after the prefix
    ([DEC-2026-10-13-labels](#dec-2026-10-13-labels)). `neomail classify` and the daemon write it
    in the same pass and under the same caps as Outlook's categories
    ([DEC-2026-10-13-categories](#dec-2026-10-13-categories)). The user's own labels are never
    touched: neomail removes only the name it wrote before.
  - Gmail is an account on `imap.gmail.com` whose server has the `X-GM-EXT-1` capability. It is
    labelled over IMAP with `X-GM-LABELS`, adding and removing single names, never replacing a
    message's set
    ([Gmail's IMAP extensions](https://developers.google.com/workspace/gmail/imap/imap-extensions)).
  - Proton is an account with Bridge's shape in the config (127.0.0.1 and a pinned certificate)
    whose server lists Bridge's `Labels` and `Folders`. A label is a copy into `Labels/<name>`.
    neomail's earlier label is removed by marking and expunging exactly that message's UIDs in
    that label's mailbox, only on a server that advertises UIDPLUS
    ([RFC 4315, section 2.1](https://www.rfc-editor.org/rfc/rfc4315#section-2.1)). Otherwise,
    and for a message without a Message-ID, the old label stays and a warning says so.
  - Other IMAP servers get no labels, so neomail signs in to them only when spam is due a move
    to Junk.
  - neomail creates its own labels when they are missing and keeps them. It never renames or
    deletes a label.
  - Spam at or above `classify.junk_at` (0.9 by default) is labelled, then moved to the account's
    Junk folder: the mailbox IMAP marks `\Junk`, or Outlook's `junkemail`. `classify.junk = false`
    moves nothing. Without a Junk folder or MOVE, nothing moves and a warning says so. Which
    messages are due a move is worked out apart from labels, so a failed move is tried again.
  - A message moves to Junk at most once: the move is recorded before it is made, and a message
    the user moves back is never moved again. Spam classified before this slice moves too, under
    the same caps.
- **Builds on:** [DEC-2026-10-13-labels](#dec-2026-10-13-labels),
  [DEC-2026-10-13-categories](#dec-2026-10-13-categories), and the "Spam" bullet under
  [Classification](#classification-jev).

### <a id="dec-2026-10-13-archive-delete"></a>DEC-2026-10-13-archive-delete: archiving and deleting mail from the CLI

- **Status:** proposed by Claude on 2026-10-13, while the owner was away ("no more questions for
  now", f679d889:6909). Part 2 of roadmap slice 8 follows it.
- **Proposal:**
  - The commands are `neomail archive ID...` and `neomail delete ID...`. They take ids as
    `neomail mark` does, run online only under the account's lock, and follow its output rules;
    `--json` also names the folder each message went to, as the server names it.
  - `delete` never deletes a message for good: it moves it to the account's trash, and neomail
    never empties the trash.
  - Over IMAP, the archive is the mailbox that LIST marks `\Archive`, else `\All` (Gmail's All
    Mail), and the trash is the one marked `\Trash`
    ([RFC 6154](https://www.rfc-editor.org/rfc/rfc6154.html#section-2)). On Outlook they are
    Graph's well-known folders `archive` and `deleteditems`
    ([mailFolder](https://learn.microsoft.com/en-us/graph/api/resources/mailfolder)).
  - Over IMAP a message moves from INBOX with `UID MOVE`
    ([RFC 6851](https://www.rfc-editor.org/rfc/rfc6851.html)), after the UIDVALIDITY check. A
    server without MOVE or without the folder fails the account and nothing moves: neomail never
    falls back to COPY, `\Deleted` and EXPUNGE, and never creates, renames or deletes a mailbox.
  - A move counts once the server confirms it: over IMAP when INBOX no longer has the UID, on
    Outlook when Graph answers `201 Created`. A confirmed message leaves the cache, as when a sync
    finds it gone.
  - The actions join the typed set as `archive` and `delete`. Moving to a folder the user names
    comes next.
- **Open question for the owner:** [DEC-2026-10-11-agent-send](#dec-2026-10-11-agent-send) has
  agents ask the owner before deleting. There are no agent profiles yet, so `neomail delete` asks
  no one. Once they exist, does a move to the trash count as deleting?
- **Builds on:** "archiving and deleting mail" in [Identity](#identity),
  [DEC-2026-10-13-mark](#dec-2026-10-13-mark) and [DEC-2026-10-12-cli-ux](#dec-2026-10-12-cli-ux).

### <a id="dec-2026-10-13-move"></a>DEC-2026-10-13-move: moving mail to a named folder, and listing folders

- **Status:** proposed by Claude on 2026-10-13, after the owner asked to "go autopilot and
  continue" (f679d889:9128). Part 3 of roadmap slice 8 follows it.
- **Proposal:**
  - `neomail folder list [--account NAME]` lists each account's folders from its server: the
    name, the account and the role (inbox, archive, trash, junk, sent, drafts, all, flagged, or
    none). It is a read: it takes no lock and leaves the cache alone. A folder that cannot hold
    messages is left out. An account that fails is one error line, the others are still listed,
    and the exit code is 1.
  - `neomail move FOLDER ID...` takes the folder first, as `neomail mark read ID...` takes the
    state, and the ids as `neomail archive` does, `-` included. Each message moves from the INBOX
    to the folder of that name in its own account, under archive's rules: online only, under the
    account's lock, counted once the server confirms it, and then removed from the cache.
  - A name matches exactly as `folder list` prints it. Over IMAP that is the mailbox name decoded
    from modified UTF-7
    ([RFC 3501 section 5.1.3](https://www.rfc-editor.org/rfc/rfc3501#section-5.1.3)), with the
    server's own delimiter, and the server's encoded name is the one sent back. On Outlook it is
    the path of display names from the top of the mailbox, such as `Inbox/Receipts`, and the move
    sends the folder's id.
  - Refused before anything moves, for that account only: the INBOX (in any case over IMAP; on
    Outlook by its id, since its display name follows the mailbox's language), a folder that
    cannot hold messages, a name no folder has, and a name two folders share. neomail never
    creates, renames or deletes a folder.
  - The action joins the typed set as `move`, carrying the folder name as a typed value.
    `folder list` is a read, not an action.
- **Builds on:** [DEC-2026-10-13-archive-delete](#dec-2026-10-13-archive-delete),
  [DEC-2026-10-13-mark](#dec-2026-10-13-mark) and [DEC-2026-10-12-cli-ux](#dec-2026-10-12-cli-ux).

### <a id="dec-2026-10-13-send"></a>DEC-2026-10-13-send: sending, replying and forwarding from the CLI

- **Status:** proposed by Claude on 2026-10-13, for roadmap slice 12.
- **Proposal:**
  - `neomail send`, `neomail reply ID [--all]` and `neomail forward ID` take a Markdown body from a
    file or standard input and send it as the Markdown drafts bullets under [UX](#ux) say: the
    rendered HTML plus the Markdown source, or `--text-only`. `--from` picks the account; one
    identity per account (its address and display name) for now.
  - Proton (through Bridge), Gmail (OAuth or an app password) and other IMAP accounts send over
    SMTP, with a small client of neomail's own in nm-imap that shares its TLS and certificate
    pinning. It greets the server as `[127.0.0.1]`, so the machine's name never leaves it.
    Outlook sends through Microsoft Graph's `sendMail`, so new Outlook sign-ins also ask for
    `Mail.Send`; an older sign-in is told to run `neomail account login` once, as for marking
    ([DEC-2026-10-13-mark](#dec-2026-10-13-mark)). Graph takes a MIME message's recipients from
    its headers, where Bcc never is, so an Outlook account refuses a Bcc before sending anything.
  - Each message sent gets one copy in Sent where it can. Gmail, Bridge and Graph store it
    themselves; for other IMAP servers neomail appends it to the mailbox marked `\Sent`, and never
    creates one, so a server without one keeps no copy and a warning says so.
  - On a terminal neomail shows who it goes to and asks before sending; a no sends nothing and
    exits 1. `--yes` sends from a script; without a terminal or with `--no-input`
    ([DEC-2026-10-12-cli-ux](#dec-2026-10-12-cli-ux)), and no `--yes`, nothing is sent: exit 2,
    as for any question that can't be asked ([exit codes](json.md#exit-codes)). `--dry-run`
    prints the message instead.
  - A refused recipient sends nothing to anyone, and so does a reply that would have to leave out
    an address it takes from the original because it can't be sent to yet (one that isn't
    ASCII). A message that was sent but whose Sent copy failed exits 0 with a warning, so a
    script never sends it twice.
  - A forward is inline, with the original's headers above its text; attachments are not carried
    until slice 15.
  - Threading follows RFC 5322 3.6.4: In-Reply-To and References from the original.
- **Builds on:** [DEC-2026-10-11-test-accounts](#dec-2026-10-11-test-accounts) for the live check;
  agents sending with approval stay with slice 17
  ([DEC-2026-10-11-agent-send](#dec-2026-10-11-agent-send)).

### <a id="dec-2026-10-13-redact"></a>DEC-2026-10-13-redact: the redaction preview and what it masks

- **Status:** proposed by Claude on 2026-10-13, after the owner asked to "go autopilot and
  continue" (f679d889:9128). Roadmap slice 9 follows it.
- **Proposal:**
  - `neomail redact ID...` prints, for each cached message, the text that classification will
    send to jev: the subject, whole, and the start of the body, both redacted. It reads the cache
    only and sends nothing; `--json` gives the text exactly, with a count of masks per kind.
  - The body is the text `neomail show` renders. It is redacted whole and then cut to its first
    1,500 characters, the excerpt size the PII benchmark measured, never inside a mask, so a value
    at the edge can't slip past its rule.
  - A new pure crate, nm-redact, owns the rules and how masks are spelled. Its result, an
    `Excerpt`, can only be made by redacting, and slice 10's request will take nothing else.
  - The rules are neomail's own, ported from the benchmark's prototype: an address's local part,
    the owner's logins whole, links down to their host, IBANs and cards that pass their checksums,
    amounts, one-time codes and passwords after their keyword, phone numbers, any other run of 4 or
    more digits, and the capitalised words of the display names in From, To, Cc and Reply-To.
    Domains, link hosts, dates, times and numbers of 1 to 3 digits stay.
  - Where rules overlap, the more specific one names the mask, in the order email, link, IBAN,
    card, amount, code, phone, number, name: a valid card number is never called a phone number.
  - [docs/privacy.md](privacy.md) says what the rules mask and keep, and what still gets through.
- **Builds on:** [DEC-2026-10-11-pii-rules](#dec-2026-10-11-pii-rules),
  [DEC-2026-10-11-redaction-gaps](#dec-2026-10-11-redaction-gaps) and
  [DEC-2026-10-11-redaction-default](#dec-2026-10-11-redaction-default).

### <a id="dec-2026-10-13-redaction-aim"></a>DEC-2026-10-13-redaction-aim: redaction covers common mail first

- **Status:** recorded. **Source:** owner, chat, 2026-10-13 (f679d889:10953).
- **Owner's words:** "remember for the redaction the objective is not having 100% coverage, the
  objective is to have coverage for the most commonly encountered emails"
- **Ruling:** the "Redaction aims at common mail" bullet under
  [Classification](#classification-jev).
- **Builds on:** [DEC-2026-10-11-redaction-gaps](#dec-2026-10-11-redaction-gaps).

### <a id="dec-2026-10-13-classify-send"></a>DEC-2026-10-13-classify-send: the sender's domain, consent, and the live check

- **Status:** recorded. **Source:** owner, chat, 2026-10-13, answering three questions
  (f679d889:11167).
- **Owner's words:**
  - Asked whether classification should also send the sender's domain (for example amazon.com):
    "Always".
  - Asked whether neomail should show what leaves the machine and ask once before the first
    `neomail classify` sends anything: "Ask once per backend".
  - Asked whether slice 10's live check may call jev with the owner's key, at most 20 requests on
    made-up messages, logged to a spend ledger: "Yes, up to 20 calls".
- **Ruling:** the "The sender's domain is sent", "Consent per backend host" and "The live check of
  classification" bullets under [Classification](#classification-jev).
- **Builds on:** [DEC-2026-10-11-jev-offline](#dec-2026-10-11-jev-offline) and
  [DEC-2026-10-13-redact](#dec-2026-10-13-redact).

### <a id="dec-2026-10-13-classify"></a>DEC-2026-10-13-classify: classifying cached mail from the CLI

- **Status:** proposed by Claude on 2026-10-13, after the owner asked to keep "working on the next
  features in the roadmap" (f679d889:10772). Roadmap slice 10 follows it.
- **Proposal:**
  - Slice 10 ships in four parts: 10a classifies cached mail and lists it by group from the CLI;
    10b classifies as the daemon syncs; 10c stores groups as Outlook categories; 10d stores them as
    Gmail and Proton labels, and moves confident spam to Junk.
  - `neomail classify [ID...]` takes cached mail with no result from the last 365 days, newest
    first, at most 1,000 messages, and estimates what the rest would cost
    ([DEC-2026-10-11-first-run](#dec-2026-10-11-first-run)); `--all` does the rest.
    `neomail key set|delete|status` keeps the key, and `neomail list --group G` lists a group.
  - With the redacted excerpt and the sender's domain go four signals that identify nobody: whether
    the sender is a role address (noreply@, orders@ …), and whether the mail comes from a mailing
    list, is automated, or is a reply. They mark the commonest bulk and automated mail. The
    address's local part and display names never go. nm-redact builds all of it, so only redacted
    text and these fields can reach a request.
  - One request per message: one choice question with the ten groups in a fixed order, `main`
    first, and the model pinned to `jev-1.13.0`, so stored probabilities stay comparable.
  - A result is keyed by account and Message-ID, or a hash of the message when it has none, so it
    survives a mailbox rebuild (a new UIDVALIDITY) and no message is paid for twice. The effective
    group (`other` below a confidence threshold, `follow-up-old` by age, `spam`) is worked out when
    the cache is read, so thresholds can change without asking jev again.
  - The key lives in the keyring under service `neomail-jev`, one entry per backend host, so it
    can't share an entry with a mail account named "jev". `JEV_API_KEY` is the fallback.
  - No new crate: every backend speaks one wire format, and the base URL is the seam. Plain HTTP
    is allowed only to a loopback host, for local backends and the tests' fake jev.
- **Builds on:** [DEC-2026-10-13-classify-send](#dec-2026-10-13-classify-send),
  [DEC-2026-10-11-first-run](#dec-2026-10-11-first-run) and
  [DEC-2026-10-11-jev-offline](#dec-2026-10-11-jev-offline).

### <a id="dec-2026-10-13-labels"></a>DEC-2026-10-13-labels: group labels carry a prefix neomail doesn't show

- **Status:** recorded. **Source:** owner, chat, 2026-10-13, answering two questions
  (f679d889:12486).
- **Owner's words:**
  - Asked what slice 10d's live check may do with labels on the test Gmail and Proton accounts:
    "Create labels, keep them (Recommended)".
  - Asked what the labels neomail creates should be called: "configurable, but the default is nm-
    as a prefix, one more thing, in neomail we do not see the nm prefix".
- **Ruling:** the "Group labels" bullet under [Classification](#classification-jev).
- **Builds on:** "Where groups are stored" under [Classification](#classification-jev).

### <a id="dec-2026-10-13-daemon-classify"></a>DEC-2026-10-13-daemon-classify: the daemon classifies new mail

- **Status:** proposed by Claude on 2026-10-13, as the second of slice 10's four parts
  ([DEC-2026-10-13-classify](#dec-2026-10-13-classify)).
- **Proposal:**
  - `neomail daemon` classifies each round's new mail after that round's sync and notifications,
    with the same request `neomail classify` sends. Mail from a first sync or a rebuilt mailbox is
    left to slice 11's first run and to `neomail classify`
    ([DEC-2026-10-11-first-run](#dec-2026-10-11-first-run)).
  - It never asks. It sends only to a host the user already consented to through `neomail
    classify`, and only with a key; otherwise it says once why classifying is off. From this
    slice on, the consent text says the daemon then sends new mail as it arrives
    ([DEC-2026-10-13-classify-send](#dec-2026-10-13-classify-send)). A yes given before, to a
    question that didn't mention the daemon, still covers `neomail classify` but not the daemon,
    which waits for a yes to the new question on a terminal; `--yes` never turns the earlier yes
    into the daemon's.
  - `classify.daemon = false` turns it off; it is on by default.
  - Each time the daemon syncs (a round, a push sync or an Outlook check) it sends at most 100
    requests; the rest wait in memory for the next time. Mail older than 365 days is skipped, as
    `neomail classify` skips it.
  - A refused key pauses classifying until the key changes. A 402 (no credit left) pauses it for
    an hour, or until the key changes. An answer that would come back every time (one that isn't
    jev's, or a status such as 400 or 404) pauses it until `classify.url` or `classify.model`
    changes. A restart ends any pause. A stopped run puts its messages back for the next time.
  - Notifications don't change: they never wait for a group
    ([DEC-2026-10-12-daemon-notifications](#dec-2026-10-12-daemon-notifications)).
- **Builds on:** [DEC-2026-10-13-classify](#dec-2026-10-13-classify) and
  [DEC-2026-10-13-classify-send](#dec-2026-10-13-classify-send).

### <a id="dec-2026-10-14-classify-when-asked"></a>DEC-2026-10-14-classify-when-asked: classify only when asked

- **Status:** recorded. **Source:** owner, chat, 2026-10-14 (f679d889:15467, f679d889:15474),
  after Claude described slice 11 as classifying the last year on first sync without asking.
- **Supersedes:** [DEC-2026-10-11-first-run](#dec-2026-10-11-first-run).
- **Changes:** [DEC-2026-10-13-daemon-classify](#dec-2026-10-13-daemon-classify): background
  classifying becomes opt-in.
- **Owner's words:**
  - "no I do not want it to classify anything without asking, it should ask and then classify or
    only classify when asked.."
  - "If I want autoclassify to run in the background I should explicitly enable that"
- **Ruling:** the "Classify only when asked" and "The daemon classifies new mail" bullets under
  [Classification](#classification-jev), and slice 11 in [the roadmap](roadmap.md).
- **Retires:** `classify.daemon` defaulting to `true`, and the consent text's line that the daemon
  then also sends each new message.

### <a id="dec-2026-10-14-attachments"></a>DEC-2026-10-14-attachments: attachments in the CLI

- **Status:** proposed by Claude on 2026-10-14, for roadmap slice 14.
- **Context:** the cache already holds every message's whole MIME source, attachments included:
  IMAP fetches `BODY.PEEK[]` and Outlook fetches Graph's `/messages/{id}/$value`, both stored in
  `message.raw`. So attachments need no fetch, no new table and no cache migration, and every
  provider gets them the same way.
- **Proposal:**
  - `neomail attachment list ID` names each attachment by an index from 1, in MIME order (the
    order mail-parser gives), which `save` and `open` take. It shows the safe name, the real type,
    the declared type, the decoded size and three notes: `mismatch`, `inline` and `executable`.
    A nested `message/rfc822` is one attachment, its name ending in `.eml` (added when missing);
    its own attachments stay inside it.
  - The bytes are the ones sent: mail-parser turns a text part (any `text/*`, and a part
    without a Content-Type) into UTF-8, which would corrupt a binary file sent that way, so for
    those only the transfer encoding (base64 or quoted-printable) is undone on the raw part and
    the charset is left as it is.
  - The real type comes from the content: a table of signatures (PDF, images, archives, Office
    and OpenDocument files told apart inside ZIP and OLE, audio and video, ELF, Windows PE,
    Mach-O, shortcuts, `#!` scripts), else HTML, SVG, XML or text for UTF-8 without NUL bytes,
    else `application/octet-stream`. No new dependency: the table is small and the standard
    library does the rest. `mismatch` is set when a recognised real type contradicts the
    declared type or the name's extension; aliases (`image/jpg`) and containers (a `.docx` sent
    as `application/zip`) are not mismatches, and unknown content never is.
  - `inline` marks a part whose Content-ID an HTML body of the message points at with `cid:`.
  - `executable` marks executables, installers, shortcuts and scripts: by real type, or by the
    safe name's extension (`.exe`, `.js`, `.sh`, `.desktop`, `.docm`, `.xlam`, `.flatpakref` and
    the like).
  - Safe names: the last path component only, without control, bidi or invisible characters,
    `:*?"<>|` as `_`, no leading dots, dashes or spaces and no trailing dots or spaces, at most
    200 bytes keeping a short extension; `attachment-INDEX` with the real type's extension when
    nothing is left; `-INDEX` added when two attachments of a message would share a name, then
    `-2`, `-3` and so on while that is taken too.
  - `save ID [INDEX...]` writes into `--dir` (default: the current directory), mode 0600 so a
    file is never executable, without following a symbolic link, and never replaces a file
    unless `--overwrite`: then it writes a temporary file and renames it over the old one. It
    prints where each file went; a file that exists or an unknown index is reported on standard
    error after the others are saved, and exits 1.
  - `open ID INDEX` writes the file to `attachments/ID-INDEX/NAME` under neomail's data
    directory (never `/tmp`), directories 0700 and the file 0600, and runs the opener on it:
    `NEOMAIL_OPENER` when set (which is also how the tests swap in a stub), else `open` on
    macOS, `wslview` on WSL, else `xdg-open`. neomail waits for the opener and reports its
    failure (exit 1). A later `open` removes a copy whose newest file is more than a day old, since
    an app may read the file after the opener returned and a user may edit it in place; the
    message after opening says the copy is temporary. Before an executable or script it warns on standard error
    and asks on a terminal (a no exits 1); without a terminal, or with `--no-input`, it refuses
    with exit 2 unless `--yes`. neomail never runs a file itself.
  - Output follows [DEC-2026-10-12-cli-ux](#dec-2026-10-12-cli-ux): `list` and `save` are
    listings (human, TSV, `--json`, `--jsonl`); `open` prints the file's path, or one object with
    `--json`. Shapes in [json.md](json.md).
- **Known limits:** macOS quarantine flags, a downloads
  folder in the config, terminal previews and the attachments picker are left for later.
- **Builds on:** "Attachments" and "The CLI and agents get the same commands" in
  [Images and attachments](#images-and-attachments), and
  [DEC-2026-10-12-cli-ux](#dec-2026-10-12-cli-ux).
