# neomail — product decisions

The source of truth for what neomail must do. Decisions are made by the project owner; agents
implement them. When a decision changes, edit this file in the same change. Newest first within
each section is not required — keep related decisions together.

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
- **A neomail logo.**
- **Autonomy:** agents work on GitButler branches; changes are merged to `main` once CI passes; the
  docs site is published to GitHub Pages; pre-release builds may be cut without asking.
- Commits in this repository are unsigned (local `commit.gpgsign=false`) while agents work
  unattended.
