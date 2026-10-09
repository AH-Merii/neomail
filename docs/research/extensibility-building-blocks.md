# Research: extensibility building blocks

> Collected 2026-10-09 by a research agent for neomail's architecture synthesis. Facts carry a
> confidence level and a source; re-verify anything time-sensitive before relying on it. Source code
> was read at these revisions: snacks.nvim `882c996` (2026-05-25), blink.cmp v1.10.2 (tag commit
> `78336bc`, 2026-04-04; `main` was at `8219b58`, 2026-09-20), frizbee 0.13.0 (tag commit `d608923`;
> `main` was at `5fa0fa4`, 2026-09-18), Helix `master` at `ba40e54` (2026-09-29, workspace version
> 25.7.1; the latest *release* is 25.07.1 from 2025-07-18), nucleo `8c16d47`, Neovim docs at
> nightly 2026-10-09 (latest stable: Nvim 0.12.6, released 2026-10-09).

## Summary

Neovim and Helix get their extensibility from **a few primitives that are general, cheap, and owned
by the core**. Features are thin compositions of them:

- **Neovim:** buffers, windows (floats are just windows), extmarks in namespaces, autocmds, keymaps
  and highlight groups. Diagnostics, LSP, Treesitter highlighting, `vim.ui.select` and every plugin
  build on the same handful of APIs.
- **snacks.nvim:** one `win` abstraction and one `layout` box tree. On top of them sit one picker
  pipeline (finder, matcher, sorter, format, preview, actions) plus `notifier` and `input`. It ships
  68 picker sources (entries in `config/sources.lua` at `882c996`), and nearly all of them are just
  config tables.
- **blink.cmp:** completion is a provider registry with a small `Source` interface plus
  per-provider policy (timeouts, fallbacks, score offsets). Results are cached, and typing only
  re-filters them unless the source says its list is incomplete.
- **Helix:** one `Compositor` that stacks `Component` layers (`handle_event`, `render`,
  `required_size`, `cursor`). Picker, completion, hover, signature help and prompts are components
  wrapped in `Popup` or `Overlay`. A typed event and hook system with debounced `AsyncHook` workers
  keeps the UI thread fast.

**For neomail this means:**

- Buffers that own content plus marks.
- Windows (floats included) that own the viewport.
- A Helix-style compositor of layers.
- Decorations stored in interval trees and keyed by namespace.
- A typed event bus with sync hooks and debounced async hooks.
- Keymap scopes resolved through a trie.
- Typed provider registries that Rust and Lua register into on equal terms.

Matching runs on worker threads (nucleo-style). Rendering touches only visible rows. The daemon owns
data and sources. The TUI owns buffers, windows and decorations.

---

## 1. Neovim: the primitives and why they compose

### 1.1 Buffers, windows, floats, tabpages

- **A buffer is content plus metadata**:
  - text lines, buffer-local options, buffer-local keymaps (`nvim_buf_set_keymap`), `b:`
    variables, extmarks, and an attached LSP/Treesitter state;
  - **`buftype`** says what kind of buffer it is: empty (normal file), `acwrite`, `help`, `nofile`,
    `nowrite`, `prompt` (only the last section is editable, for plugins), `quickfix`, `terminal`.
  - Plugins build UIs as `nofile` scratch buffers with `modifiable=false` and buffer-local keymaps.
    That is exactly the "every view is a buffer" model in decisions.md.
- **A window is a viewport onto a buffer.** It owns the cursor, the scroll position (`topline`) and
  window-local options (`number`, `relativenumber`, `wrap`, `conceal*`, `winhighlight`). Many
  windows can show one buffer, each with its own cursor. Tabpages are collections of window
  layouts.
- **Floating windows are ordinary windows** created by `nvim_open_win` with a `relative` anchor:
  - anchors: `editor`, `win`, `cursor`, `mouse`, ...;
  - options: an `anchor` corner (NW/NE/SW/SE), `zindex` (default 50), `focusable`, `border`,
    `title`/`footer`, `style="minimal"` and `hide`;
  - because a float holds a normal buffer, every buffer feature works in it unchanged: extmarks,
    keymaps and highlights.
  - Hover, diagnostics floats, completion docs, notifications and pickers are all buffer-in-float.

### 1.2 Extmarks and namespaces

- **Extmarks are positions between characters that track edits** (forward gravity by default). One
  mark can carry any of these decorations:
  - a highlight range (`hl_group`, which can be stacked);
  - virtual text: `virt_text` at `eol`, `eol_right_align`, `overlay`, `right_align` or `inline`,
    plus `virt_lines`;
  - signs: `sign_text` and `sign_hl_group`, plus `number_hl_group` and `line_hl_group`;
  - `conceal`, `url`, `spell`, `priority`, and `ephemeral`.
- **Namespaces** (`nvim_create_namespace`) group marks by owner, so a producer can clear and redraw
  only its own marks over a line range (`nvim_buf_clear_namespace`). Highlight namespaces
  (`nvim_win_set_hl_ns`) let one window restyle groups without touching global state.
- **The storage is a B-tree (`marktree.c`).** It is derived from klib's kbtree and inspired by Atom's
  marker tree. Insertion, `splice` on text change, and positioned iteration are all logarithmic. A
  text edit updates mark positions without touching every mark.
- **Decoration providers** (`nvim_set_decoration_provider`) are the redraw-time hook:
  - Callbacks: `on_start`, `on_buf`, `on_win(toprow, botrow)`, `on_range` (which replaces the
    deprecated `on_line`) and `on_end`.
  - They run **only for windows being redrawn** and only for visible ranges. They set `ephemeral`
    marks that exist for one frame.
  - Returning `false` from `on_start` or `on_win` skips work. The docs ask plugins with many
    decoration sources to register **one** provider and merge internally.
  - Removing or updating extmarks inside `on_range`/`on_line` is forbidden.
  - Treesitter highlighting uses this path. Highlighting cost is therefore proportional to the
    visible screen, not to file size.

### 1.3 Autocmds, user commands, keymaps

- **Autocmds** are `(event, pattern-or-buffer, group) -> callback`.
  - Events cover buffer and window lifecycle (`BufEnter`, `WinNew`), editing (`TextChanged`,
    `InsertCharPre`), idle (`CursorHold`) and plugin-defined events (`User` with a pattern).
  - `augroup`s make a plugin's hooks clearable as one unit, which is what makes hot reload safe.
  - Buffer-local autocmds die with the buffer.
- **User commands** (`nvim_create_user_command`) and **keymaps** (`vim.keymap.set` with `buffer=`
  and `desc=`) point at the same Lua callbacks.
  - `desc` feeds which-key style hints and keymap pickers.
  - Neovim has no separate action registry. Keymaps and commands are the registry, which is why
    pickers can list them. neomail's typed action catalogue is a stronger version of this.

### 1.4 Diagnostics, completion, `vim.ui`, highlights

- **`vim.diagnostic` splits producers from consumers.**
  - **Producers** call `vim.diagnostic.set(ns, bufnr, diagnostics)` under their own namespace, for
    example one per LSP client or linter.
  - **Consumers** are *handlers*: `virtual_text`, `virtual_lines`, `signs` and `underline`, each
    built on extmarks. A plugin adds one by inserting a key into `vim.diagnostic.handlers`, and can
    wrap built-in handlers.
  - Navigation (`jump`), `open_float` and quickfix export work for every producer.
  - This is the model neomail's spelling and grammar checks need.
- **Completion:**
  - The `'complete'` option lists sources. `F{func}` adds completion functions as sources, and
    several are allowed.
  - Nvim 0.12 added an `'autocomplete'` option (default off).
  - LSP completion goes through `vim.lsp.completion`.
  - The popup menu is a core UI element (with `ext_popupmenu` for external UIs). Plugins such as
    blink.cmp replace it with their own float.
- **`vim.ui.select` / `vim.ui.input`** are *swappable interfaces*. Core features call them (in
  nightly/0.13-dev, `z=` spelling suggestions and `:tselect` now delegate to `vim.ui.select`; this is
  not in 0.12.6), and a plugin replaces the implementation. snacks sets
  `ui_select = true` to route every `vim.ui.select` call into its picker. Callers pass
  `format_item`, `kind` and optionally `preview_item`.
- **Highlight groups are semantic names with links**:
  - `nvim_set_hl(0, "DiagnosticUnderlineError", {...})`, or `link=`;
  - plugins define `default = true` links to standard groups, so any colourscheme themes them
    without knowing the plugin exists.

### 1.5 Process model

- Since 0.9 the built-in TUI runs **in a separate process** and talks to the core over the msgpack-RPC
  UI protocol.
- The `ext_*` UI extensions (`ext_messages`, `ext_cmdline`, `ext_popupmenu`, `ext_multigrid`) let a
  UI draw those elements natively.
- The core owns all state and all Lua. The UI is a renderer that sends input. This is the same
  split neomail already has between daemon and TUI, but with one big difference: Neovim has **one
  core per editing session**, while neomail's daemon serves many clients (see §5.9).

### 1.6 The cost model: why it is fast

1. **Visible-only work.**
   - Redraw is lazy: changes mark lines or windows "dirty", and one redraw per event-loop turn
     coalesces them.
   - Decoration providers run only for the rows being drawn.
2. **Logarithmic marks.**
   - Edits splice the mark tree instead of rewriting marks.
   - Namespaces make bulk clear proportional to the range cleared.
3. **One-frame (ephemeral) decorations** for derived data such as syntax, so nothing is stored or
   invalidated.
4. **Direct API calls.** Lua calls C API functions through LuaJIT with no IPC. `api-fast`
   functions are safe inside libuv callbacks, and anything else is deferred with `vim.schedule`.
5. **Event granularity.**
   - Expensive reactions hang off `CursorHold` or debounced timers, not `CursorMoved`.
   - Autocmd dispatch is indexed by event.
6. **Batching at the UI boundary.** Grid updates are sent as batched `redraw` notifications, and
   only the cells that changed are flushed.

---

## 2. snacks.nvim (folke)

### 2.1 Few primitives, many features

- **`Snacks.win`** is one window abstraction. It covers floats and splits (`position = "float" |
  "bottom" | "top" | "left" | "right" | "current"`). It owns:
  - sizing: relative or absolute, `min_`/`max_` width and height, or functions;
  - `border`, `backdrop`, `footer_keys`;
  - buffer- and window-scoped `keys`, `events` (autocmds) and `actions`;
  - a named **style** looked up in `Snacks.config.styles[...]`.
- **Each feature registers a style.** `float`, `split`, `help`, `minimal`, `input`, `notification`,
  `notification_history`, `terminal`, `scratch`, `zen`, `dashboard`, `lazygit`, `blame_line` and
  `snacks_image` are all styles over the same window. The modules built on `Snacks.win` are input,
  notifier, terminal, scratch, zen, dashboard, image, layout and every picker sub-window.
- **`Snacks.layout`** is a box tree of named windows (`box = "horizontal" | "vertical"` with
  children). Windows can be hidden or toggled, and the tree is resolved to screen coordinates.
- **The picker composes three windows** (`input`, `list`, `preview`) in a layout. Each window shares
  the picker's `actions`, so any key in any sub-window can run any picker action.

### 2.2 The picker pipeline

The picker is configured by one table, `snacks.picker.Config`:

| Stage | Field | Contract |
|---|---|---|
| Source | `source` | A named preset in `config/sources.lua`, merged into the config. Users override any field per source. |
| Finder | `finder` | `fun(opts, ctx) -> Item[] \| fun(cb)`. Either a list or an async producer that calls `cb(item)` per item. `ctx.filter.search` holds the *search* string for live finders. `multi = {...}` merges several finders. |
| Matcher | `matcher` | A fuzzy, exact, regex or field matcher over `item.text` and named fields, with frecency and cwd bonuses. |
| Sorter | `sort` | Default `fields = {"score:desc", "#text", "idx"}`: score, then text length, then insertion order. |
| Transform | `transform` | Filters or rewrites items after finding (for example `unique_file`). |
| Format | `format` | Item -> list of `{text, hl_group}` chunks: highlighted, icon-aware columns. |
| Preview | `preview` | A named previewer (`file`, `diff`, `git`, `man`, `preview`) that renders into the preview window. |
| Actions | `actions`, `confirm`, `win.*.keys` | Named functions `(picker, item)`. Keys map to action names, and actions can be chained. |
| Layout | `layout` | A preset name or a box tree. Layouts are themselves listable (`picker_layouts`). |

- **Search vs pattern.** `search` is what the *finder* gets. Changing it re-runs the finder, which
  is how live grep works (`live = true`, `<C-g>` toggles it). `pattern` is what the *matcher*
  applies to the items already found.
  - Live finders are capped by `limit_live` (default 10000).
  - This two-query split is exactly neomail's "live text search over all mail": the finder is a
    daemon FTS query, and the matcher refines the results locally.
- **Most of the 68 sources are configs, not code.** For example:
  - `grep` is `finder = "grep", format = "file", live = true`;
  - `smart` is `multi = {"buffers", "recent", "files"}` with frecency, a cwd bonus and
    `transform = "unique_file"`;
  - `spelling` is a finder over `vim.fn.spellsuggest`.
- **Meta-pickers list the registries themselves:** `pickers`, `picker_actions`, `picker_format`,
  `picker_layouts`, `picker_preview`, `keymaps`, `commands`, `highlights`, `autocmds`. Because
  everything is registered, everything is pickable.

### 2.3 Matcher and performance tricks (read from `core/matcher.lua`, `core/score.lua`, `util/async.lua`)

- **Scoring** (in `core/score.lua`, not `matcher.lua`) is a Lua port of fzf's algo constants: `SCORE_MATCH = 16`, gap start -3, gap
  extension -1, boundary bonus 8, camelCase bonus 7, consecutive bonus 4, and a first-character
  multiplier of 2.
  - Candidates are found with plain `string.find` per character before scoring.
  - Positions for highlighting are computed **lazily**, only for rendered rows (`M:positions`).
- **Incremental work:**
  1. Items that were in the previous **top-k** (a min-heap from `util/minheap.lua`, held by `core/list.lua`) are matched first, so the
     visible list refreshes immediately.
  2. Then items that matched last time.
  3. If the new pattern *extends* the old one (`subset`), items that failed last time are skipped
     outright.
  4. Then the rest.
- **Entropy ordering.** Each pattern part gets an "entropy" estimate: rare characters, exact, word
  and field patterns score higher. Parts are evaluated so that the least likely to match runs
  first, which short-circuits non-matches.
- **Cooperative time slicing, single-threaded.**
  - Finders and matchers are Lua coroutines that `yield` every 1 ms (`YIELD_FIND`, `YIELD_MATCH`).
  - A libuv `check` handle resumes them with a **10 ms budget** per event-loop turn
    (`Async.BUDGET = 10`).
  - Typing aborts the running task (`task:abort()`) and restarts it with a new tick.
  - A UI timer refreshes the list, and preview is throttled.
  - Empty patterns skip matching entirely (a fast path).
- **Frecency** is persisted in SQLite through LuaJIT FFI (`util/db.lua`) and boosts scores
  (`BONUS_FRECENCY = 8`, `BONUS_CWD = 10`).

### 2.4 Telescope and fzf-lua, briefly

- **Telescope:**
  - same decomposition: `finders` (static table, job, dynamic), `sorters` (generic, fzy, or the
    C-backed `telescope-fzf-native`), `previewers`, `entry_maker` (item to display and ordinal),
    `attach_mappings` (actions);
  - extensions register new pickers.
  - Matching and sorting run in Lua on the main loop, so large lists stutter without the native
    sorter. That cost is what snacks' coroutine budget and Helix's threaded nucleo avoid.
- **fzf-lua:**
  - Delegates matching to the external `fzf` binary (> 0.36). Lua builds the shell command or feeds
    a pipe, and fzf does matching, sorting and rendering in a terminal buffer.
  - Very fast on huge lists, but items are strings crossing a process boundary, previews are shell
    commands or Lua callbacks, and the UI is fzf's, not the editor's primitives.
- **Lesson for neomail:** the snacks shape (named sources, config-only pickers, its own window
  primitive, meta-pickers) gives the best extensibility. Speed comes from native threaded matching
  (nucleo/frizbee), not from Lua or an external process.

---

## 3. blink.cmp and frizbee

### 3.1 The source and provider model (`sources/lib/types.lua`, `config/sources.lua`)

- **A `Source` is a small object**:
  - `new(opts, config)`;
  - optional `enabled()`, `get_trigger_characters()`;
  - `get_completions(ctx, callback)`, which returns a cancel function;
  - `should_show_items`, `resolve(item, cb)` (lazy details), `execute(ctx, item, cb, default)`;
  - `get_signature_help*` and `reload`.
  - Everything is callback-based and cancellable.
- **The response is `{ items, is_incomplete_forward, is_incomplete_backward }`.** If a source
  returns a *complete* list, further typing only **re-filters the cached items in Rust**. The source
  is re-queried only when the response was incomplete in that direction. This avoids a round trip
  per keystroke.
- **The provider config is policy, separate from the source.** Per provider:
  - `module`, `enabled`, `opts`;
  - `async` (show other results without waiting) and `timeout_ms`;
  - `transform_items`, `should_show_items`, `max_items`, `min_keyword_length`;
  - `fallbacks` (if this provider returns nothing, use these), `score_offset`;
  - `override` (patch any source method).
  - Source lists can be `default` or `per_filetype`.
  - Most user customisation is policy, not new code.
- **One fuzzy core for all sources.**
  - The Rust module (mlua `module` + LuaJIT) receives items once.
  - It applies keyword-range matching, typo tolerance (`max_typos`), frecency (stored with
    blake3 + bincode) and proximity bonuses, then sorts.
  - `implementation = "prefer_rust_with_warning" | "prefer_rust" | "rust" | "lua"` falls back to
    Lua if the prebuilt binary is missing.
  - Note: blink.cmp v1.10.2's `Cargo.toml` pins `frizbee = "0.9.0"` and `mlua = "0.11.3"`, so the
    released plugin does not yet use frizbee 0.13.

### 3.2 frizbee 0.13.0 (MIT)

- **Algorithm:**
  - SIMD Smith-Waterman with affine gaps. Unlike greedy matchers it always finds the best
    alignment.
  - It supports insertion, deletion and substitution, so it is typo-resistant (`max_typos`).
- **Prefilter:** checks that every needle character occurs in order, loading the haystack in SIMD
  chunks and masking out the prefix that already matched. For a 32-byte haystack and 16-byte SIMD
  that is about 2 loads. Variants exist for 1, 2 and N typos.
- **Query syntax:** fzf-like: `'substr`, `^prefix`, `suffix$`, `^exact$`, `!negate`, with
  whitespace-separated multi-patterns. The API is `match_list` or
  `match_list_parallel(haystacks, threads)`, plus a radix sort.
- **Author's benchmark** (1.41M Chromium paths, needle `linux`, Ryzen 9950X3D):

  | Run | Nucleo | fzf | frizbee |
  |---|---|---|---|
  | Sequential | 90.5 ms | 120.8 ms | **22.4 ms** |
  | 8 threads | 36.8 ms | 16.4 ms | **3.5 ms** |

  It is vendor-run, so treat the numbers as indicative. The frizbee column is `Config::default()`
  with **typo tolerance off** (with 1 typo: 60.8 ms / 9.5 ms), and nucleo runs with Unicode
  normalisation disabled.
- **Unicode:** frizbee 0.13 has a UTF-8 path (prefilter on the final codepoint byte, one matrix
  row per codepoint), and its README claims about 20x over nucleo and fzf on Unicode haystacks.
  It is not ASCII-only.
- **Used by** blink.cmp, atuin, television, skim and fff.
- **Licence:** MIT, which suits neomail's MIT OR Apache-2.0 better than nucleo's MPL-2.0. MPL is
  file-level copyleft and usable as a dependency, but it is a licence-review item.

---

## 4. Helix

### 4.1 helix-view: Editor, Document, View

- **`Editor`** is the whole model:
  - `documents: BTreeMap<DocumentId, Document>`;
  - `tree: Tree` of `View`s (split layout), `mode`, `registers`, `count`, `macro_recording`;
  - `diagnostics`, the `language_servers` registry, `diff_providers`;
  - `theme`, `config: Arc<dyn DynAccess<Config>>` (hot-swappable), `status_msg`, `autoinfo`;
  - `idle_timer`, `needs_redraw`.
  - It knows nothing about the terminal. Rendering lives in helix-term.
- **`Document`** owns the text (`Rope`), history, diagnostics, syntax, and **selections keyed by
  `ViewId`**: each view's cursor state lives on the document, per view. Neovim keeps the cursor on
  the window instead.
- **`View`** is a viewport: `area`, `doc`, `jumps`, scroll offset, gutters, docs access history.
  - Changes are synced to views *lazily* through per-document revision numbers (`doc_revisions`).
  - The source has a "HACKS" comment (on the per-view `diagnostics_handler`) saying a proper fix
    needs View and Document refactored into an entity-component-like structure. That is a warning
    to keep neomail's per-view state on the window from day one.
- **Annotations and decorations:**
  - `TextAnnotations` in helix-core holds layered `InlineAnnotation`s (virtual text that takes up
    space), `Overlay`s (replacement glyphs) and `LineAnnotation`s (reserved virtual lines).
  - The term-side `Decoration` trait (`decorate_line`, `decorate_grapheme`, `render_virt_lines`)
    paints into the space those annotations reserve.
  - Inline diagnostics and inlay hints are both built this way.

### 4.2 helix-term: the compositor (`compositor.rs`)

- **`Component` trait:**
  - `handle_event(&Event, &mut Context) -> EventResult`, which is `Consumed` or `Ignored`, each with
    an optional `Callback = Box<dyn FnOnce(&mut Compositor, &mut Context)>`;
  - `render(area, &mut Surface, &mut Context)`, `cursor(area, &Editor)`;
  - `required_size(viewport) -> Option<(u16, u16)>`, `should_update()`, `id() -> Option<&'static str>`.
- **`Compositor`** is a `Vec<Box<dyn Component>>` of layers.
  - **Render** runs bottom to top, every frame, into one cell buffer. helix-tui diffs that buffer
    against the previous frame and flushes only the cells that changed.
  - **Input** goes top to bottom until a layer consumes it, so events bubble down.
  - Callbacks let a component push, pop or replace layers without holding the compositor (a
    borrow-checker-friendly command pattern).
  - The API: `push`, `pop`, `remove(id)`, `replace_or_push(id, layer)`, `find::<T>()`,
    `find_id::<T>(id)`.
  - The base layer is `EditorView`. It renders the editor and resolves keymaps.
- **Wrappers make positioning reusable:**
  - `Popup<T: Component>` anchors content near a position, usually the cursor. It flips above or
    below by `position_bias`, can scroll and `auto_close`, and carries an `id`.
  - `Overlay<T>` centres content with `calc_child_size: Fn(Rect) -> Rect`. `overlaid` reserves 2
    bottom rows and uses the full width when the terminal is under 200 columns, otherwise 90% (a 5%
    margin per side; the doc comment's "5%" is only true for wide terminals).
- **How the features are built:**

  | Feature | Built as |
  |---|---|
  | Picker | `Overlay<Picker<T, D>>` |
  | Completion | `Popup<Completion>`, which wraps a `Menu`; the doc popup is another `Popup` |
  | Hover, signature help | `Popup<Markdown>` / `Popup<SignatureHelp>` with fixed ids, so a re-trigger replaces the existing popup |
  | Diagnostics | Inline decorations, gutter signs and a picker source; nothing new |
  | Command line, search | `Prompt` component |
  | Which-key style hints | `Info` |

- **Picker (`ui/picker.rs`):**
  - `columns: Arc<[Column<T, D>]>`, where each column has a format function and a `filter` flag.
    Queries can target a column (`%column text`, via `PickerQuery`).
  - The matcher is a `nucleo::Nucleo<T>`. Items are pushed through an `Injector` from any thread
    while the picker is open.
  - Each render calls `matcher.tick(10)`, waiting at most 10 ms for workers, and nucleo's notify
    callback is `helix_event::request_redraw`.
  - `version: Arc<AtomicUsize>` invalidates stale injectors when a *dynamic* picker (for example
    global search or workspace symbols) restarts. A debounced `DynamicQueryChange` hook re-runs the
    query.
  - Previews are cached per path, and syntax highlighting of previews runs on a background hook.

### 4.3 Events and hooks (`helix-event`)

- **Typed events.**
  - `events!` declares them:
    - in helix-view: `DocumentDidOpen`, `DocumentDidChange`, `DocumentDidClose`,
      `SelectionDidChange`, `DiagnosticsDidChange`, `DocumentFocusLost`,
      `LanguageServerInitialized`, `LanguageServerExited`, `ConfigDidChange`;
    - in helix-term: `OnModeSwitch`, `PostInsertChar`, `PostCommand`.
  - `register_hook!` adds synchronous `Fn(&mut Event) -> Result<()>` hooks. Hooks get mutable
    access to the event payload, so they can, for example, hide the completion popup at once.
  - `register_dynamic_hook` exists for runtime-registered hooks, which a plugin system would use.
- **`AsyncHook`** is a stateful worker on a tokio task, fed by a bounded channel of 128 that is
  drained immediately.
  - `handle_event(event, timeout) -> Option<Instant>` either consumes the event or sets or extends
    a debounce deadline. `finish_debounce()` fires when the deadline passes.
  - Sync hooks forward to async hooks with `send_blocking`.
  - Completion uses this pattern: a debounce of `editor.completion-timeout` (default 250 ms) for
    auto-triggered completion, or 5 ms for trigger characters and manual requests, then requests to
    every provider (LSP servers, path, word) in a `JoinSet`. It waits up to **100 ms** for all of them, shows what has arrived,
    and adds late results incrementally.
  - Signature help uses a VSCode-derived debounce.
  - `TaskController`/`TaskHandle` plus `cancelable_future` cancel superseded requests.
- **Frame control:**
  - `request_redraw()` and `redraw_requested()` let any thread ask for a frame.
  - `start_frame()` and `lock_frame()` let a background task hold off rendering while it applies a
    batch of updates, so no half-updated frame is drawn.
  - `status` posts messages to the main loop.

### 4.4 Plugin system status

- Helix has **no plugin system in a release** as of 2026-10-09.
- The Steel (Scheme) plugin PR, helix-editor/helix#8675 "Add Steel as an optional plugin system",
  is **still open and draft**: 336 issue comments plus 91 review comments, last updated
  2026-10-09. (Unverified by the fact-check: that maintainers have warned users not to build
  ecosystems on the unmerged branch.)
- Helix's extensibility today comes from config: keymaps to typed commands, themes and
  `languages.toml`.
- The internal seams are the ones neomail should copy, and a plugin system would bind to them:
  typed events plus dynamic hooks, `Component`, the picker with columns and injectors, and the
  `DiagnosticProvider` enum.
- **Lesson:** design the primitives so a scripting layer binds to them later. Helix has struggled to
  retrofit plugins because UI components are Rust generics (`Picker<T, D>`) with closures rather
  than registry entries a script can create.

---

## 5. neomail building blocks (Rust sketches and performance contracts)

These are proposals for the synthesis, not final APIs. Conventions:

- IDs are generational handles (`slotmap` keys) so stale handles held by Lua fail safely.
- All primitives live in the TUI process unless §5.9 says otherwise.
- Every primitive has a **performance contract** that tests enforce: benchmarks and `insta` frame
  snapshots with timing assertions in CI.

```rust
new_key_type! { pub struct BufferId; pub struct WindowId; pub struct NsId; pub struct HookId; }
pub struct HlId(u32);        // interned highlight group ("NmailDiagError"); theme resolves to Style
pub struct ActionId(u32);    // index into the typed action catalogue (nmail-api)
```

### 5.1 Buffers: every view is a buffer

A buffer owns **content, kind, options, buffer-local keymaps and marks**. Windows own the viewport.
There are two content shapes because mail lists must never be materialised as text.

```rust
pub struct Buffer {
    pub id: BufferId,
    pub kind: BufferKind,
    pub name: SafeText,                 // shown in statusline / buffer picker
    pub content: Content,
    pub opts: BufOpts,                  // typed core options + ext: HashMap<Arc<str>, Value>
    pub keymaps: KeymapLayer,           // buffer-local, see §5.6
    pub marks: MarkStore,               // decorations, see §5.4
    pub vars: VarMap,                   // like b: variables, readable from Lua
    pub rev: u64,                       // bumps on every content change; views sync lazily
}

pub enum BufferKind {
    MailList { view: ViewSpec },        // query + scope + grouping + sort
    Message  { msg: MessageId },        // rendered, read-only, foldable quotes/headers
    Thread   { thread: ThreadId },      // messages as folds; per-message marks
    Draft    { draft: DraftId },        // editable Markdown; the only text-editing kind in v1
    Picker   { session: PickerId },     // prompt + results (rows) + preview (another buffer)
    Help     { topic: Arc<str> },
    Log,                                // tracing + errors + Lua print; append-only
    Ext      { kind: Arc<str> },        // extension-defined, e.g. "calendar.agenda"
}

pub enum Content {
    Text(TextStore),                    // ropey Rope + line index (message, draft, help, log)
    Rows(Box<dyn RowSource>),           // virtual list (mail list, picker results)
}

/// Virtual list over data that may live in the daemon. Never materialises all rows.
pub trait RowSource: Send {
    fn len(&self) -> RowCount;                          // Exact(n) or AtLeast(n) while streaming
    fn row(&self, i: usize) -> RowRef<'_>;              // Ready(&Row) | Pending (placeholder)
    fn key(&self, i: usize) -> Option<ItemKey>;         // stable identity (MessageId, ...)
    fn index_of(&self, k: &ItemKey) -> Option<usize>;   // to keep cursor/marks on reorder
    fn prefetch(&mut self, rows: Range<usize>);         // async page request, never blocks
    fn columns(&self) -> &[ColumnSpec];                 // for formatters, picker %column queries
}
```

**Contracts:**

- `row(i)` is O(1) and allocation-free on a cache hit. A miss returns `Pending`, renders a
  placeholder row, and the page arrives as an event.
- `prefetch` is called with the visible range ± one screen.
- A mail list with 100k+ rows opens in **one frame**: the count comes from the daemon's view index
  and the rows from one page.
- `Text` edits are O(log n) on the rope plus a mark splice (§5.4).
- `kind` is fixed at creation. Behaviour comes from options, keymaps and providers attached to the
  kind, which is Neovim's `buftype` + `filetype` pair.
- Lua-defined kinds register `(kind name, default opts, keymaps, providers)`. Nothing is
  special-cased.

### 5.2 Windows, floats and layout

```rust
pub struct Window {
    pub id: WindowId,
    pub buf: BufferId,
    pub cursor: Cursor,                  // per window (Neovim), not per buffer (avoid Helix's hack)
    pub top: usize,                      // topline / first visible row
    pub opts: WinOpts,                   // number, relativenumber, wrap, conceal, signcolumn, winhl
    pub placement: Placement,
    pub synced_rev: u64,                 // last buffer rev seen; lazy resync like Helix View
}

pub enum Placement { Split(LayoutNodeId), Float(FloatSpec) }

pub struct FloatSpec {
    pub relative: Relative,              // Screen | Window(WindowId) | Cursor | Mouse | Mark(NsId, MarkId)
    pub anchor: Corner,                  // NW | NE | SW | SE
    pub offset: (i16, i16),
    pub size: SizeSpec,                  // Fixed | Percent | Fit { min, max } via required_size
    pub flip: FlipPolicy,                // above/below and left/right when off-screen (Helix Popup)
    pub zindex: u16,                     // default 50, like nvim_open_win
    pub focusable: bool,
    pub border: BorderSpec, pub title: Option<StyledLine>, pub footer: Option<StyledLine>,
    pub style: Option<StyleName>,        // named preset (snacks "styles"): "hover", "diagnostic", "notify"
    pub close_on: CloseOn,               // CursorMoved | BufLeave | Esc | Never
}

pub enum LayoutNode {                    // snacks-style box tree; also used inside picker layouts
    Box { dir: Dir, children: Vec<(LayoutNodeId, Constraint)> },
    Leaf(WindowId),
    Hidden(WindowId),                    // toggled out (e.g. preview pane) without destroying it
}
```

- **Floats are windows.** Hover, the diagnostic explainer, completion menus and docs,
  notifications, `z=` suggestions and confirmation dialogs are all `Buffer` + `Window` +
  `FloatSpec`. So each gets the same keymaps, decorations, scrolling, mouse and theme support.
- **Named window styles** (`StyleName`) are a registry of `FloatSpec` and `WinOpts` presets that
  users override in TOML or Lua. Built-in features only use styles, which keeps the look
  consistent and themeable.
- **Tabpages become "workspaces"**: saved layout trees such as an "inbox + reader" split and a
  "triage" grid. Defer them until after v1.
- **Contract:**
  - Layout resolution is O(windows) and runs only on resize or layout change.
  - Moving the cursor only re-renders the affected window, plus any float anchored to the cursor.

### 5.3 Compositor and layers

Use Helix's `Component` and callback model, but make windows the content. That way extensions
never have to implement `Component` to get UI.

```rust
pub trait Layer: Send {
    fn handle(&mut self, ev: &UiEvent, cx: &mut Cx) -> Handled;          // Consumed | Ignored, + Option<Callback>
    fn render(&mut self, area: Rect, surf: &mut Surface, cx: &RenderCx);
    fn cursor(&self, area: Rect, cx: &RenderCx) -> Option<(Position, CursorKind)> { None }
    fn required_size(&mut self, viewport: (u16, u16)) -> Option<(u16, u16)> { None }
    fn id(&self) -> Option<&'static str> { None }                         // replace_or_push by id
    fn dirty(&self) -> bool { true }                                     // skip unchanged layers
}
pub type Callback = Box<dyn FnOnce(&mut Compositor, &mut Cx) + Send>;

pub enum LayerClass { Base, Float, Popup, Overlay, Fx, Notify, Prompt }   // fixed band order
```

- **Bands, bottom to top:**

  | Band | Contents |
  |---|---|
  | `Base` | the split layout of windows |
  | `Float` | floats ordered by `zindex` |
  | `Popup` | cursor-anchored menus |
  | `Overlay` | pickers |
  | `Fx` | the `SortFx` sorting animation; never takes input |
  | `Notify` | notifications |
  | `Prompt` | the `:` command line and confirms |

  Within a band, `zindex` orders layers.
- **Input** goes top to bottom through bands that accept it. A modal layer can "trap" input (a
  confirm dialog). Mouse hits are routed by the window rects from the last frame.
- **The frame loop:**
  - Redraw requests are coalesced (`request_redraw` from any thread sets an atomic flag and wakes
    the loop).
  - At most one frame per input batch, capped at the terminal's refresh rate, about 60 fps.
  - Ratatui's buffer diff flushes only changed cells.
  - A `lock_frame` guard (from helix-event) lets a task apply a batch, such as a sync delta,
    atomically.
- **Contract:**
  - A frame is under **4 ms** p99 for a 200x60 terminal with 3 floats, which leaves headroom for
    SSH.
  - Render functions never block, never do I/O, and never await.
  - `Fx` layers can be dropped by an `FxGovernor` when a frame overruns, and sorting itself never
    waits on them.

### 5.4 Decorations and namespaces

Neovim's extmarks with one adaptation: **marks in list buffers anchor to item keys, not text
positions**, because lists re-sort and re-group.

```rust
pub struct Namespace { pub id: NsId, pub name: Arc<str>, pub owner: Owner }  // Core | Ext(ExtId)

pub enum Anchor {
    Text { start: Pos, end: Option<Pos>, gravity: (Gravity, Gravity) },   // text buffers
    Item { key: ItemKey, col: Option<ColRange> },                         // row buffers
}

pub struct Mark {
    pub ns: NsId, pub anchor: Anchor, pub priority: u16,
    pub hl: Option<HlId>,                               // range highlight (stackable)
    pub virt_text: Option<(StyledLine, VirtPos)>,       // Eol | Overlay | RightAlign | Inline
    pub virt_lines: Option<Vec<StyledLine>>,            // e.g. thread separators, grammar hints
    pub sign: Option<(SafeText, HlId)>,                 // signcolumn: spelling, flagged, unread
    pub conceal: Option<Option<char>>,                  // hide Markdown syntax, long URLs
    pub url: Option<SafeUrl>,                           // OSC 8 hyperlink
    pub data: Option<Value>,                            // payload for hover/diagnostic lookup
}

pub trait MarkStore {
    fn set(&mut self, m: Mark) -> MarkId;                                // O(log n)
    fn del(&mut self, ns: NsId, id: MarkId);                             // O(log n)
    fn clear(&mut self, ns: NsId, rows: Range<usize>);                   // O(log n + k)
    fn splice(&mut self, edit: &TextEdit);                               // O(log n + touched)
    fn query(&self, rows: Range<usize>, out: &mut Vec<&Mark>);           // O(log n + k), no alloc
}

/// Redraw-time decorations (Neovim decoration providers): derived data, never stored.
pub trait DecorationProvider: Send {
    fn ns(&self) -> NsId;
    fn on_win(&mut self, win: &WinInfo, visible: Range<usize>) -> bool { true }   // false = skip
    fn on_range(&mut self, buf: &BufView, rows: Range<usize>, out: &mut FrameMarks);
}
```

- **Storage:**
  - Text buffers use an interval B-tree like Neovim's marktree. Write it in-house, or adapt the
    `rust-lapper` / `iset` interval-tree crates (not yet evaluated) and add a `splice` on top.
  - Row buffers use `HashMap<ItemKey, SmallVec<Mark>>`. A query maps visible rows to keys through
    `RowSource::key`, which is O(visible).
- **Ephemeral providers** do the Markdown syntax highlighting of drafts (incremental, visible rows
  only), search-match highlights, quote and signature colouring, and the "selected" and "unread"
  styling of rows. Persistent marks hold diagnostics, user flags and Lua annotations.
- **Highlights:** semantic group names (`NmailUnread`, `NmailDiagSpell`, `NmailGroupReceipts`)
  with `link`s and `default` links (Neovim). Themes set groups. Per-window `winhl` remaps groups,
  for example to dim inactive windows. `HlId` is resolved to a `Style` once per theme change, not
  once per cell.
- **Contracts:**
  - `query` of the visible range is under 50 µs for 10k marks.
  - Providers get at most 1 ms per window per frame. Overruns are logged and the provider is
    throttled, so a slow Lua provider cannot freeze typing.
  - Marks from untrusted text (mail content) go through `SafeText`.

### 5.5 The event bus (autocommands)

This is a typed bus in the style of helix-event, with Neovim's ergonomics: groups, patterns,
buffer-local hooks and `User` events.

```rust
pub enum Event {
    Buf(BufEvent),        // Enter, Leave, New, Changed{rev}, Wipe, ...
    Win(WinEvent),        // Opened, Closed, Resized, Scrolled
    Cursor(CursorEvent),  // Moved, Hold (after `updatetime` idle)
    Mode { old: Mode, new: Mode },
    Picker(PickerEvent),  // Open, QueryChanged, Confirm, Close
    Mail(MailEvent),      // forwarded daemon events: Arrived, Classified{group}, SyncDone, SendState
    Config, Theme,
    User { name: Arc<str>, data: Value },
}

pub struct HookSpec {
    pub filter: EventFilter,          // kind + optional BufferKind / BufferId / pattern
    pub group: GroupId,               // augroup: clear-all on extension reload
    pub priority: i16,
    pub once: bool,
}

pub enum Hook {
    Sync(Box<dyn Fn(&mut Event, &mut HookCx) -> Flow + Send + Sync>),   // may mutate / cancel
    Async(mpsc::Sender<Event>),                                         // AsyncHook worker w/ debounce
    Lua(LuaFnRef),                                                      // runs on the Lua thread
}
```

- **Dispatch** is indexed by event kind: a `Vec<HookId>` per kind, sorted by priority.
  `Flow::Stop` cancels later hooks and, for `Pre*` events, the default behaviour.
- **Daemon events** arrive on one channel. They are **coalesced per frame**: 500 `Classified`
  events become one list refresh plus one `SortFx` batch.
- **Contracts:**
  - Native sync hooks get 1 ms in total per event.
  - Lua hooks on hot events (`Cursor::Moved`, `Buf::Changed`) are debounced by default and run off
    the input path.
  - `Cursor::Hold` is the place for expensive reactions such as hover-on-idle.
  - Unsubscribing is O(1) by `HookId`, and by group it is O(group size).
- The same `MailEvent` stream is what the CLI's `nmail events --json` prints, so scripts and the
  TUI see one vocabulary.

### 5.6 Keymap scopes

```rust
pub enum Scope { Layer(&'static str), Buffer(BufferId), Kind(Arc<str>), Mode(Mode), Default }
pub struct Binding { pub rhs: Rhs, pub desc: Arc<str>, pub nowait: bool }
pub enum Rhs { Action(ActionId, Value /* typed args */), Lua(LuaFnRef), Keys(KeySeq) }

pub struct Keymaps { tries: HashMap<(Scope, Mode), Trie<KeyEvent, Binding>> }
```

- **Resolution order** (first hit wins):
  1. the focused layer (picker, float);
  2. buffer-local;
  3. the buffer kind (`mail_list`, `draft`, `ext:calendar.agenda`);
  4. the mode;
  5. defaults.
- Resolution walks the tries in step: O(sequence length × scopes). A pending prefix waits up to
  `timeoutlen`.
- **Hints and help come from the trie.** The which-key style hint line shows the current node's
  children across all active scopes, deduplicated by priority. The "lookup sheet" and the keymaps
  picker list `(scope, lhs, desc)`.
- **Bindings mostly point at catalogue actions.** So a binding is documentation, a CLI command and
  an MCP tool for free, and rebinding never needs code. Lua bindings exist for
  extension-specific glue.

### 5.7 Provider registries

One generic registry type. Each provider kind is a trait, plus a policy struct that users configure
without code (the blink.cmp lesson).

```rust
pub struct Registry<P: ?Sized> { entries: IndexMap<Arc<str>, Entry<P>> }
pub struct Entry<P: ?Sized> { pub provider: Arc<P>, pub owner: Owner, pub policy: Policy, pub enabled: bool }
pub struct Policy { pub timeout_ms: u32, pub score_offset: i32, pub fallbacks: Vec<Arc<str>>,
                    pub max_items: Option<u32>, pub kinds: Option<Vec<Arc<str>>> /* buffer kinds */ }
```

**PickerSource.** This is the snacks finder plus Helix's `Injector`, and it can run in the daemon or
the TUI.

```rust
pub trait PickerSource: Send + Sync {
    fn meta(&self) -> &SourceMeta;     // name, columns, scopes supported, live?, default
                                       // format/preview/actions/layout, frecency key
    fn find(&self, q: FindQuery, out: Injector<PickItem>, cancel: CancelToken) -> BoxFuture<'static, Result<FindDone>>;
}
pub struct FindQuery { pub search: String, pub scope: Scope, pub limit: Option<u32> } // `search` = finder query (live)
pub struct PickItem { pub key: ItemKey, pub cols: SmallVec<[Utf32String; 4]>, pub bonus: i32, pub data: Value }
```

- **Two queries, as in snacks:**
  - `search` goes to `find`. Live sources such as full-text mail search turn it into an FTS5 query
    in the daemon and stream pages.
  - The *pattern* is matched locally on worker threads.
  - Typing in live mode cancels and restarts `find`. Otherwise it only re-matches.
  - Scope switching re-runs `find` and keeps both strings.
- **Matcher:** nucleo's `Nucleo<T>` model (an append-only injector, worker pool, `tick(budget)`,
  and a notify-to-redraw callback). frizbee is an MIT, typo-tolerant alternative for completion. Use
  a `Matcher` trait so either can sit underneath.
- **Contracts:**
  - The first results render within one frame of a keystroke.
  - `tick` gets at most 2-4 ms of the frame.
  - Columns are converted to UTF-32 once, at injection.
  - Match positions are computed only for visible rows (snacks).
  - The top-k is kept in a heap.
- **Picker composition.** A picker is `source + format + previewer + actions + layout(style)`, all
  named registry entries. "New picker" usually means a config table (snacks), not code. The
  meta-pickers come free: pickers, actions, keymaps, providers, highlights, layouts, log.
- `nmail pick <source> --query .. --scope .. --json` runs daemon-side sources headless through the
  same matcher crate, so agents and the CLI query the same sources.

**The other providers:**

```rust
pub trait CompletionSource: Send + Sync {                       // blink.cmp model
    fn triggers(&self) -> &[char] { &[] }
    fn complete(&self, cx: &CompletionCx, cancel: CancelToken) -> BoxFuture<'static, CompletionResponse>;
    fn resolve(&self, item: &CompletionItem) -> Option<BoxFuture<'static, CompletionItem>> { None }
}
pub struct CompletionResponse { pub items: Vec<CompletionItem>, pub incomplete_fwd: bool, pub incomplete_bwd: bool }

pub trait DiagnosticSource: Send + Sync {                       // vim.diagnostic producer
    fn ns(&self) -> NsId;
    fn check(&self, snap: &TextSnapshot, dirty: &[Range<usize>], cancel: CancelToken) -> BoxFuture<'static, Vec<Diagnostic>>;
    fn actions(&self, d: &Diagnostic) -> Vec<CodeAction> { vec![] }   // z= suggestions, zg add-word
}
pub trait DiagnosticHandler: Send + Sync {                      // vim.diagnostic.handlers consumer
    fn show(&self, buf: BufferId, ns: NsId, diags: &[Diagnostic], marks: &mut dyn MarkStore);
    fn hide(&self, buf: BufferId, ns: NsId, marks: &mut dyn MarkStore);
}
pub trait Previewer: Send + Sync {
    fn preview(&self, item: &PickItem, size: (u16, u16), cancel: CancelToken) -> BoxFuture<'static, PreviewContent>;
}
pub trait HoverProvider: Send + Sync {
    fn hover(&self, at: &HoverTarget /* text pos or row item */, cancel: CancelToken) -> BoxFuture<'static, Option<HoverDoc>>;
}
pub trait AnimationStyle: Send {                                // the SortFx seam from the prototypes
    fn begin(&mut self, moves: &[SortMove], area: Rect, motion: MotionPref) -> FxPlan;
    fn frame(&mut self, t: Duration, surf: &mut Surface) -> FxState; // Running | Done; ≤ 1 ms
}
```

- **Completion:**
  - The sources are addresses (daemon, contacts ranked by frecency), dictionary words, emoji,
    templates and snippets, and Lua sources.
  - All queries go out in parallel. The menu shows what has arrived after `timeout_ms` (Helix uses
    100 ms) and late arrivals are merged in.
  - A complete response is re-filtered locally on later keystrokes.
- **Diagnostics:** spelling and grammar are `DiagnosticSource`s under namespaces `spell` and
  `grammar`. The built-in handlers are underline, sign, eol virtual text, the explainer float, the
  picker and `]s`/`[s` navigation. `z=` opens `ui.select` over the actions. Producers are debounced
  per dirty paragraph.
- **Hover** merges providers by priority into one float with the `hover` style: diagnostic
  explanation, address card, link target, attachment info.
- **AnimationStyle:** `MotionPref::Reduced` or a governor overrun gives `FxPlan::Skip`. Sorting has
  already been applied to the buffer before `begin`.
- **Swappable UI interfaces** (Neovim's `vim.ui`): `ui.select(items, opts) -> Future<Option<idx>>`
  and `ui.input(opts) -> Future<Option<String>>`. Core features call these and never a concrete
  widget, so a Lua extension can replace them.

### 5.8 How Lua (mlua) plugs into the same registries

- **Equal footing.**
  - Every registry stores `Arc<dyn Trait>`. A `LuaProvider` adapter implements each trait by
    calling into Lua. Rust built-ins and Lua extensions are indistinguishable to callers.
  - Built-in pickers should be *declared* the way a Lua one would be (a `SourceMeta` + config), so
    the Lua path is exercised by core features every day.
- **One Lua thread per process that runs Lua.**
  - The `mlua::Lua` state lives on a dedicated thread. Rust sends calls over a channel and gets
    futures back.
  - Do **not** use the `send` feature to share the state across threads: it forces `Send + Sync`
    on all userdata and adds locking.
  - mlua's `async` feature maps Lua coroutines to Rust futures, so a Lua `find` can `await` a
    daemon query and push items as they arrive.
- **API shape mirrors Neovim**, so the docs, users' habits and agents' prior knowledge transfer:

  ```lua
  local ns  = nmail.api.create_namespace("myext.flags")
  local buf = nmail.api.buf_create({ kind = "ext:myext.digest", modifiable = false })
  nmail.api.buf_set_mark(buf, ns, { item = msg_key, sign = "★", hl = "NmailFlag" })
  local win = nmail.api.open_win(buf, { relative = "cursor", anchor = "NW", style = "hover" })
  nmail.on("Mail.Classified", { group = "myext" }, function(ev) ... end)
  nmail.keymap.set("n", "gd", "myext.digest", { kind = "mail_list", desc = "Digest" })
  nmail.picker.source("myext.vips", { columns = {"name","addr"}, find = function(q, push) ... end })
  nmail.diagnostic.set(ns, buf, { { row = 3, col = 4, severity = "hint", message = "..." } })
  nmail.ui.select = my_select   -- swap the implementation, like vim.ui.select
  ```

- **Handles** (`buf`, `win`, `ns`) are userdata wrapping generational IDs. A stale handle returns
  an error instead of touching a reused slot.
- **Ownership:** every registration records `Owner::Ext(id)`, so unload and hot reload remove
  hooks, keymaps, providers, namespaces and windows in one sweep. These are Neovim augroups
  generalised to every registry.
- **Budgets:**
  - Luau `sandbox(true)` + `set_interrupt` give per-call instruction and time limits. Use a memory
    limit per extension state.
  - Calls on the input or render path (keymap callbacks, decoration `on_range`) are capped at 1 ms
    and are cancelled or throttled when they exceed it.
  - Everything else runs off the frame.

### 5.9 What lives in the daemon versus the TUI

The architecture draft (`docs/architecture/README.md`) currently says the daemon is the only
process that runs Lua. **That works for data extensions but not for UI extensions.** A decoration
provider or keymap callback that crosses a Unix socket every frame or keystroke breaks the frame
contract and couples every client's UI to one shared interpreter. Neovim gets away with "all Lua in
the core" only because it has one core per editing session.

| Lives in the **daemon** (shared, headless, holds data and secrets) | Lives in the **TUI** (per client, latency-critical) |
|---|---|
| Action catalogue and `Dispatcher` (authoritative) | Buffers, windows, layout, compositor, layers |
| Store, sync, providers, jev classification, outbox | Decorations, namespaces, decoration providers |
| `PickerSource`s over mail data: full-text search, senders (frecency), subjects, folders and labels, accounts, attachments, drafts | `PickerSource`s over UI state: draft lines, keymaps, layouts, highlights, open buffers; matcher workers for **all** pickers |
| `CompletionSource`: addresses and contacts | `CompletionSource`: words, emoji, snippets; the completion menu and merge |
| Event origin for `Mail.*`; `nmail events` stream | UI events (`Buf`, `Win`, `Cursor`, `Mode`); forwards to its own hooks |
| **Daemon Lua:** data hooks (on arrive, classify, send), custom actions, scheduled jobs, data sources | **TUI Lua:** keymap callbacks, decoration providers, buffer kinds, floats, `ui.select` overrides |
| Spelling dictionaries and user word lists (persisted) | Spelling and grammar `DiagnosticSource` workers on the draft text (for example spellbook for Hunspell, harper-core for English grammar), so per-keystroke checks avoid IPC |

- **An extension is one package with optional `daemon/` and `ui/` entry points**, sharing a manifest
  and capabilities.
  - The `ui/` half talks to its `daemon/` half only through actions and `User` events. Both are
    typed and audited.
  - TUI Lua has no network, no filesystem outside its own directory, and no secrets (the TUI
    never holds them anyway).
- **Remote sources are cheap.** A daemon `PickerSource` is exposed to the TUI as a proxy that
  streams `PickItem` pages over the socket into a local `Injector`. The matcher always runs in the
  TUI. This gives one-frame latency even for daemon sources, and CLI/agent queries reuse the same
  source.

---

## Key facts

- **[high]** Neovim's `buftype` values are empty, `acwrite`, `help`, `nofile`, `nowrite`, `prompt`, `quickfix` and `terminal`. Buffers carry local options, keymaps and marks. Windows carry the cursor, the scroll position and window-local options. — <https://neovim.io/doc/user/options.html#'buftype'>
- **[high]** `nvim_open_win` floats are positioned `relative` to the editor, a window, the cursor or the mouse, with an `anchor` corner. They take `zindex` (default 50), `focusable`, `border`, `title`/`footer`, `style="minimal"` and `hide`. — <https://neovim.io/doc/user/api.html#nvim_open_win()>
- **[high]** Extmarks track edits, with forward gravity by default. They can carry stacked `hl_group`s, `virt_text` (eol, overlay, right_align, inline), `virt_lines`, signs, `conceal`, `url`, `priority` and `ephemeral`. — <https://neovim.io/doc/user/api.html#extmarks>
- **[high]** Marks are stored in a B-tree (`marktree.c`) derived from klib's kbtree and inspired by Atom's marker tree. Text changes are applied with `marktree_splice`. — <https://github.com/neovim/neovim/blob/master/src/nvim/marktree.c>
- **[high]** Decoration providers run only for windows being redrawn (`on_start`, `on_buf`, `on_win`, `on_range`, `on_end`). They set ephemeral marks, may return `false` to skip work, and must not change extmarks in `on_range`. The docs advise one provider per plugin. — <https://neovim.io/doc/user/api.html#nvim_set_decoration_provider()>
- **[high]** `vim.diagnostic` separates producers (namespaced `vim.diagnostic.set`) from handlers (`virtual_text`, `signs`, `underline`, `virtual_lines`). Custom handlers are added to `vim.diagnostic.handlers`. — <https://neovim.io/doc/user/diagnostic.html#diagnostic-handlers>
- **[high]** `vim.ui.select` is the standard, overridable picker interface, and replacements are expected to call `format_item` and `preview_item` and may use `kind` (0.12.6 docs). **[medium]** In nightly (0.13-dev) `z=` and `:tselect` delegate to it; 0.12.6 does not. — <https://neovim.io/doc/user/lua.html#vim.ui.select()>, <https://github.com/neovim/neovim/blob/master/runtime/doc/news.txt>
- **[high]** In Nvim 0.12 the `'complete'` option accepts several `F{func}` sources, and `'autocomplete'` exists (default off). Verified in the v0.12.6 `options.txt`. — <https://neovim.io/doc/user/options.html#'complete'>
- **[high]** Since Nvim 0.9 the TUI runs in a separate process from the core. The latest stable is 0.12.6 (2026-10-09). — <https://neovim.io/doc/user/news-0.9.html>, <https://github.com/neovim/neovim/releases/tag/v0.12.6>
- **[high]** snacks.nvim builds input, notifier, terminal, scratch, zen, dashboard, image and every picker sub-window on `Snacks.win`. Each feature registers a named style with `Snacks.config.style`. — <https://github.com/folke/snacks.nvim/blob/882c996cf28183f4d63640de0b4c02ec886d01f2/lua/snacks/win.lua>
- **[high]** The snacks picker config separates `finder`, `matcher`, `sort` (default `score:desc`, `#text`, `idx`), `transform`, `format`, `preview`, `actions` and `layout`. It also separates `search` (re-runs the finder in `live` mode, with `limit_live` 10000) from `pattern` (matcher only). — <https://github.com/folke/snacks.nvim/blob/882c996cf28183f4d63640de0b4c02ec886d01f2/lua/snacks/picker/config/defaults.lua>
- **[high]** The snacks matcher re-checks the previous top-k first, then previous matches. It skips previous non-matches when the new pattern extends the old one, orders pattern parts by "entropy", and uses fzf's scoring constants (defined in `core/score.lua`). — <https://github.com/folke/snacks.nvim/blob/882c996cf28183f4d63640de0b4c02ec886d01f2/lua/snacks/picker/core/matcher.lua>, <https://github.com/folke/snacks.nvim/blob/882c996cf28183f4d63640de0b4c02ec886d01f2/lua/snacks/picker/core/score.lua>
- **[high]** snacks finders and matchers are coroutines that yield every 1 ms. A libuv check handle runs them with a 10 ms budget per loop turn, all on the main thread. — <https://github.com/folke/snacks.nvim/blob/882c996cf28183f4d63640de0b4c02ec886d01f2/lua/snacks/picker/util/async.lua>
- **[high]** A blink.cmp `Source` has `get_completions(ctx, cb)` returning a cancel function, plus optional `resolve`, `execute`, trigger characters and signature help. Responses carry `is_incomplete_forward/backward`. Provider policy includes `async`, `timeout_ms`, `fallbacks`, `score_offset`, `max_items`, `min_keyword_length` and `override`. — <https://github.com/saghen/blink.cmp/blob/v1.10.2/lua/blink/cmp/sources/lib/types.lua>, <https://github.com/saghen/blink.cmp/blob/v1.10.2/lua/blink/cmp/config/sources.lua>
- **[high]** blink.cmp's fuzzy core is Rust (mlua module) on frizbee, with a Lua fallback (`implementation = "prefer_rust_with_warning"`). Typo tolerance, frecency and proximity apply only to the Rust implementation. v1.10.2 pins frizbee 0.9.0, not 0.13. — <https://github.com/saghen/blink.cmp/blob/v1.10.2/lua/blink/cmp/config/fuzzy.lua>
- **[medium]** frizbee 0.13.0 (MIT) is SIMD Smith-Waterman with affine gaps and typo-aware prefilters. In the author's benchmark on 1.41M paths it took 22.4 ms sequential and 3.5 ms on 8 threads, against nucleo's 90.5 / 36.8 ms, with typo tolerance off (1 typo: 60.8 / 9.5 ms). It is vendor-run. 0.13.0 was published 2026-08-13. — <https://github.com/saghen/frizbee/blob/main/BENCHMARKS.md>, <https://crates.io/crates/frizbee>
- **[high]** nucleo 0.5.0 (MPL-2.0, last crates.io release 2024-04) provides `Nucleo<T>` with a thread-safe `Injector`, background workers, `tick(timeout)`, and a notify callback. Helix (`master` at `ba40e54`, workspace version 25.7.1) depends on `nucleo = "0.5.0"`. — <https://github.com/helix-editor/nucleo/blob/master/src/lib.rs>, <https://crates.io/crates/nucleo>
- **[high]** Helix's `Component` trait is `handle_event`, `render`, `cursor`, `required_size`, `should_update` and `id`. The `Compositor` renders layers bottom to top and routes events top to bottom, and components return deferred `Callback`s that mutate the compositor. — <https://github.com/helix-editor/helix/blob/ba40e54/helix-term/src/compositor.rs>
- **[high]** The Helix picker is `Nucleo<T>` plus `Column`s with filter flags. It calls `tick(10)` per render, uses `request_redraw` as the notify hook, and uses an atomic `version` to invalidate dynamic-query injectors. — <https://github.com/helix-editor/helix/blob/ba40e54/helix-term/src/ui/picker.rs>
- **[high]** helix-event provides typed events, sync hooks, debounced `AsyncHook` workers on bounded channels, `TaskController` cancellation, and `request_redraw`/`lock_frame`. Completion debounces by `completion-timeout` (default 250 ms) when auto-triggered or 5 ms for trigger characters and manual requests, then waits up to 100 ms for all providers before showing. — <https://github.com/helix-editor/helix/blob/ba40e54/helix-event/src/lib.rs>, <https://github.com/helix-editor/helix/blob/ba40e54/helix-term/src/handlers/completion/request.rs>
- **[high]** Helix keeps selections on the `Document` keyed by `ViewId` (`selections: HashMap<ViewId, Selection>`). A "HACKS" comment on the per-view diagnostics handler says fixing it needs an entity-component-like refactor of View and Document. — <https://github.com/helix-editor/helix/blob/ba40e54/helix-view/src/view.rs>
- **[high]** Helix's Steel plugin PR #8675 is still an open draft as of 2026-10-09. — <https://github.com/helix-editor/helix/pull/8675>
- **[high]** mlua 0.12.2 (2026-10-03) supports async on all Lua versions including Luau, through coroutines. The `send` feature makes `Lua: Send + Sync` at the cost of `Send` bounds on functions and userdata. — <https://github.com/mlua-rs/mlua>
- **[medium]** Offline checking candidates: spellbook 0.4.2 (Hunspell-compatible spellchecking in Rust, **MPL-2.0**, 2026-06-03) and harper-core 2.11.0 (an offline grammar checker, **Apache-2.0**, 2026-09-16). Versions and licences verified on crates.io; neither was evaluated in depth here. — <https://crates.io/crates/spellbook>, <https://crates.io/crates/harper-core>

## Recommendations

1. **Adopt seven primitives as the frozen UI interface crate** (`nmail-ui-api` or a new
   `nmail-ui-core`):
   - `Buffer` (with `Content::Text | Rows`);
   - `Window` with `Placement::Split | Float`;
   - `Layer`/`Compositor` bands;
   - `MarkStore` + `Namespace` + `DecorationProvider`;
   - the typed `Event` bus with sync, async and Lua hooks;
   - `Keymaps` scopes resolving to catalogue actions;
   - the generic `Registry<P>`.

   Build every v1 feature from them: list, reader, thread, composer, picker, hover, diagnostics,
   notifications, help, log, and the sorting animation.
2. **Floats are windows and features are styles.** Hover, the diagnostic explainer, completion
   docs, `z=`, confirms and notifications are buffers in floats with a named style. No bespoke
   popup widgets.
3. **Anchor list decorations to `ItemKey`, not rows.** Text buffers use a splice-able interval tree.
4. **Two-query pickers** (snacks): the daemon finder handles `search` and scope, and the local
   threaded matcher handles `pattern`.
   - Use nucleo's injector and tick model behind a `Matcher` trait.
   - Evaluate frizbee for completion (typos, MIT).
   - Steal snacks' top-k-first, subset-skip and lazy-positions tricks.
5. **Completion follows blink.cmp:** a small source trait, a separate policy config,
   `incomplete_*` caching, parallel sources with a 100 ms show deadline (Helix), and late merge.
6. **Diagnostics follow `vim.diagnostic`:** namespaced producers and pluggable handlers. Spelling
   and grammar are just two producers.
7. **Revise "only the daemon runs Lua"** to "daemon Lua for data, TUI Lua for UI, one package
   format" (§5.9). Otherwise UI extensibility either stalls or breaks the frame budget.
8. **Make the contracts executable.** Use criterion benchmarks and a frame-time harness in CI for:
   - frame under 4 ms p99;
   - visible mark query under 50 µs at 10k marks;
   - first picker results within one frame at 100k items;
   - hook budget 1 ms;
   - Lua render-path callbacks 1 ms.
9. **Declare built-in pickers, keymaps and styles as data**, the same way extensions do. Then the
   extension path is the core path, which is the Neovim and snacks property the owner wants.

## Open questions

- **Lua placement:** split daemon and TUI runtimes (recommended), or keep one daemon runtime and
  push UI ops to TUIs Neovim-style? This needs an owner and architecture decision.
- **Which matcher?** nucleo (Unicode normalisation, battle-tested in Helix, MPL-2.0, slow release
  cadence) or frizbee (MIT, faster, typo-tolerant, SIMD with a UTF-8 path since 0.13)? Or both behind a trait? Run a
  benchmark on real subjects and senders.
- **Mark storage:** write a marktree-style B-tree in-house, or adapt an existing interval-tree
  crate? Drafts are small, so a simpler structure may meet the contract.
- **Draft editing:** reuse an existing vim engine (edtui or modalkit) or build on the `Buffer`
  primitive directly? Only the latter gives drafts the same marks and keymaps as every other
  buffer.
- **Should mail-list rows be text-like?** That is, should vim motions, visual selection and search
  work across rows uniformly (Neovim `nofile` style), or only through list-specific actions?

## Fact-check (independent)

Checked 2026-10-09/10 against crates.io API, docs.rs, GitHub source at the pinned revisions, and
Neovim's `runtime/doc` at tag v0.12.6 (the neovim.io `.html` links now redirect to `/doc/user/<page>/`).

**Checked and confirmed:**

- Crate versions, dates and licences: frizbee 0.13.0 (MIT, 2026-08-13), nucleo 0.5.0 (MPL-2.0,
  2024-04-02), mlua 0.12.2 (MIT, 2026-10-03), spellbook 0.4.2, harper-core 2.11.0. Also noted for
  the synthesis: slotmap 1.1.1 (Zlib), rust-lapper 1.3.0 (MIT; 2.0.0-beta.1 exists), iset 0.3.3 (MIT).
- Neovim: 0.12.6 released 2026-10-09; `buftype` values; `nvim_open_win` keys and zindex default 50;
  extmark keys; decoration provider callbacks, `on_line` deprecated for `on_range`, one-provider
  advice and the no-modify rule; `vim.diagnostic.handlers` incl. `virtual_lines`; marktree derived
  from klib kbtree and Atom's marker tree; TUI in a separate process since 0.9; fzf-lua needs fzf > 0.36.
- snacks `882c996`: `limit_live = 10000`, sort `score:desc, #text, idx`, `<c-g>` toggles live,
  `ui_select = true`, `YIELD_MATCH`/`YIELD_FIND` 1 ms, `Async.BUDGET = 10` on a libuv check handle,
  top-k / previous-matches / subset-skip order, entropy ordering, `BONUS_FRECENCY = 8`,
  `BONUS_CWD = 10`, fzf constants, `smart`/`grep`/`spelling` definitions, styles registered via
  `Snacks.config.style`.
- blink.cmp v1.10.2: `Source` fields, `is_incomplete_forward/backward`, provider policy fields,
  `per_filetype`, fuzzy `implementation` values, Rust-only typos/frecency/proximity, blake3 + bincode.
- frizbee README/BENCHMARKS: SIMD Smith-Waterman with affine gaps, 2-load prefilter example, 1/2/N
  typo prefilters, query syntax, `match_list_parallel`, radix sort, "used by" list, the Chromium numbers.
- Helix `ba40e54`: `Component`/`EventResult`/`Callback`, compositor API, `Popup` `position_bias` and
  `auto_close`, picker `tick(10)`, `request_redraw` notify, `Arc<AtomicUsize>` version, `Column`
  filter flag, `DynamicQueryChange`; event lists; `AsyncHook` channel of 128, `send_blocking`,
  `register_dynamic_hook`, `start_frame`/`lock_frame`; 100 ms completion deadline; `Editor` fields;
  `TextAnnotations`/`Decoration`; `DiagnosticProvider` enum (in helix-core). PR #8675 open draft.
- mlua: async for all Lua versions incl. Luau; `send` bounds; `Lua::sandbox` (Luau only),
  `set_interrupt`, `set_memory_limit` exist on docs.rs 0.12.2.

**Corrected in place:**

1. Revision header: `8219b58` and `5fa0fa4` are later `main` commits, not the v1.10.2 / 0.13.0
   tags (`78336bc`, `d608923`); `ba40e54` is Helix `master`, not the 25.07.1 release.
2. `z=` and `:tselect` delegating to `vim.ui.select` is nightly (0.13-dev) only, not 0.12.6.
3. snacks has 68 sources, not ~65; scoring constants live in `core/score.lua`; heap is `util/minheap.lua`.
4. blink.cmp v1.10.2 pins frizbee 0.9.0 (not 0.13).
5. frizbee benchmark is with typos off; frizbee 0.13 is Unicode-aware, not "byte-oriented".
6. Helix `overlaid` is full-width under 200 columns (5% margin only on wide terminals).
7. Helix auto-completion debounce is `completion-timeout` (250 ms default); 5 ms is for trigger
   characters and manual requests.
8. Helix "HACKS" comment is about the per-view diagnostics handler.
9. Added `eol_right_align` virt-text position; added spellbook MPL-2.0 and harper-core Apache-2.0.

**Still uncertain:**

- That Helix maintainers warned against building on the Steel branch (not located in the PR thread).
- That snacks frecency uses SQLite through LuaJIT FFI in `util/db.lua` (file exists, contents not read).
- Telescope's internals (§2.4) were not re-read against source.
- The §5 performance contracts are proposals, not measured facts.
