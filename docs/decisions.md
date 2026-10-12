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
  (`service=neomail`, `account=jev` in development). Users bring their own key; neomail never ships
  one.
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
- **Each email is classified once** (results cached locally with model id, probabilities and
  timestamp so thresholds can be re-tuned without re-calling the model).
- **First run** ([DEC-2026-10-11-first-run](#dec-2026-10-11-first-run)): neomail classifies the last 365 days
  automatically, newest first, without asking, capped at the newest 1,000 emails. It then shows an
  estimate of what classifying all the remaining emails would cost (count, requests, time, money),
  and the user can start that with one action. The window and the cap are configurable.
- **Redaction uses our own rules** ([DEC-2026-10-11-pii-rules](#dec-2026-10-11-pii-rules)): the PII
  redaction that runs before text goes to jev is neomail's own rule redactor. No model is part of
  the default.

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

- **Status:** recorded. **Source:** owner, chat, 2026-10-11.
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
