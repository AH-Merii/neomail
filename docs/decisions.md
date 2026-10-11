# neomail — product decisions

The source of truth for what neomail must do. Decisions are made by the project owner; agents
implement them. The bullets below say what is decided now; keep related decisions together.

Each ruling also has an entry in the [decision log](#decision-log) with an ID
(`DEC-YYYY-MM-DD-slug`), the owner's own words and a status. Docs and workflow scripts cite the
ID and never copy the text. Log entries are append-only: a change is a new entry that supersedes
the old one, and the bullet above it is updated to link the new entry. The process is
[process/consistency/process.md](process/consistency/process.md).

## Identity

- **Name:** neomail (inspired by Neovim). **Binary:** `nmail`.
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
  guided by an in-app wizard (`nmail account add`) and step-by-step docs that explain *why* each
  step exists. Never embed or borrow another application's client ID (e.g. Thunderbird's).
- **Gmail sign-in modes:** (1) the user's own Google OAuth client (Gmail API, with IMAP as a
  fallback), (2) app password over IMAP/SMTP (requires Google 2-Step Verification).
- **Outlook:** the user's own Microsoft Entra app registration (multi-tenant + personal accounts,
  public client, `http://localhost` redirect, public client flows enabled for device code).
  Microsoft Graph is the primary API (categories, delta sync, send). IMAP/SMTP is an optional
  fallback. Note for docs: personal Microsoft accounts cannot register apps without a directory —
  the user must sign up for an Azure (free) account to get one (card required for identity
  verification only). The registration survives cancelling the Azure subscription.
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
- **Theming.**
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
  - The CLI and agents get the same commands (`nmail attachment list|save|open --json`). Agents read attachment text as data, never as instructions.
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
- **WSL specifics are first-class:** detect WSL (`WSL_DISTRO_NAME` / WSLInterop) and handle it —
  secrets fall back when no Secret Service is available (password command such as `pass`, Windows
  Credential Manager via interop, or an encrypted file store), OAuth opens the Windows browser
  (`wslview` / `explorer.exe`) while the loopback redirect still works via WSL2 localhost
  forwarding, and clipboard/notifications go through Windows interop (or OSC 52).

## Project and delivery

- **GitHub repo** (public) managed with the **GitButler CLI (`but`)** so parallel agents don't step on
  each other's toes.
- **CI/CD** for builds and tests.
- **Clear, simple installation.**
- **A simple website with docs hosted on GitHub Pages**, including a first-time setup guide
  (why and how to register OAuth apps, Proton Bridge, app passwords, keyring).
- **A neomail logo** ([DEC-2026-10-11-logo](#dec-2026-10-11-logo)): variation 05, "Split and
  statusline", in `assets/logo/`.
- **Autonomy:** agents work on GitButler branches; Claude creates, updates and merges PRs without
  asking ([DEC-2026-10-11-pr-autonomy](#dec-2026-10-11-pr-autonomy)); changes are merged to `main`
  once CI passes; the docs site is published to GitHub Pages; pre-release builds may be cut without
  asking.
- Commits in this repository are unsigned (local `commit.gpgsign=false`) while agents work
  unattended.
- **`main` is always usable** ([DEC-2026-10-11-main-usable](#dec-2026-10-11-main-usable)): every
  merge to `main` builds and publishes `nmail`, so whatever has landed can be tried while the rest
  is still being built. A merged CLI command works before the TUI exists, and independent features
  become usable as each one merges.
- **Live tests** ([DEC-2026-10-11-test-accounts](#dec-2026-10-11-test-accounts)): the three
  dedicated test accounts may send and receive freely. The owner's personal accounts are never used.
- **Design consistency** ([DEC-2026-10-11-consistency-process](#dec-2026-10-11-consistency-process)):
  the process in [process/consistency/process.md](process/consistency/process.md) binds every
  workflow, agent and CI job that writes `docs/` or code.
- **Architecture rules** ([DEC-2026-10-11-architecture-retro](#dec-2026-10-11-architecture-retro)):
  - a seams inventory comes first;
  - a capability with two or more consumers, or a realistic alternative, gets a port with a contract
    and a conformance suite;
  - each cross-cutting concern has one owner section, which others link to and never redefine;
  - the architecture is machine-checkable: a manifest, generated tables and a docs lint;
  - invariants live in types, not comments;
  - adversarial review runs until a round finds nothing new;
  - when requirements change, the affected sections are re-derived, not patched;
  - a walking skeleton comes early.
- **Benchmarks** ([DEC-2026-10-11-bench-sample](#dec-2026-10-11-bench-sample)): a benchmark runs
  on a representative sample, just large enough to measure the metrics, never on whole datasets.
  The choice weighs speed against accuracy.

## Decision log

Statuses:

- **proposed:** made on the owner's behalf while they were away; followed until the owner accepts
  or rejects it.
- **recorded:** the owner ruled, and the docs are not yet updated to match.
- **live:** the docs match.
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
  The concrete CI/CD and versioning design is
  [DEC-2026-10-11-edge-channel](#dec-2026-10-11-edge-channel).

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

- **Status:** recorded; the tooling it needs is not built yet. **Source:** owner, chat,
  2026-10-11, delegating the decision to Claude.
- **Owner's words:** "based on this how would avoide that from happening down the line? think of
  how you would structure your workflows to catch this or to routinely check I will leave the
  decision to you."
- **Ruling:** [process/consistency/process.md](process/consistency/process.md). Its evidence is in
  the same folder.

### <a id="dec-2026-10-11-walking-skeleton"></a>DEC-2026-10-11-walking-skeleton: walking skeleton, then a thin slice

- **Status:** proposed by Claude on 2026-10-11 while the owner was hands-off. It was D1 in the
  architecture brief.
- **Proposal:**
  - Wave 0 ends with every crate compiling behind its real traits, and with the composition roots
    (`nm-daemon`, `nm-tui`, `nmail`) running end to end on stubs and fakes (`nmail --demo`, one
    action over IPC).
  - Work is then ordered by a thin-slice milestone, 0.1.0-alpha.1: read, search and triage
    (archive, delete, move, classify) real mail in the TUI from Proton (via Bridge) and Gmail.
  - Everything else follows that milestone.

### <a id="dec-2026-10-11-crate-reasons"></a>DEC-2026-10-11-crate-reasons: every crate has a reason

- **Status:** proposed by Claude on 2026-10-11. It was D2 in the architecture brief.
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
- **Why:** this settles the conflict between the permission profiles in `07-actions-api-cli.md` and
  the agent rules in `12-extensions-agents-mcp.md`.

### <a id="dec-2026-10-11-redaction-default"></a>DEC-2026-10-11-redaction-default: redact in the daemon; the benchmark picks the detectors

- **Status:** proposed by Claude on 2026-10-11.
- **Proposal:** text leaves the daemon, for jev or for MCP clients, only after redaction inside the
  daemon. `privacy.detectors` defaults to `["rules"]` until the PII benchmark reports, and the
  benchmark's result then sets the default.
- **Owner's words on the benchmark:** "you will benchmark it later and that will enforce your
  decision."

### <a id="dec-2026-10-11-edge-channel"></a>DEC-2026-10-11-edge-channel: the edge channel and versioning

- **Status:** proposed by Claude on 2026-10-11. This is the concrete design for
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
- **Refines:** D1 in the brief, which had stewards re-wire the roots at each phase.

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
- **Context:** on the PII benchmark's email excerpts (the first 1,500 characters, as jev receives
  them), the rules caught 99–100% of email addresses, 96% of phone numbers, all links and 67% of
  other IDs, at about 0.04 ms per email and 55 MB. They caught at most 29% of people's names. The
  detectors that catch names took 1.7–1.8 s per email, which would hold up the jev sorter, or had
  no speed measurement yet.
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
  - Add patterns for the identifier formats the rules miss (they caught 67% of IDs).
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
