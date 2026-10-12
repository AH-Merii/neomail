# Research: references-jev-superlogical-pi

> Collected 2026-10-09 by research agents for neomail's design. Facts carry a confidence
> level and a source; re-verify anything time-sensitive before relying on it.

## 1. "jev" is TypeSafe AI's **Jev** decision model (about 97% sure)
- **What it is.** Jev is a "System One" or *decision* model from TypeSafe AI. It went into early access on 2026-09-15. It takes text or JSON `state` plus typed questions and returns probabilities, not text. The question types are `noul` (yes/no), `choice` (one of up to 255 labels) and `score` (2–10 ordered levels). Choice and score answers also carry a `confidence`. It costs $0.042 per million input tokens, and output is free. A request can be up to 64k tokens, of which `state` plus the longest question can use 32k. Input is text only.
- **"jev-style input"** means the System One wire format: `POST {base}/v1/systemone` with body `{model, state, questions:{id:{type, instructions, criteria}}}`, which returns `{model, answers, usage}`. Other backends adopted this format within weeks:
  - Cloudflare Clef / Clef-flash: open weights, and you switch from Jev by changing only the endpoint and model.
  - Ollama 0.35 (local `/v1/systemone`)
  - Kev (open source, self-hosted)
  - OpenRouter (`/api/v1/systemone`)
  - Liquid d1 (`/decisions/v1/systemone`)
  - OpenAI's Decisions API (`/v1/decisions`) is similar but uses a different schema, so it needs an adapter.

  This matches the spec's "pluggable backend" wording almost exactly.
- **Other readings, all unlikely:**
  - Kev, the open Jev clone (about 2%).
  - The OpenAI Decisions API or GPT-6 Luna (under 1%).
  - Mishearings such as Gemma, GEPA, JEPA, Jan, Jeeves (which is also the name of a small System One model), jq or "JSON events" (under 1%). None of them fits "model that takes X-style input".
- **Local machine:**
  - Nothing named `jev` is on PATH, and no file matching `*jev*` exists in `~/.local/bin`, `~/.cargo/bin`, the mise installs or `~/Projects` (searched 3 levels deep). `~/go/bin` doesn't exist.
  - `~/Projects/sortmail` contains only `.env`. I did not read it.
  - Ghostty 1.3.1 is the active terminal, and `/usr/bin/opencode` is installed (OpenCode resells `jev-1.13-free`, according to Pi's docs).

## 2. "superlogical" is Mitchell Hashimoto's company Superlogical; its product is **Rex**
- Superlogical was announced on 2026-07-29 and is built on libghostty. Rex is a terminal plus server-side multiplexer: the server owns sessions, and the app, CLI and scripts are all clients of it. Public testing started on 2026-10-05, for macOS only, so the user can't run it on Linux yet.
- Hashimoto, 2026-09-14: *"Everything you can do in the GUI, you can do via the CLI. Actually, you can do more in the CLI."*
- **How the `rex` CLI gets GUI parity:**
  1. **One API underneath everything.** `rex api list|describe|call` exposes every server method with JSON-Schema input and output plus examples. The CLI and the Lua API are derived from it, and `--json` output is meant to give coding agents the exact API shape.
  2. **Named, typed actions.** `rex do <action> k=v` runs either client actions (dotted names like `pane.split`, which need the app running with *Remote Control* turned on) or custom Lua actions. A custom action is defined once (`rex.action{name,title,args,run}`) and works from a key, the command palette, the CLI, the API or a script. `ctx.origin` tells it which one called it. `rex actions` lists them all.
  3. **Context and targets.**
     - Every terminal gets `REX_SESSION`, `REX_BLOCK` and `REX_SERVER` in its environment, so commands act on "here" by default.
     - `-s/-w/-b/-C/-S` pick another target. A target can be a typed ID like `block:<uuid>`, a label, a position or an ID suffix.
     - An ambiguous target exits with status 3 and lists what matched.
  4. **Machine contract.**
     - `--json` everywhere. Output is pretty on a terminal and one line when piped.
     - Commands that create something return its new IDs.
     - Exit codes are documented: 0, 1, 2, 3, 4 (server unreachable) and 130. `rex wait` and `run --wait` pass through the child's exit status.
  5. **Input, output and time.** `send` and `send-key` type into a terminal, `capture` reads the screen (text, HTML or VT), and `wait --for 10m` waits with a timeout.
  6. **Events.** `rex events --json` streams JSONL. Lua scripts can use `rex.on`, `rex.wait`, `--all` and `--fail-fast`.
  7. **Config.**
     - Lua 5.1 in `~/.config/rex/init.lua`, with `rex.bind`, `rex.mode` (key modes, with `exclusive` and `once`), `rex.action` and `rex.host`.
     - `rex config check` validates without applying, `rex config reload` reloads (the file is not watched), and `rex keymap` shows each binding with its file and line.
     - The docs are written for agents too: every page is available as `.md`, plus `llms.txt`, `llms-full.txt` and `search.json`.
- Ghostty is the ancestor of this design: every config key works as a CLI flag, `+action` subcommands, 85 named keybind actions, and in 1.3 an AppleScript preview and key tables.

## 3. "PI for ai agents" is **Pi** by Mario Zechner, now at Earendil
- The repo is now `earendil-works/pi` (badlogic/pi-mono redirects there) and the npm package is `@earendil-works/pi-coding-agent`. Version 1.0.0 shipped 2026-10-01 and 1.1.0 on 2026-10-07.
- **Customization layers, smallest first:**
  - AGENTS.md, SYSTEM.md and APPEND_SYSTEM.md
  - Prompt templates: Markdown files that become `/name` commands with `$1`, `$@` and `${1:-default}` substitution.
  - Skills, following the Agent Skills `SKILL.md` spec. Only the name and description go into the prompt; the full skill loads on demand or via `/skill:name`.
  - Extensions
  - Themes: JSON with a schema. The active user theme hot-reloads.
  - Packages: `pi install npm:/git:/./path`, a `pi` manifest in `package.json`, and a gallery keyword.
  - Locations: `~/.pi/agent/…` for user files and `.pi/…` for projects. Project resources load only after the user grants "project trust".
- **Extensions** are TypeScript modules loaded with jiti, so there is no build step. Their default factory receives `ExtensionAPI` and can:
  - Hook events with `pi.on`: lifecycle, `before_agent_start` (prompt and tools), `context` rewrites, `tool_call` (mutate or block), `tool_result`, input and provider streams.
  - Register things: `registerTool` (TypeBox schema, renderers, exposure, annotations), `registerCommand`, `registerShortcut`, `registerFlag`, `registerProvider`, `registerMcpServer` and `registerVirtualModel`.
  - Build UI through `ctx.ui` (dialogs, status, widgets, custom components).
  - Keep state with `appendEntry` or tool `details`, and talk to other extensions over `pi.events`.
  - `/reload` swaps the extension runtime in place.
  - The same extensions run in TUI, print/JSON, RPC (JSONL over stdio) and SDK modes.
- **Philosophy:** a minimal core with "primitives, not features". You "ask the agent to extend itself" and it loops on write, reload, test. Pi's system prompt lists the paths to Pi's own docs and examples, with a topic-to-file routing table, so the agent can write extensions. There is no sandbox: extensions have full process permissions.

## Recommendations for neomail (details in the recommendations list)
Build one typed command core with a self-describing schema. The TUI, the CLI, Lua scripts and agents all call it, as Rex does. Classify mail through a `DecisionBackend` trait that speaks the System One format. Use Lua (mlua) for extensions, with generated type stubs and docs shipped for agents, in Pi's style.

*Note: curl briefly wrote and then deleted `/tmp/sl_page.txt`. No other files were touched.*

## Key facts

- **[high]** Jev is TypeSafe AI's 'System One' decision model: you send state plus typed questions (noul/choice/score) and get back probabilities and confidence, not text. — <https://docs.typesafe.ai/api.md>
- **[medium]** Jev went into limited early access on 2026-09-15. Simon Willison wrote about it on 2026-09-21 and used the llm-typesafe plugin. — <https://simonwillison.net/2026/Sep/21/jev/>
- **[high]** Wire format: POST https://api.typesafe.ai/v1/systemone with a Bearer key. Body is {model:'jev-latest', state, questions:{id:{type,instructions,criteria}}}; response is {model, answers{id:{type,choice|score|noul,probabilities,confidence,legend}}, usage}. Errors: 401, 422, 429, 529. — <https://docs.typesafe.ai/api.md>
- **[high]** Limits: up to 255 options per Choice; Score has 2–10 levels; 64k tokens per request and 32k for state plus the longest question; text-only input. — <https://docs.typesafe.ai/models.md>
- **[high]** Price is $0.042 per million input tokens and output is free. Rate limits are 100K tokens/s and 80 requests/s, and TypeSafe says they are 'adjusting dynamically'. Aliases jev-latest and jev-preview both point to jev-1.13.0; pin the version if your thresholds are tuned to it. — <https://docs.typesafe.ai/models.md>
- **[high]** Known weak spots in Jev 1.13: literal reading, numbers and dates, large states full of irrelevant detail, adversarial content (prompt injection), and a bias toward the first Choice option. — <https://docs.typesafe.ai/model-jaggedness/jev-1.13.md>
- **[high]** Cloudflare Clef and Clef-flash (2026-10-01; Apache-2.0 weights on Hugging Face) follow the System One API: 'switch an existing Jev integration to Clef by changing the endpoint and model'. Up to 64 questions per request. — <https://developers.cloudflare.com/changelog/post/2026-10-01-clef-workers-ai/>
- **[high]** Ollama v0.35.0 (2026-09-28) added local /v1/systemone 'based on TypeSafe's Jev API', with the nimble and tev1 models, and later clef. — <https://github.com/ollama/ollama/releases/tag/v0.35.0>
- **[high]** Kev (jaredpalmer/kev) is a family of Apache-2.0 decision models from 0.8B to 27B. It serves /v1/systemone (example port 8009), and the TypeSafe Python SDK works against it unchanged. — <https://github.com/jaredpalmer/kev>
- **[high]** OpenRouter serves Jev at /api/v1/systemone (works with the TypeSafe SDK) and at /api/alpha/decisions. Model IDs are typesafe/jev-1.13 and ~typesafe/jev-latest, with a 32k context. — <https://openrouter.ai/docs/guides/community/jev>
- **[medium]** Liquid AI's d1 decision model uses the endpoint https://api.liquid.ai/decisions/v1/systemone. — <https://www.liquid.ai/blog/d1-decision-model>
- **[high]** OpenAI's Decisions API (POST /v1/decisions, model gpt-6-luna, public beta) uses a different schema: an input field and a questions array with names and predicate/choice/score types. It needs an adapter. — <https://developers.openai.com/api/docs/guides/decisions>
- **[high]** Several unofficial Rust clients exist (jev 0.1.2, judgment 0.12.0, kunobi-decision 0.3.0, system-one 0.1.0, typesafe-ai-sdk 0.5.0). All were created in Sep–Oct 2026 and have fewer than about 800 downloads. — <https://crates.io/crates/kunobi-decision>
- **[high]** Pi's pi-ai treats Jev, Clef and GPT-6 Luna as 'classifier' models behind one normalized choice/score/bool API, with providers typesafe, openrouter, cloudflare-workers-ai, vercel-ai-gateway, opencode and openai. — <https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/models.md>
- **[high]** Local check: no `jev` on PATH and no *jev* files in ~/.local/bin, ~/.cargo/bin, the mise installs or ~/Projects. ~/Projects/sortmail contains only .env, which was not read.
- **[high]** Superlogical is Mitchell Hashimoto's company, announced 2026-07-29. It builds on libghostty and starts with a terminal multiplexer. Co-founders are Jack Pearkes, Alasdair Monk and Hector Simpson. — <https://mitchellh.com/writing/superlogical>
- **[medium]** The product is Rex. Public testing began 2026-10-05 with the macOS app only; Linux and Windows servers run internally, and those clients are still in development. — <https://www.superlogical.com/updates/public-testing-beginning>
- **[high]** Hashimoto, 2026-09-14: 'Everything you can do in the GUI, you can do via the CLI. Actually, you can do more in the CLI.' He also mentions a streaming event system and a JSON process API. — <https://x.com/mitchellh/status/2099622049325232505>
- **[high]** Rex CLI: `rex` is on PATH in every terminal, with REX_SESSION, REX_BLOCK and REX_SERVER as context. Targets are chosen with -s/-w/-b/-C/-S and can be a typed ID, label, position or ID suffix; an ambiguous target exits with status 3. --json is available, and exit codes are 0, 1, 2, 3, 4 and 130. — <https://www.superlogical.com/rex/docs/automate/cli>
- **[high]** `rex api list|describe|call` exposes every server method with JSON Schema input/output and examples. Per the docs, 'the CLI and Lua APIs all derive dynamically from the server API', and the --json output is useful for giving a coding agent the exact API shape. — <https://www.superlogical.com/rex/docs/automate/server-api>
- **[high]** Rex actions: `rex do <action> k=v` runs either client actions (dotted names, which need the app running with Remote Control on) or Lua custom actions with typed args. One definition works from a key, the palette, the CLI, the API or a script, and ctx.origin says which. — <https://www.superlogical.com/rex/docs/customize/actions>
- **[high]** Rex events stream as JSONL via `rex events --json`; Lua scripts use rex.on and rex.wait. Waiting with timeouts uses `rex wait --for`, and `run --wait` passes through the child's exit status. — <https://www.superlogical.com/rex/docs/reference/events>
- **[high]** Rex config is a Lua 5.1 file at $XDG_CONFIG_HOME/rex/init.lua. `rex config check` validates it, `rex config reload` applies it (no file watching), and `rex keymap` shows each binding's source line. Key modes support exclusive and once. — <https://www.superlogical.com/rex/docs/customize/config>
- **[high]** Rex docs are published for agents: llms.txt, llms-full.txt, every page as .md, and search.json. — <https://www.superlogical.com/rex/docs/llms.txt>
- **[high]** Ghostty 1.3.0 (2026-03-09) added an AppleScript preview, key tables and +new-window -e on GTK. The locally installed Ghostty 1.3.1 lists 85 keybind actions. — <https://ghostty.org/docs/install/release-notes/1-3-0>
- **[high]** Pi now lives at github.com/earendil-works/pi with npm package @earendil-works/pi-coding-agent. Version 1.0.0 shipped 2026-10-01 and 1.1.0 on 2026-10-07. — <https://www.npmjs.com/package/@earendil-works/pi-coding-agent>
- **[high]** Pi's ExtensionAPI provides pi.on events (before_agent_start, context, tool_call mutate/block, tool_result, input, provider_stream_event, session_*), plus registerTool (TypeBox), registerCommand, registerShortcut, registerFlag, registerProvider, registerMcpServer, registerVirtualModel, renderers, ctx.ui, appendEntry and pi.events. — <https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/extensions.md>
- **[high]** Pi skills follow the Agent Skills SKILL.md spec and load on demand (/skill:name). Prompt templates are Markdown files that become /commands. Themes are JSON with a schema. /reload hot-reloads resources. — <https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/skills.md>
- **[high]** Pi packages install via `pi install npm:|git:|./path`. A package declares its resources in package.json under a 'pi' key (extensions, skills, prompts, themes). Project resources load only after project trust is granted. — <https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/packages.md>
- **[high]** Pi's system prompt includes the paths to Pi's own docs and examples, with a topic-to-file routing table, so the agent can extend Pi itself. — <https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/core/system-prompt.ts>
- **[high]** Pi's philosophy: 'if you want the agent to do something that it doesn't do yet, you ask the agent to extend itself', with hot reload so the agent can write, reload and test in a loop. — <https://lucumr.pocoo.org/2026/1/31/pi/>
- **[high]** Pi has no built-in sandbox or permission system; extensions run with the full permissions of the Pi process. — <https://github.com/earendil-works/pi>

## Recommendations

- Jev backend: write a small in-house reqwest/serde client for POST {base}/v1/systemone behind a `DecisionBackend` trait; the protocol is one endpoint. Ship presets for TypeSafe (api.typesafe.ai, jev-1.13.0), OpenRouter (https://openrouter.ai/api, typesafe/jev-1.13), Ollama (localhost:11434, nimble/tev1/clef), Kev (localhost:8009) and Liquid d1. Add adapters for Cloudflare's REST `result` wrapper and OpenAI /v1/decisions. Normalize answers to choice/score/bool, as pi-ai does.
- Classification: send one Choice question per email whose criteria map group names to descriptions (at most 255). Add Nouls for flags such as needs_reply or newsletter in the same call. Gate on confidence: low-confidence mail goes to an 'Unsorted / review' queue. Store the full probability vector, the model ID and a timestamp in SQLite so thresholds can be retuned without re-calling. Only then write Outlook categories, Gmail labels or Proton labels.
- Build the state from the headers you need (from, to, list-id, subject) plus HTML-stripped, truncated body text. Do date and number logic in code. Treat email as adversarial input because injected text can move answers. For low-confidence cases, reorder the options and check the answer stays the same, since Jev favors the first option.
- Privacy: make the backend explicit per account, and offer a local default (Ollama or Kev) for Proton users. TypeSafe does not train on requests, but zero data retention is enterprise-only.
- CLI parity, modeled on Rex: build a core engine that owns accounts, the SQLite cache, sync and classification, and make the TUI just one client. Every keybinding maps to a named, typed action (e.g. thread.archive, message.move, group.assign). `neomail do <action> k=v` runs any of them and `neomail actions` lists them.
- Self-describing API: offer `neomail api list|describe|call` backed by JSON Schema (e.g. the schemars crate) with request/response examples. Generate the CLI's clap subcommands, the Lua bindings and the docs from that single registry so they cannot drift.
- Targets and IDs: use typed, prefixed, never-reused IDs (acct:, mbox:, thr:, msg:, grp:). Accept a label, position or ID suffix as a target, and exit with status 3 when a target is ambiguous. Commands spawned from the TUI (editor, pager, shell) should get NEOMAIL_ACCOUNT, NEOMAIL_THREAD and NEOMAIL_MESSAGE so they act on 'here' by default.
- Machine contract: support --json everywhere (pretty on a terminal, one line when piped), make create commands return their IDs, and document exit codes (0/1/2/3/4 unreachable/130). Add `neomail events --json` as a JSONL stream (new_mail, classified, sync_*, label_changed) and `neomail wait --for` / `sync --wait`.
- Config: use Lua in ~/.config/neomail/init.lua with neomail.bind, neomail.mode (vim modes as key tables with exclusive/once) and neomail.action. Add `neomail config check`, `neomail config reload` and `neomail keymap` (showing each binding's source file and line). Only let the CLI drive UI actions in a running TUI when the user turns on a Remote Control setting.
- Extension runtime: use embedded Lua via mlua (Luau if you need sandboxing, LuaJIT for speed). It matches Rex and Neovim and hot-reloads easily. Generate LuaLS `---@meta` type stubs from the API schema so agents get types. Alternatives: TypeScript through rquickjs (closest to Pi; Pi's codemode already uses a QuickJS sandbox), or wasmtime/WIT components later for untrusted third-party packages.
- Pi-style layers: support an AGENTS.md-style context file, prompt templates (for AI drafting and replies), Agent Skills (SKILL.md), Lua extensions (actions, hooks such as on_message_received/before_label_write, commands, classifier-rule providers, status widgets), JSON themes with a schema, and packages installed from git or a local path, with trust prompts.
- Docs written for agents: ship the docs and examples inside the binary (`neomail docs <topic>` as Markdown, plus llms.txt). Point agents at them with a topic-to-file routing table like Pi's system prompt. Add `neomail ext new` scaffolding, a test harness against a fixture SQLite mailbox, --dry-run for remote mutations, per-extension capabilities (read, label, send, network), readOnly/destructive annotations that trigger confirmation, and an undo journal.
- Optionally emit OSC 7501 program-status sequences (working while syncing, blocked when waiting on OAuth) so Rex and other supporting terminals can show neomail's state.

## Open questions

- Confirm with the user that 'jev' means TypeSafe's Jev. Which backend should be the default, hosted or local, especially for Proton Mail? Do they already have a TypeSafe, OpenRouter or OpenCode key? (~/Projects/sortmail/.env exists but was deliberately not read.)
- Can an email belong to one group or several? That decides between a single Choice question and one Noul per group, and affects how labels and categories are written back.
- Should the neomail CLI always talk to a running engine or daemon, as Rex does, or fall back to working directly on SQLite and the provider APIs when no TUI or daemon is running?
- Which extension language: Lua, matching Rex and Neovim, or TypeScript, matching Pi? Does the user expect to install third-party extensions, which would justify WASM sandboxing?
- Rex is in a macOS-only public beta and the user is on CachyOS Linux, so the reference can only be studied through its docs (superlogical.com/rex/docs), not run locally yet.
- Jev access: TypeSafe says rate limits are 'adjusting dynamically', and one secondary source reports new signups were paused on 2026-09-22. Confirm the account and its quota before relying on Jev as the default.
