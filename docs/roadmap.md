# neomail roadmap

The order in which neomail is built. Each slice is thin, usable on its own, about a day or two of
work, and merges to `main` when it works
([DEC-2026-10-11-build-first](decisions.md#dec-2026-10-11-build-first)). Every merge publishes
`nmail`, so each slice can be tried as soon as it lands
([DEC-2026-10-11-main-usable](decisions.md#dec-2026-10-11-main-usable)). The "Delivers" column
links the parts of [decisions.md](decisions.md) a slice implements; most sections land over
several slices. Before a slice is built it lists the seams it touches
([DEC-2026-10-11-architecture-retro](decisions.md#dec-2026-10-11-architecture-retro)).

Every CLI feature comes before the TUI
([DEC-2026-10-11-cli-first](decisions.md#dec-2026-10-11-cli-first)). A feature with both a command
and a screen is split in two: its CLI slice is in the first table and its TUI slice in the second.

This file is a plan, not a ruling: decisions.md wins where they differ. Reorder slices only with a
reason from decisions.md.

## The CLI

| # | Slice | Once it lands, you can… | Delivers |
|---|---|---|---|
| 1 | Read-only CLI inbox | sync the inboxes of your IMAP accounts (Proton via Bridge, or any server with a password command), list the messages and show one, from an edge pre-release that CI builds on every merge. | [Build first](decisions.md#dec-2026-10-11-build-first), [`main` is always usable](decisions.md#dec-2026-10-11-main-usable), Proton in [Accounts](decisions.md#accounts-and-providers), the SQLite cache in [Data and storage](decisions.md#data-and-storage), CI/CD in [Project and delivery](decisions.md#project-and-delivery) |
| 2 | OS keyring | keep the IMAP password in the OS keyring instead of behind a password command, on Linux, macOS and WSL. | Secrets in [Accounts](decisions.md#accounts-and-providers), the WSL secret fallback in [Platforms](decisions.md#platforms) |
| 3 | Gmail with your own OAuth app | add a Gmail account with `nmail account add`, which guides you through registering your own Google client and signs in with PKCE in the browser. | Gmail and "no built-in client IDs" in [Accounts](decisions.md#accounts-and-providers), the WSL browser in [Platforms](decisions.md#platforms) |
| 4 | Outlook with your own Entra app | add an Outlook or Hotmail account the same way, reading through Microsoft Graph, with device code sign-in over SSH. | Outlook in [Accounts](decisions.md#accounts-and-providers) |
| 5 | Background sync | see new mail without running `nmail sync`: a background process keeps every account's cache current (IMAP IDLE, Gmail history, Graph delta), and the CLI reads from it. | The SQLite cache with sync in [Data and storage](decisions.md#data-and-storage), multiple accounts in [Accounts](decisions.md#accounts-and-providers) |
| 6 | Triage in the CLI | archive, delete, move and mark read from the CLI, with `--json` output and documented exit codes. | The action registry and `--json` in [CLI, scripting, agents, extensibility](decisions.md#cli-scripting-agents-extensibility) |
| 7 | Redaction preview | run `nmail redact` on a message to see exactly the text that would go to jev, with addresses, numbers, IDs and known names masked. | [Redaction uses our own rules](decisions.md#dec-2026-10-11-pii-rules), [redact in the daemon](decisions.md#dec-2026-10-11-redaction-default) and [the rules' gaps](decisions.md#dec-2026-10-11-redaction-gaps) (both proposed) |
| 8 | Classify with jev | sort messages into the default groups with your own jev key, cached once per message and stored as provider labels or categories, and list mail by group from the CLI, across all accounts or one. | Groups, key, backends and tag storage in [Classification](decisions.md#classification-jev), [tests never call jev](decisions.md#dec-2026-10-11-jev-offline) (proposed), unified views in [Accounts](decisions.md#accounts-and-providers) |
| 9 | First run and group rules | get the last year classified automatically on first sync, with a cost estimate for the rest, `follow-up-old` by age and confident spam in Junk. | [First run](decisions.md#dec-2026-10-11-first-run), the spam, `other` and `follow-up-old` rules in [Classification](decisions.md#classification-jev) |
| 10 | Send from the CLI | send, reply or forward from a chosen identity with a Markdown body, sent as rendered HTML plus the Markdown source. | Markdown drafts in [UX](decisions.md#ux), choosing the sending identity in [Accounts](decisions.md#accounts-and-providers) |
| 11 | Search from the CLI | search all mail with `nmail search`, across every account or within a thread, group or account, with `--json`, through the same sources the TUI's pickers will use. | Cross-account search in [Accounts](decisions.md#accounts-and-providers), scopes and "Agents and the CLI can query the same sources" in [Search and pickers](decisions.md#search-and-pickers) |
| 12 | Attachments in the CLI | list, save and open attachments with `nmail attachment list\|save\|open --json`, with real types, safe names and a warning before an executable opens. | Attachments and "The CLI and agents get the same commands" in [Images and attachments](decisions.md#images-and-attachments) |
| 13 | Files from the CLI | attach files to a message sent from the CLI, get warned about a forgotten attachment or a size limit, and send large files through the provider's upload sessions. | Drafting with files in [Images and attachments](decisions.md#images-and-attachments) |
| 14 | Messages in a browser | open a message from the CLI as a sanitised copy in the browser, with remote images blocked unless you allow them for that message or always for its sender. | "Open the full message in a browser" and remote images in [Images and attachments](decisions.md#images-and-attachments) |
| 15 | Agents over MCP | let an agent read, search and draft your mail through MCP or the CLI's self-describing schema, with sending and deleting approved per action. | Agents in [CLI, scripting, agents, extensibility](decisions.md#cli-scripting-agents-extensibility), [agents draft freely, send with approval](decisions.md#dec-2026-10-11-agent-send) (proposed) |
| 16 | Lua actions, hooks and scripts | add your own actions and hooks in Lua with hot reload, and run scripts on a schedule. | Lua, scripting, and the events and action registry building blocks in [CLI, scripting, agents, extensibility](decisions.md#cli-scripting-agents-extensibility) |

## The TUI

| # | Slice | Once it lands, you can… | Delivers |
|---|---|---|---|
| 17 | Read-only TUI | run `nmail` with no arguments to browse the inbox and read messages with vim keys, the mouse and on-screen key hints. | Modal controls, key hints and mouse in [UX](decisions.md#ux) |
| 18 | Triage in the TUI | do the same four actions with a key from the message list or the reader, through the same actions as the CLI. | "The CLI can do everything the TUI can" in [CLI, scripting, agents, extensibility](decisions.md#cli-scripting-agents-extensibility), configurable keys in [UX](decisions.md#ux) |
| 19 | Groups in the TUI | browse mail by group, across all accounts or one. | Unified views in [Accounts](decisions.md#accounts-and-providers), "Each email belongs to exactly one group" in [Classification](decisions.md#classification-jev) |
| 20 | Compose in the TUI | write, reply and forward in a modal, Neovim-like editor with relative line numbers and live Markdown highlighting, or in `$EDITOR`. | "Drafting feels like Neovim" and Markdown drafts in [UX](decisions.md#ux) |
| 21 | Offline spelling and grammar | see spelling and English grammar problems as diagnostics while drafting, with `]s`, `z=` and `zg`, without anything leaving the machine. | Spelling and grammar checks in [UX](decisions.md#ux) |
| 22 | Picker and live search | open a fuzzy picker that streams matching emails as you type, across all mail or inside the draft. | Picker building block and first pickers in [Search and pickers](decisions.md#search-and-pickers) |
| 23 | Scopes and more pickers | switch scope (thread, group, account, everything) without retyping, and pick senders by frecency, folders, accounts and keymaps. | Scopes and next pickers in [Search and pickers](decisions.md#search-and-pickers) |
| 24 | Motion and themes | see the live sorting animation (Decode with the confidence bar) as jev sorts, small transitions elsewhere, and pick a theme. | Live sorting animation and theming in [UX](decisions.md#ux), [refined motion](decisions.md#dec-2026-10-10-motion) |
| 25 | Sound | hear a few tasteful sounds when mail is sent or arrives, and turn them off. | [A few thoughtful sounds](decisions.md#dec-2026-10-11-sound) |
| 26 | Attachments in the TUI | see a message's attachments in the reader, save or open one with a key (through the CLI's attachment actions), preview images, text and PDF text in the terminal, and search attachments with a picker over all accounts. | Save, open, previews and the attachments picker in [Images and attachments](decisions.md#images-and-attachments) |
| 27 | Images in messages | see inline images in the terminal, load remote images with one key, for this message or always for this sender, and open the message in the browser with a key. | Images, remote images and "open in a browser" in [Images and attachments](decisions.md#images-and-attachments) |
| 28 | Drafting with files | attach files with `:attach` and a file picker, by pasting or dropping a path, or by pasting an image, with images previewed inline in the composer. | Drafting with files and images in [Images and attachments](decisions.md#images-and-attachments) |
| 29 | Lua keymaps and UI | add your own keymaps and UI in Lua, built from the same buffers, windows and decorations as the built-in views. | Keymaps, UI and the building blocks in [CLI, scripting, agents, extensibility](decisions.md#cli-scripting-agents-extensibility) |

## Release

| # | Slice | Once it lands, you can… | Delivers |
|---|---|---|---|
| 30 | Docs site, packaging and alpha | install a tagged first alpha on Linux, macOS or WSL from a simple guide, with a docs site that explains setup step by step. | Installation, docs site and logo in [Project and delivery](decisions.md#project-and-delivery), first release in [Platforms](decisions.md#platforms) |

## Why this order

- The CLI comes first because the owner chose to finish every CLI feature before starting the TUI
  ([DEC-2026-10-11-cli-first](decisions.md#dec-2026-10-11-cli-first)). The CLI can do everything
  the TUI can, through one action registry
  ([CLI, scripting, agents, extensibility](decisions.md#cli-scripting-agents-extensibility)), so
  each TUI slice puts keys and screens on actions that already work.
- Sign-in comes right after the inbox because Gmail and Outlook are providers "from day one" and
  secrets belong only in the keyring or a password command
  ([Accounts and providers](decisions.md#accounts-and-providers)).
- Triage comes before jev because jev's groups are written as provider tags and confident spam is
  moved to Junk, both of which use the write path triage builds
  ([Classification](decisions.md#classification-jev)). Redaction comes before the first call,
  because text reaches jev only after it ([DEC-2026-10-11-redaction-default](decisions.md#dec-2026-10-11-redaction-default)).
- Agents follow sending, search and attachments in the CLI. The send-approval rule needs sending to
  exist ([DEC-2026-10-11-agent-send](decisions.md#dec-2026-10-11-agent-send)), and agents use the
  CLI's search sources and attachment commands
  ([Search and pickers](decisions.md#search-and-pickers),
  [Images and attachments](decisions.md#images-and-attachments)).
- Lua keymaps and UI wait for the TUI, because they are built from its buffers, windows and
  keymaps ([CLI, scripting, agents, extensibility](decisions.md#cli-scripting-agents-extensibility)).
- The sorting animation follows jev because it shows jev's sorting, and sound follows
  compose, since sending is one of its moments ([UX](decisions.md#ux)).
- The attachments picker follows the first pickers because it is one of the next ones, and
  drafting with files follows compose and the file picker
  ([Images and attachments](decisions.md#images-and-attachments)).
