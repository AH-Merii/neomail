# Research: composer (a Neovim-grade drafting editor)

> Collected 2026-10-09 by a research agent for neomail's design. Each fact has a confidence level
> and a source. Re-verify anything time-sensitive before relying on it. Benchmarks were run on the
> owner's Linux machine (CachyOS, Rust 1.99.0, `--release`) with a throwaway crate in
> `/tmp/composer-research/bench`. The input was `docs/decisions.md` (11 KB) and that file repeated
> ten times (110 KB).

## Summary

- **Editor core: build our own, on ropey.** Vendor Helix's `ChangeSet`/`Transaction`,
  `Selection`, `History` (an undo tree) and the grapheme and text-object helpers, which are
  MPL-2.0 at file level. On top of them, write a Vim grammar layer: operator-pending state,
  counts, registers, dot-repeat. It plugs into neomail's own keymaps and action registry.
  - edtui and ratatui-textarea are fine widgets, but they can't support the decisions.
  - edtui has no counts and no registers. Its undo snapshots the whole buffer (at most 100).
    It has only absolute or relative line numbers and no decoration API beyond highlight ranges.
  - ratatui-textarea isn't modal at all; Vim exists only as an example.
- **Markdown highlighting: use pulldown-cmark, not tree-sitter-md.** It is about 100x faster on
  real drafts: 0.9 ms to fully parse 110 KB, against 80 ms for tree-sitter-md's two-grammar
  incremental reparse.
  - harper-core already depends on pulldown-cmark.
  - tree-sitter-md 0.5.3's `parser` feature pins `tree-sitter ^0.26`, so it clashes with
    tree-sitter 0.27, because tree-sitter declares `links = "tree-sitter"`.
  - Add small email-specific scanners for quote depth, signatures, links and addresses.
- **Markdown to email: comrak to produce HTML, then mail-builder.**
  - comrak's AST lets us inline styles per node. With `unsafe = false` it drops raw HTML.
  - ammonia is a defence-in-depth pass that also forbids remote assets.
  - mail-builder already emits `multipart/alternative` with the text part first and the HTML
    part second. It does **not** build `multipart/related` on its own: `inline()` parts land in
    `multipart/mixed` next to the alternative, so for `cid:` images we build the MIME tree
    ourselves with `MessageBuilder::body(MimePart::new("multipart/related", …))`.
  - Don't use format=flowed for the Markdown text part.
- **Spellcheck: spellbook 0.4.2 (MPL-2.0).**
  - It loads en_US in 7 ms and de in 15 ms, checks about 6 words per µs, and suggests in about
    17–26 ms per word. Suggestions should therefore run on demand (`z=`) and off the UI thread.
  - Dictionaries must be UTF-8.
  - Use the system Hunspell dictionaries on Linux. On macOS and WSL, and as a fallback, download
    them from wooorm/dictionaries; each language has its own licence.
- **Grammar: harper-core 2.11.0 (Apache-2.0).** It is English only, in five dialects.
  - On an 11 KB draft a warm lint takes about 60 ms; the first call costs about 0.5–1.7 s while
    it initialises.
  - **It adds about 15.6 MB to a stripped binary** (about 11.2 MB with `default-features = false`,
    which drops the thesaurus), so put it behind a cargo feature (on by default) and initialise
    it lazily on a worker thread.
- **Terminal presentation.**
  - crossterm 0.29 already supports undercurl (`Attribute::Undercurled`, SGR `4:3`) and
    underline colour (SGR `58`).
  - ratatui 0.30 has underline colour (the `underline-color` feature) but no undercurl. That is
    still open in PR #2740.
  - So neomail needs either a small post-render pass over the cells or a patched backend. When
    the terminal doesn't support undercurl, fall back to a plain coloured underline.
- **Completion: a blink.cmp-style engine with `CompletionSource` providers.** The sources are
  addresses (by frecency), buffer words, the dictionary (spellbook) and emoji (`emojis`). It
  shares one fuzzy matcher with the pickers, and its popups and hover are ordinary floating
  windows.

## Key facts

### 1. Editor component options

**edtui 0.11.7** (MIT, 2026-08-16, 10.2k lines; it depends on ratatui-core 0.1 and crossterm 0.29)
- [high] Text is held as `Lines` = `edtui-jagged` (a `Vec<Vec<char>>`), not a rope. Source: `edtui-0.11.7/src/state.rs`, <https://crates.io/crates/edtui>
- [high] Undo is a "brute-force" snapshot of the entire `Lines` plus the cursor for each action, capped at 100 entries, and linear: no tree. The module's own doc says it "should be replaced with an action-based mechanism". Source: `src/state/undo.rs`
- [high] It also depends on ratatui-widgets 0.3, and it does have `.` dot-repeat of the last change (tests in `src/events/key.rs`). Source: `edtui-0.11.7/Cargo.toml`, `src/events/key.rs:1237`
- [high] Keymaps can be rebound: `KeyEventHandler::{vim_mode, emacs_mode, insert, extend, remove}` with `KeyEventRegister::{n,v,i,s}(Vec<KeyInput>)`. An action is a fixed enum (`enum_dispatch`) or an `fn(&mut EditorState)`. Source: `src/events/key.rs`
- [high] There are **no counts** (`5j`, `3dd`): no digit-prefix handling exists. **No named registers**: there is one clipboard, optionally arboard. **No operator-pending grammar**: `diw`, `ci(` and so on are hard-coded key sequences. Source: grep of `src/events/key.rs`, `src/actions/*`
- [high] `LineNumbers::{None, Absolute, Relative}`, with no hybrid mode. Soft wrap is by character (`line_wrapper.rs`). Syntax highlighting is syntect (optional). Decorations are only `Highlight { start, end, style }` ranges: no signs, virtual text or underline colour. Source: `src/view.rs`, `src/state/highlight.rs`
- [high] himalaya-tui uses it for its composer (`edtui::{EditorEventHandler, EditorState}`, plus `edtui::system_editor` for the `$EDITOR` handoff). That it is the most complete drop-in Vim widget is a judgement [medium]. Source: <https://github.com/pimalaya/himalaya-tui> (`src/tui/model.rs`, `src/tui/app.rs`), `docs/research/rust-ecosystem.md`

**ratatui-textarea 0.9.3** (MIT, 2026-10-05, the ratatui-org successor of tui-textarea 0.7.0, which only supports ratatui 0.29)
- [high] It stores `Vec<String>` lines and has a linear edit history (`VecDeque` of insert and delete chunks). Its default keys are Emacs-like. Vim exists only as `examples/vim.rs`, a state machine you copy. Source: `ratatui-textarea-0.9.3/src/{textarea,history}.rs`, README
- [high] It offers `WrapMode::{None, Word, Glyph, WordOrGlyph}`, a line-number style (absolute only), and selection, search and cursor-line styles. It has no arbitrary range decorations, no relative numbers and no registers. Source: `src/textarea.rs`, `src/wrap.rs`
- [high] It is the right tool for **single-line prompts** (`:` command line, picker query), not for the composer.

**modalkit 0.0.31 + modalkit-ratatui** (Apache-2.0, 2026-10-07, 33.8k lines, pre-1.0, about 4k recent downloads)
- [high] It is a full Vim/Emacs/Kakoune keybinding environment: counts, registers, marks, macros and text objects over a ropey buffer. It is iamb's editing engine. Source: `modalkit-0.0.31/src/{env,editing}`, <https://docs.rs/modalkit>
- [medium] It brings its own action types, keybinding trie (`keybindings` crate) and buffer store. These would compete with neomail's action registry, keymaps and buffers rather than build on them. Its undo history is linear: a `HistoryList<EditRope>` of whole-rope snapshots (cheap rope clones), capped at `BUFFER_HISTORY_LEN = 100`. Source: `modalkit-0.0.31/src/editing/{history.rs,buffer/mod.rs:79,112}`
- [medium] It is worth reading as a **reference and test oracle** for Vim grammar edge cases, but not as a dependency.

**Helix core (ropey + Transactions)**
- [high] helix-core isn't published: crates.io `helix-core` is a 0.0.0 placeholder from 2021. Depending on it means a git dependency that drags in `helix-stdx`, `helix-loader`, `tree-house`, `nucleo`, `imara-diff`, `encoding_rs`, a pinned `unicode-width = "=0.1.12"` and more. Source: <https://crates.io/crates/helix-core>, <https://github.com/helix-editor/helix/blob/master/helix-core/Cargo.toml>
- [high] The latest Helix release is 25.07.1 (2025-07-18). Source: GitHub releases API
- [high] The useful modules are small and self-contained, so they can be **vendored, not depended on**:
  - `transaction.rs`: 1,186 lines. `ChangeSet` of `Retain/Delete/Insert`, `Transaction`, `map_pos` with `Assoc`, `compose`, `invert`. It depends only on `Range`, `Rope`, `Selection`, `Tendril`, `chars::char_is_word` and `smallvec`.
  - `selection.rs`: 1,458 lines. Multiple ranges with a primary.
  - `history.rs`: 634 lines. **An undo tree**: revisions with a parent, `last_child`, an inverse transaction and a timestamp, plus `:earlier`/`:later` by count or time.
  - `textobject.rs`: 593 lines. `textobject_word`, `textobject_paragraph`, `textobject_pair_surround(_closest)`, plus a tree-sitter variant you can drop.
  - `graphemes.rs`: 341 lines.

  Source: <https://github.com/helix-editor/helix/tree/master/helix-core/src>
- [high] Helix's `History` limitations, from its own doc: selection changes don't commit revisions, the revision vector is unbounded, and it stores inverse transactions because deletes don't keep the deleted text. Source: `history.rs` doc comment
- [high] `text_annotations.rs` (`InlineAnnotation` for virtual text, `Overlay` and the `LineAnnotation` trait) together with `doc_formatter.rs` (soft wrap with annotations) are Helix's design for virtual text plus soft wrap. They are coupled to Helix's `syntax::Highlight`. Port the ideas, not the files. Source: same tree
- [high] Licence: MPL-2.0 is file-level copyleft. Vendored files keep the MPL header, and any changes to them must be published as MPL. The rest of neomail stays MIT OR Apache-2.0. neomail already accepts MPL-2.0 dependencies (nucleo, spellbook). Source: <https://www.mozilla.org/en-US/MPL/2.0/FAQ/>
- [high] Helix is selection-first (Kakoune grammar), so its `movement.rs` (2,202 lines) and keymaps don't give Vim semantics. The Vim grammar (operator + count + motion or text object, registers, `.`) has to be written by us either way. Source: Helix docs, <https://docs.helix-editor.com/keymap.html>
- [high] ropey 1.6.1 (2023-10-18) is the stable release. 2.0.0-beta.1 (2025-08-02) makes byte indices the primary metric ("Ropey stores text as utf8 and uses byte indices"). Helix's workspace still pins `ropey = "1.6.1"`, so use 1.6.1 to match the vendored code. Source: <https://crates.io/crates/ropey/versions>, <https://docs.rs/ropey/2.0.0-beta.1/ropey/>, Helix root `Cargo.toml`

**Feature matrix** (✔ built in, ~ partial, ✘ missing; "own" = what we would build)

| Feature | edtui | ratatui-textarea | modalkit | own core (ropey + Helix bits) |
|---|---|---|---|---|
| Vim motions | ~ fixed set | ✘ (example) | ✔ | own grammar, Helix helpers |
| Counts | ✘ | ✘ | ✔ | own |
| Text objects | ~ hard-coded | ✘ | ✔ | Helix `textobject.rs` |
| Registers | ✘ | ~ one yank | ✔ | own (`"a`–`"z`, `"+`, `"0`, `"_`) |
| Undo tree | ✘ (100 snapshots) | ✘ linear | ✘ linear | Helix `History` |
| Abs, rel, hybrid numbers | abs/rel | abs | via widget | own gutter (trivial) |
| Soft wrap | char | word/glyph | ~ | port Helix `doc_formatter` ideas |
| Highlights, undercurl, virtual text, signs | ~ ranges only | ✘ | ✘ | shared decoration primitive |
| Completion and diagnostic floats | ✘ | ✘ | ~ completion | shared float primitive |
| Fits neomail's keymaps and actions | ✘ | n/a | ✘ (own env) | ✔ |

### 2. Markdown highlighting

**Measured on this machine** (110 KB input unless noted; median of two runs)

| Operation | Time |
|---|---|
| tree-sitter-md `MarkdownParser` full parse (block and all inline trees) | 92 ms |
| tree-sitter-md incremental reparse after a 1-character insert (block and inline) | 80 ms |
| tree-sitter-md block grammar only, full / incremental | 29.5 ms / 16 ms |
| tree-sitter-md full parse of 4 KB (a typical draft) | 3.8 ms |
| pulldown-cmark 0.13.4 `into_offset_iter()`, all extensions, full parse (8,250 events) | 0.6–0.9 ms |

- [high] tree-sitter-md 0.5.3 (MIT, 2026-02-26) ships **two grammars**: block (`LANGUAGE`) and inline (`INLINE_LANGUAGE`).
  - It provides `HIGHLIGHT_QUERY_BLOCK/INLINE` and `INJECTION_QUERY_BLOCK/INLINE`.
  - Behind the `parser` feature it adds a `MarkdownParser` that parses inline content as one tree per inline node (`MarkdownTree::inline_tree(&node)`).

  Source: `tree-sitter-md-0.5.3/bindings/rust/{lib,parser}.rs`, <https://github.com/tree-sitter-grammars/tree-sitter-markdown>
- [high] The `parser` feature of tree-sitter-md 0.5.3 requires `tree-sitter ^0.26`, while tree-sitter itself is at 0.27.1 (2026-10-08). Because tree-sitter declares `links = "tree-sitter"`, Cargo refuses to resolve both. We reproduced the error. Source: cargo resolver output in the benchmark crate
- [high] Its generated `parser.c` files are 2.0 MB (block) and 2.2 MB (inline). A stripped binary with tree-sitter 0.26 and tree-sitter-md is 1.28 MB, against a 0.34 MB empty baseline: **+0.94 MB**. It also needs a C compiler in CI. Source: measured
- [high] pulldown-cmark 0.13.4 (MIT, 2026-05-20) adds 0.44 MB stripped (with `html`). `into_offset_iter()` gives a byte range for every event. That is enough to map events to highlight groups: heading, emphasis, strong, code, link, blockquote, list marker, fence. Source: measured, <https://docs.rs/pulldown-cmark>
- [high] harper-core 2.11.0 already depends on pulldown-cmark for its Markdown parser. Source: `harper-core-2.11.0/Cargo.toml`
- [medium] For drafts of 1–20 KB, a full pulldown-cmark reparse on every edit costs under 0.2 ms, below a frame budget. Incrementality isn't needed: debounce to the frame and reparse the buffer. tree-sitter-md is slower even when incremental, because the external scanner and the inline trees are reparsed. Source: measured, extrapolated
- [medium] pulldown-cmark is a CommonMark parser, not an error-tolerant highlighter. While the user is typing, unclosed emphasis or fences change the meaning of the text below them. This is the same visible behaviour as Neovim's tree-sitter-md, so it is acceptable. Source: CommonMark spec <https://spec.commonmark.org/0.31.2/>
- [medium] Email-specific highlighting isn't Markdown syntax, so it is better done by small line scanners after the Markdown pass:
  - **quote depth:** count leading `>` (allowing `> >` and `>>`) and cycle colours per depth, like Neovim's `mailQuoted1..6`;
  - **signature:** from a line that is exactly `-- ` (RFC 3676 §4.3) to the end, or up to the quoted reply;
  - **attribution lines:** `On … wrote:`;
  - **header-like lines:** if headers live in the buffer;
  - **URLs and email addresses:** with `linkify` 0.11.0 (MIT OR Apache-2.0).

  Source: <https://www.rfc-editor.org/rfc/rfc3676#section-4.3>, <https://crates.io/crates/linkify>, Vim `syntax/mail.vim`
- [low] Tree-sitter still makes sense later for fenced code-block injections and for the message viewer. If adopted, pin one tree-sitter version across the workspace, and patch or wait for tree-sitter-md to accept 0.27. Source: judgement

### 3. Markdown to email

- [high] comrak 0.56.0 (BSD-2-Clause, 2026-10-06; +0.9 MB stripped with `default-features = false`):
  - GFM extensions: tables, strikethrough, autolink, task lists, footnotes, alerts. (The `tagfilter` extension was deprecated in 0.55 and **removed in 0.56.0**; raw HTML is handled by `unsafe = false` and ammonia instead.)
  - `hardbreaks`, `sourcepos`, `shortcodes` (emoji).
  - An arena **AST you can rewrite before rendering**, plus custom HTML formatters.
  - `render.r#unsafe = false` (the default) replaces raw HTML with a comment. `escape` escapes it instead.

  Source: `comrak-0.56.0/src/parser/options.rs`, `comrak-0.56.0/CHANGELOG.md` (PR #845), <https://docs.rs/comrak>
- [high] Disable comrak's default features (`cli`, `syntect-onig`, `bon`); they pull in clap, syntect and Oniguruma. Source: `comrak-0.56.0/Cargo.toml`
- [high] pulldown-cmark renders to HTML with `html::push_html` over an event stream. You transform the HTML by mapping events, which is easy for attributes but awkward for structural rewrites. Raw HTML in the source passes through unless the events are filtered. Source: <https://docs.rs/pulldown-cmark/latest/pulldown_cmark/html/>
- [high] mail-builder 1.0.0 (Apache-2.0 OR MIT, 2026-09-12):
  - `MessageBuilder::text_body` together with `html_body` emits `multipart/alternative` with **text first and HTML second**, the order RFC 2046 §5.1.4 requires (the last part is preferred).
  - With attachments it wraps that in `multipart/mixed`.
  - `inline(content_type, cid, bytes)` sets `Content-Disposition: inline` and `Content-ID`, but the part is pushed onto the same attachments list, so it ends up in `multipart/mixed`, **not** `multipart/related`. `multipart/related` appears only in a test that builds a custom tree with `MessageBuilder::body(MimePart::new("multipart/related", vec![…]))`; neomail must do the same for `cid:` images (`mixed > [alternative > [text, related > [html, img…]], attachments…]`).
  - It sets `charset=utf-8` and picks the transfer encoding automatically: 7bit, quoted-printable or base64.

  Source: `mail-builder-1.0.0/src/lib.rs:189-213,300-330` (`write_body_parts`), `src/mime.rs`, <https://www.rfc-editor.org/rfc/rfc2046#section-5.1.4>
- [high] mail-builder has no format=flowed support (no `flowed` in the source). Source: grep of the crate
- [medium] **Don't mark the Markdown text part as format=flowed (RFC 3676).**
  - Under flowed, a trailing space means "soft break".
  - In Markdown, two trailing spaces mean a hard break, and the signature delimiter `-- ` also ends in a space.
  - Flowed readers would therefore reflow Markdown hard breaks and code blocks.
  - Send plain `text/plain; charset=utf-8`. Lines longer than 998 octets are made safe by quoted-printable, which mail-builder chooses.

  Source: <https://www.rfc-editor.org/rfc/rfc3676>
- [medium] Email-safe HTML rules (Can I Email / Litmus consensus):
  - put **inline `style=""` attributes** on every element (Gmail drops `<style>` in some contexts, for example non-Gmail accounts in the Gmail apps);
  - use no external CSS, fonts, scripts or remote images;
  - use simple block elements: `p`, `blockquote`, `pre`, `ul`/`ol`, `table`;
  - prefer `table` for anything resembling layout;
  - give `pre` an explicit `white-space: pre-wrap` and a monospace font stack.

  Source: <https://www.caniemail.com/>, <https://www.caniemail.com/features/html-style/>
- [medium] Classic Outlook for Windows renders with Word. It ignores `max-width`, most `margin` on `div`, `border-radius` and background images. Blockquote borders work if they are set inline (`border-left: 3px solid #ccc; padding-left: 8px; margin: 0`). The new Outlook and Outlook on the web render like browsers. Source: <https://www.caniemail.com/clients/outlook/>
- [medium] The best way to inline styles is to emit them while rendering: a fixed style table keyed by node type, applied in comrak's formatter. css-inline 0.22.0 (MIT) is only needed if users supply a theme CSS file. If it is used, turn off its default features (`http` pulls in reqwest and fetches remote stylesheets; `cli` pulls in rayon). Source: `css-inline-0.22.0/Cargo.toml`
- [high] ammonia 4.2.1 is a defence-in-depth allowlist sanitiser after rendering. It should allow `style`, and `img` only with `cid:` or `data:` sources, so no remote assets ever leave the machine. Source: <https://docs.rs/ammonia>
- [medium] Pitfalls specific to Markdown drafts:
  - **The signature is a Markdown hazard.** After a blank line, `-- ` is a paragraph line, and the signature lines below it merge into one paragraph. A line `--` directly under text turns that text into a setext `<h2>`. Split the signature off before parsing, and render it as `<pre>` or with hard breaks.
  - **Lazy continuation:** an unquoted line directly after a `>` block joins the quote. The highlighter must show what the renderer will do.
  - **Quoted replies:** quote the original as `>` Markdown, so it renders as nested inline-styled `<blockquote>`. Keep the attribution (`On <date>, <name> wrote:`) outside the quote.
  - The original HTML of a replied-to message is lost. Optionally attach the original HTML quote below the rendered body as a separate, sanitised block; this is an open question.
  - **Autolinks:** enable the GFM autolink extension so bare URLs and addresses become links. In the HTML part, make `mailto:` links explicit.
  - **Plain-text-only toggle:** send only `text_body`; mail-builder then emits a single `text/plain` part.

  Source: CommonMark spec §4.3 and §5.1, <https://spec.commonmark.org/0.31.2/>, judgement
- [low] Gmail clips HTML bodies larger than about 102 KB ("[Message clipped]"), so keep the generated HTML lean. Source: <https://www.caniemail.com/> (commonly cited figure)

### 4. Spellcheck

- [high] spellbook 0.4.2 (MPL-2.0, 2026-06-03, by the-mikedavis, a Helix maintainer):
  - `no_std`; the only dependency is hashbrown (plus foldhash by default).
  - API: `Dictionary::new(aff: &str, dic: &str)`, `check(&str) -> bool`, `suggest(&str, &mut Vec<String>)`, `checker()`/`suggester()` for configured variants, and `add("word/FLAGS")` to add words at runtime (e.g. `add("foobarbaz/G")` also accepts "foobarbazing").

  Source: `spellbook-0.4.2/src/lib.rs`, <https://docs.rs/spellbook>
- [high] Maturity is "alpha" by its own README:
  - `check` works well for en_US;
  - "dictionaries which use complex compounding directives may work less well";
  - `suggest` matches Nuspell's, but **phonetic suggestions aren't implemented**;
  - almost all of the Hunspell test corpus that Nuspell uses passes.

  Source: <https://github.com/helix-editor/spellbook#maturity>
- [high] Dictionaries must be UTF-8. `new` takes `&str`, and the `.aff` parser has `// TODO: encoding? Or just require all dictionaries to be UTF-8?`. Legacy ISO-8859 dictionaries (some distro packages) must be transcoded first, with encoding_rs driven by the `.aff` `SET` line. Source: `src/aff/parser.rs:152`
- [high] Measured with wooorm dictionaries:

  | Dictionary | Load | Check 15,730 words | 5 suggestions |
  |---|---|---|---|
  | en_US (552 KB `.dic`) | 7.4 ms | 2.6 ms | 83 ms |
  | de (1.1 MB `.dic`) | 14.7 ms | 33 ms | 130 ms |

  Checking a whole draft per keystroke is cheap (microseconds per word). `suggest` costs about 17–26 ms per word, so run it only on `z=` or on a completion request, on a worker thread. `suggest("recieve")` returned `receive, relieve, reverie`. Source: measured
- [high] Stripped binary cost: about +0.37 MB. Source: measured (`sz-spellbook`)
- [medium] Users: codebook (a spell-checking LSP for Zed) and cargo-spellcheck's optional backend. Source: spellbook README
- [low] Helix plans to use it for built-in spelling. Not stated in the README; inferred from the crate living under the `helix-editor` org and its author being a Helix maintainer. Source: <https://github.com/helix-editor/spellbook>
- [medium] Alternatives:
  - zspell 0.5.5 (2024-06, non-standard licence field, stale);
  - hunspell-rs 0.4.0 (2022, FFI to C++ libhunspell, so a system dependency on macOS and WSL);
  - harper's own curated English dictionary (English only).

  spellbook is the only maintained pure-Rust, multi-language option. Source: crates.io API

**Dictionary sourcing**
- [high] Arch Linux: `hunspell-en_us` 2026.02.25 installs into `/usr/share/hunspell/`. It is LGPL-2.1+/BSD-style. On this machine no Hunspell dictionary is installed, so a fallback is needed even on Linux. Source: `pacman -Si hunspell-en_us`; `ls /usr/share/hunspell` → missing
- [medium] Distros:
  - Debian/Ubuntu (`hunspell-en-us`, etc.) and Fedora install into `/usr/share/hunspell/`;
  - some packages still use the legacy `/usr/share/myspell/dicts/`;
  - LibreOffice bundles its own under `/usr/lib/libreoffice/share/extensions/dict-*`.

  Search order: `$NEOMAIL_DICT_PATH`, then `$XDG_DATA_HOME/neomail/dict`, `/usr/share/hunspell`, `/usr/share/myspell/dicts`, `/usr/local/share/hunspell`, `/opt/homebrew/share/hunspell`. Source: Debian/Fedora package file lists, Hunspell man page <https://man.archlinux.org/man/hunspell.1>
- [medium] macOS ships no Hunspell dictionaries. Its system spellchecker (NSSpellChecker) uses its own format, and Homebrew's `hunspell` formula installs no dictionaries. Use `~/Library/Spelling` (user Hunspell files, which LibreOffice and Firefox also read) when present; otherwise download. Source: <https://formulae.brew.sh/formula/hunspell>
- [medium] WSL distros are minimal images with no dictionaries. Install with `apt install hunspell-en-us`, or download. Source: judgement and Ubuntu WSL image contents
- [high] **Download source: wooorm/dictionaries.** 92 dictionaries, all converted to UTF-8 (`de.aff` has `SET UTF-8`), at `dictionaries/<lang>/index.{aff,dic}`. Licences are per language:

  | Language | Licence |
  |---|---|
  | en, en-GB | MIT AND BSD (SCOWL) |
  | de | GPL-2.0 OR GPL-3.0 |
  | fr | MPL-2.0 |
  | es | GPL-3.0 OR LGPL-3.0 OR MPL-1.1 |
  | it | GPL-3.0 |
  | nl | BSD-3-Clause OR CC-BY-3.0 |
  | ru | BSD-3-Clause |
  | pl | GPL-3.0 OR LGPL-3.0 OR MPL-2.0 |

  So neomail must **not bundle** GPL dictionaries in its MIT/Apache binary. `neomail spell install <lang>` downloads at runtime, pins a commit, checks SHA-256, shows the licence, and stores the files in `$XDG_DATA_HOME/neomail/dict/`. Bundling en_US (MIT AND BSD) is legally fine, and costs about 0.55 MB uncompressed. Source: `package.json` per dictionary in <https://github.com/wooorm/dictionaries>
- [high] This is like Neovim's spellfile download: on `SpellFileMissing` it asks whether to download the missing `.spl` file. Current Neovim does this in Lua (`runtime/lua/nvim/spellfile.lua`, `:help package-spellfile`) from `g:spellfile_URL` or `https://ftp.nluug.nl/pub/vim/runtime/spell`, configurable with `require('nvim.spellfile').config({ url = … })`. Source: <https://github.com/neovim/neovim/blob/master/runtime/lua/nvim/spellfile.lua>, <https://neovim.io/doc/user/spell/>

**Personal word lists (`zg`)**
- [medium] Neovim keeps the format simple: `zg` appends to a plain-text `spellfile` (`~/.config/nvim/spell/en.utf-8.add`), `zw` marks a word as bad, `zug`/`zuw` undo, and `zG` is session-only. harper-ls does the same with a plain `dictionary.txt` per user, per workspace and per file. Source: <https://neovim.io/doc/user/spell.html>, <https://writewithharper.com/docs/integrations/language-server>
- [medium] Recommended:
  - files: `$XDG_DATA_HOME/neomail/spell/<lang>.add` and a global `all.add`, one word per line, with an optional Hunspell `/FLAGS` suffix;
  - load: `Dictionary::add` for each line, with no rebuild needed;
  - the same words also go into harper's `MutableDictionary`, so grammar doesn't flag them;
  - CLI: `neomail spell add <word>`;
  - an event: `SpellWordAdded`, so other buffers re-check.

  Source: spellbook `add` API, harper `MergedDictionary`

### 5. Grammar: harper-core

- [high] harper-core 2.11.0 (Apache-2.0, 2026-09-16, Automattic). It releases about every 2–4 weeks: 2.5.0 (06-11), 2.7.0 (07-28), 2.8.0 (08-13), 2.10.0 (09-10), 2.11.0 (09-16). Pin the exact version. Source: <https://crates.io/crates/harper-core/versions>
- [high] API:
  ```rust
  let dict = FstDictionary::curated();                         // Arc<FstDictionary>
  let mut lints = LintGroup::new_curated(dict, Dialect::British);
  let doc = Document::new_markdown_default_curated(text);      // also new_plain_english_curated
  let found: Vec<Lint> = lints.lint(&doc);                     // `Linter` trait
  ```
  - `Lint { span: Span<char>, lint_kind: LintKind, suggestions: Vec<Suggestion>, message: String, priority: u8 }`;
  - `Suggestion::{ReplaceWith(Vec<char>), InsertAfter(Vec<char>), Remove}`;
  - `MergedDictionary`/`MutableDictionary` for user words.

  **Spans are in chars, not bytes**, so convert to rope char indices. That is easy, because ropey indexes by char. Source: `harper-core-2.11.0/src/{document,linting/lint,linting/suggestion}.rs`
- [high] Languages: **English only**. Dialects are `American, Canadian, Australian, British, Indian`, and `Dialect` can guess the dialect from a document. `src/linting` has 353 top-level entries (336 `.rs` files plus 17 rule subdirectories; 397 `.rs` files in total), roughly one per rule. Source: `src/dict_word_metadata.rs:1078`, `find src/linting` on the 2.11.0 crate
- [high] Performance, measured:
  - first `lint` on 11 KB: **1.68 s** (lazy initialisation of models and caches);
  - same text warm: **59 ms**;
  - 110 KB: 554 ms;
  - `FstDictionary::curated()` plus `LintGroup::new_curated`: 477 ms.

  Linting must run on a background worker, debounced (about 300–500 ms after the last edit), with results tagged by buffer revision and dropped if stale. Initialise it during startup idle time or when the composer is first opened. Source: measured
- [high] **Binary size: a stripped hello-world plus harper-core is 15.9 MB, against a 0.34 MB baseline (+15.6 MB).** The cost is the embedded curated dictionary, the Brill POS tagger (`harper-brill`), the thesaurus and the rule tables. It also pulls in `ammonia`, `zip`, `fst`, `levenshtein_automata`, `trie-rs`, `cached`, `lru` and `regex`. Source: measured (`sz-harper-core`), `Cargo.toml`
- [high] The thesaurus is optional: harper-core's default features are `thesaurus` (`harper-thesaurus`) and `concurrent` (`boxcar`). With `default-features = false` the same stripped hello-world is 11.5 MB (**+11.2 MB**, about 4.4 MB less). Check that the rest of the API still compiles and lints the same without `concurrent` before relying on it. Source: `harper-core-2.11.0/Cargo.toml` `[features]`; measured by the fact-checker (`cargo build --release`, `strip = true`)
- [high] harper-ls presentation:
  - `diagnosticSeverity`: `hint` by default; also information, warning, error;
  - code actions: quick-fix replacements, `HarperIgnoreLint` (session), `HarperAddToUserDict`, `HarperAddToWSDict`, `HarperAddToFileDict`;
  - user dictionary: `$XDG_CONFIG_HOME/harper-ls/dictionary.txt`;
  - each linter can be toggled in `linters`, e.g. `SpellCheck`, `AnA`, `RepeatedWords`;
  - `dialect` setting.

  Source: <https://writewithharper.com/docs/integrations/language-server>
- [medium] Harper has its own `SpellCheck` linter. When spellbook is active, disable it, so one spelling source exists (with `z=` and `zg`) and Harper covers grammar and style only. Harper's style lints (e.g. "Use `configuration` instead of `config`") are noisy for email, so ship a curated `linters` allowlist. Source: benchmark output, harper-ls docs

### 6. Presentation in a terminal

- [high] The escape codes are the kitty extension:
  - `CSI 4:0..5 m` selects none, straight, double, **curly**, dotted or dashed underline;
  - `CSI 58:2::r:g:b m` (or `58;5;n`) sets the underline colour, and `CSI 59 m` resets it;
  - the terminfo capability `Su` advertises support.

  Source: <https://sw.kovidgoyal.net/kitty/underlines/>
- [high] crossterm 0.29.0 has `Attribute::{Undercurled, Underdotted, Underdashed, DoubleUnderlined}` and `Colored::UnderlineColor` (written as `58;…`, reset with `59`). Source: `crossterm-0.29.0/src/style/types/{attribute,colored}.rs`
- [high] ratatui 0.30 (ratatui-core / ratatui-crossterm 0.1.2):
  - `Style::underline_color` exists behind the `underline-color` feature, and the crossterm backend emits it;
  - `Modifier` has only `UNDERLINED`, with no curly, dotted or dashed variant;
  - when converting from crossterm, `Undercurled` is mapped down to `UNDERLINED`.

  PR #2740 "feat: custom underlines" (opened 2026-08-28) is still open and unmerged as of 2026-10-10. A maintainer (orhun) reviewed it on 2026-09-04 and on 2026-09-18 commented that pieces were still missing (e.g. `Stylize` helpers); there is no formal "changes requested" review. Source: `ratatui-core-0.1.2/src/style.rs`, `ratatui-crossterm-0.1.2/src/lib.rs:637,872`, <https://github.com/ratatui/ratatui/pull/2740>
- [medium] Until that PR lands, there are two workarounds:
  1. A thin wrapper around `CrosstermBackend` whose `draw` emits `SetAttribute(Undercurled)` for cells tagged in a side table (indexed by cell position, written by the decoration layer).
  2. Carry a fork patch of ratatui-core.

  Option 1 needs no fork. Source: judgement from the backend code
- [medium] Terminal support:
  - curly and coloured underlines: kitty, WezTerm, Ghostty, foot, VTE (GNOME Terminal), iTerm2 3.4+, Alacritty 0.11+ (its changelog lists `CSI 4 : [3-5] m` and `CSI 58`/`59` under 0.11.0), and Windows Terminal 1.19+ (first stable 2024-01-30; also conhost);
  - tmux turns `4:3` into a straight underline unless `terminal-overrides` includes `Smulx`/`Setulc`;
  - the Linux console and old xterm show only a straight underline.

  Fallback: the straight underline still carries the diagnostic colour where SGR 58 works. Otherwise use foreground colour or reverse video, never nothing. Source: <https://github.com/microsoft/terminal/issues/7228>, <https://github.com/tmux/tmux/issues/1492>, <https://github.com/wezterm/wezterm/issues/415>, <https://github.com/alacritty/alacritty/blob/master/CHANGELOG.md>
- [low] ConPTY on Windows (which WSL inside Windows Terminal goes through) may re-encode or drop SGR 58 and `4:3`; reports in 2026 disagree. Make undercurl a capability with a config override (`ui.undercurl = auto|on|off`) and probe it at runtime (`XTGETTCAP Su`/`Smulx`, or terminfo). Source: search results summarising microsoft/terminal discussions
- [medium] Signs: draw them in a sign column of the gutter, next to absolute, relative or hybrid line numbers. Hybrid (Neovim `number` + `relativenumber`) means: the current line shows its absolute number, the others show their distance. Neovim's diagnostics are the reference:
  - `vim.diagnostic.config({ signs, underline, virtual_text, float })`;
  - signs `E`/`W`/`I`/`H` or icons with highlight groups `DiagnosticSign*`;
  - an open float for the diagnostic under the cursor (`vim.diagnostic.open_float`);
  - `]d`/`[d` jumps; neomail's `]s`/`[s` follow Vim's spelling-only motions.

  Source: <https://neovim.io/doc/user/diagnostic.html>
- [high] blink.cmp is the completion reference. Sources are providers with:
  - `enabled()`, `get_trigger_characters()`;
  - `get_completions(ctx, callback)`, which returns `items` plus `is_incomplete_forward/backward` and an optional cancel function;
  - `resolve(item)` for lazy documentation, `execute(item)`;
  - per-provider `score_offset`, `fallbacks` and `min_keyword_length`.

  Sources return **unfiltered** items, and the engine fuzzy-matches and sorts. Source: <https://cmp.saghen.dev/development/source-boilerplate.html> (`score_offset`, `fallbacks` and `min_keyword_length` are provider options documented at <https://cmp.saghen.dev/configuration/sources.html>)
- [high] blink.cmp's matcher is frizbee 0.13.0 (MIT): a SIMD Smith-Waterman, "typo resistant", with frecency and proximity bonuses. nucleo-matcher 0.3.1 (MPL-2.0) is the alternative already planned for pickers. Source: <https://github.com/saghen/blink.cmp>, <https://crates.io/crates/frizbee>
- [high] emojis 0.9.0 ((MIT OR Apache-2.0) AND Unicode-3.0) offers O(1) lookup, GitHub shortcodes (`:tada:`) and iteration. It matches comrak's `shortcodes` feature, which uses the same crate. Source: <https://crates.io/crates/emojis>

### 7. Recommended design for neomail's composer

**Crates and pins**: ropey 1.6.1 · Helix `transaction.rs`, `selection.rs`, `history.rs`, `textobject.rs`
(minus tree-sitter) and `graphemes.rs`, vendored into an MPL-2.0 crate `neomail-text` · pulldown-cmark
0.13.4 (highlighting, shared with harper) · comrak 0.56.0 (`default-features = false`, plus `shortcodes`)
· ammonia 4.2.1 · mail-builder 1.0.0 · linkify 0.11.0 · spellbook 0.4.2 · harper-core =2.11.0
(feature `grammar`) · emojis 0.9.0 · the matcher shared with pickers (nucleo-matcher, or frizbee for
typo tolerance) · ratatui-textarea 0.9.3 only for single-line prompts.

**Layering, using only the shared primitives**
1. **Text core (`neomail-text`)**:
   - `Rope`, `Selection` (the Vim cursor is a single range; visual mode is one range or a block);
   - `Transaction` is the only way to change a buffer, and every change emits a `BufferChanged { rev, changes }` event;
   - `History` gives the undo tree: `u`, `<C-r>`, `g-`/`g+`, `:earlier 5m`. Cap the revisions, for example by size, to fix Helix's unbounded vector.
2. **Vim grammar (`neomail-vim`)**:
   - a pending-operator state machine fed by the keymap trie: `[count]["x][count]operator(motion|textobject)`, plus dot-repeat recorded as the last `Transaction`-producing action;
   - registers: unnamed, `0`–`9`, `a`–`z` (append with `A`–`Z`), `_`, `+`/`*` via the clipboard, `/`, and macros later;
   - motions and text objects are **registered actions** (`motion.word_forward`, `textobject.inner_paragraph`), so Lua and extensions add new ones the same way.
3. **Decorations**: one per-buffer store, keyed by namespace (`spell`, `grammar`, `markdown`, `search`, `ext:<name>`), holding char-indexed ranges. The kinds are highlight (style plus underline style and colour), virtual text (inline or end-of-line), sign and conceal. Ranges are mapped through each `Transaction` (`ChangeSet::map_pos`), so they stay put between provider runs.
4. **Providers**:
   - `trait DiagnosticSource { fn id(); fn languages(); async fn check(&self, snapshot: BufSnapshot) -> Vec<Diagnostic>; fn actions(&self, d: &Diagnostic) -> Vec<CodeAction>; }`;
   - `Diagnostic { range, severity, source, code, message, suggestions }`, published into the `diagnostics` decoration namespace with signs;
   - built-in providers: `spell` (spellbook, on a debounced worker; only prose nodes, skipping code spans, URLs, addresses, quoted text if configured, and the signature), `grammar` (harper, English only, its SpellCheck rule off) and `markdown-lint`, later and optional;
   - `trait CompletionSource { fn triggers(); async fn complete(ctx) -> CompletionList { items, incomplete } ; fn resolve(item) }`.
5. **Completion sources**:
   - `addresses` (SQLite contacts ranked by frecency; triggered in To/Cc/Bcc and after `@`);
   - `buffer_words` (current draft plus the quoted thread);
   - `dictionary` (spellbook; prefix matches first, `suggest` only on request);
   - `emoji` (`:` trigger, the emojis crate).

   The engine does the fuzzy matching, scoring (`score_offset`, frecency) and rendering in a **float window**. The same float primitive serves the diagnostic hover (`K` or automatic on CursorHold), the `z=` suggestion list (a picker in a float) and signature-help-like hints.
6. **Rendering**: the composer is a normal buffer and window:
   - a gutter with signs and absolute, relative or hybrid numbers;
   - soft wrap at word boundaries (ported from Helix `doc_formatter`), so `j`/`k` move by real lines and `gj`/`gk` by screen lines;
   - highlights from the Markdown and email scanners;
   - diagnostics drawn with undercurl through the backend wrapper.
7. **Send pipeline** (an action, `draft.send`):
   - split off the headers and signature;
   - comrak AST, then inline-style formatter, then ammonia allowlist (`cid:`/`data:` images only), giving the HTML;
   - the Markdown source becomes the `text/plain` part (not flowed);
   - mail-builder `text_body` plus `html_body` plus `attachment()`; when the HTML references `cid:` images, build the tree explicitly with `MessageBuilder::body()` so the HTML and its images sit in `multipart/related` (mail-builder's `inline()` alone puts them in `multipart/mixed`);
   - the per-message `plain_only` flag skips the HTML.
8. **`$EDITOR` escape hatch**: write the draft to a temporary file, then suspend the TUI, run the editor and resume. Re-read the file as one `Transaction`, so undo still works.

## Recommendations

- Build the composer on our own ropey core, with vendored Helix Transaction, Selection and History, and our own Vim grammar on neomail's keymaps and action registry. Don't adopt edtui or modalkit as the core. Use modalkit's behaviour and Vim's own tests as a conformance oracle.
- Highlight Markdown with pulldown-cmark (full reparse per frame) plus email scanners. Defer tree-sitter until code-block or viewer highlighting needs it, and keep one tree-sitter version in the workspace.
- Render email HTML with comrak and inline styles, sanitise with ammonia, and assemble with mail-builder. The Markdown source is the text part, not flowed. Handle signatures and quotes before Markdown parsing.
- Spellcheck with spellbook. Use system dictionaries first, otherwise download from wooorm/dictionaries with licence display. Bundle en_US only. Use plain-text `zg` word lists shared with harper.
- Put harper-core behind a default-on `grammar` cargo feature. Initialise it lazily, lint debounced off-thread, use a curated rule set, and keep it English-only.
- Implement undercurl with a backend wrapper now. Drop it when ratatui PR #2740 merges. Detect the capability, and offer a config override.

## Open questions

- Do headers (To/Cc/Subject) live inside the draft buffer, as in mutt or aerc, so Vim motions and address completion work there? Or are they separate single-line fields?
- When replying to an HTML message, should neomail append the original HTML (sanitised) below the rendered Markdown, or send only the Markdown-quoted text?
- Which fuzzy matcher should completion share with the pickers: nucleo-matcher (no typo tolerance) or frizbee (typo-resistant)? Run a benchmark on 100k contacts.
- Should spellbook's lack of phonetic suggestions and its weaker compound support (de, nl, hu, fi) be compensated, for example by optional hunspell-rs FFI on Linux?
- Can harper-core's size be reduced further? `default-features = false` (no thesaurus) already gets it to about +11.2 MB; a smaller dictionary would need upstream work. Is a separate `neomail-grammar` helper process worth it for minimal builds?
- What is the undo-tree memory cap, and should history persist across composer sessions (Neovim `undofile`) for long-lived drafts?

## Fact-check (independent)

> Done 2026-10-10 against crates.io API, the published `.crate` sources (2.11.0, 0.56.0 and so on,
> read from the cargo registry or downloaded from static.crates.io), GitHub source and APIs, and
> official docs. The original benchmark binary in `/tmp/composer-research/bench` was re-run twice.

**Checked and confirmed**
- Every crate version, release date and licence in the note matches crates.io: edtui 0.11.7,
  ratatui-textarea 0.9.3, tui-textarea 0.7.0 (ratatui ^0.29), modalkit 0.0.31 (about 4.0k recent
  downloads), helix-core 0.0.0 placeholder, ropey 1.6.1 / 2.0.0-beta.1, tree-sitter-md 0.5.3,
  tree-sitter 0.27.1, pulldown-cmark 0.13.4, linkify 0.11.0, comrak 0.56.0, mail-builder 1.0.0,
  css-inline 0.22.0, ammonia 4.2.1, spellbook 0.4.2, zspell 0.5.5, hunspell-rs 0.4.0, harper-core
  2.11.0 (and its 2.5–2.10 dates), crossterm 0.29.0, ratatui 0.30.2 / ratatui-core and
  ratatui-crossterm 0.1.2, frizbee 0.13.0, nucleo-matcher 0.3.1, emojis 0.9.0.
- Source-level claims: edtui (jagged `Vec<Vec<char>>`, brute-force undo capped at 100,
  `LineNumbers::{None, Absolute, Relative}`, `Highlight` ranges, keymap API, no counts or
  registers); ratatui-textarea (`Vec<String>`, `VecDeque` history, `WrapMode`, `examples/vim.rs`);
  modalkit (33.8k lines, ropey, `keybindings` crate); Helix file line counts (1,186 / 1,458 / 634 /
  593 / 341 / 2,202), `History` limitations doc, `last_child`, `text_annotations` coupling to
  `syntax::Highlight`, helix-core deps, release 25.07.1; tree-sitter-md constants, `inline_tree`,
  `parser = ["dep:tree-sitter"]` with `tree-sitter = "0.26"`, tree-sitter `links = "tree-sitter"`,
  `parser.c` sizes; comrak options and default features; mail-builder text-before-HTML order,
  auto transfer encoding, no `flowed`; spellbook API, `no_std`, deps, `parser.rs:152` TODO, README
  maturity text, `add("foobarbaz/G")`; harper API, `Lint` fields, `Suggestion`, dialects;
  crossterm `4:` attributes and `58`/`59`; ratatui `underline-color` feature and the
  `Undercurled -> UNDERLINED` mapping; kitty underline spec and `Su`; harper-ls settings and code
  actions; blink.cmp source API; frizbee README; css-inline features; wooorm licences (92
  dictionaries, `de` has `SET UTF-8`, en `.dic` 552 KB); Arch `hunspell-en_us` 2026.02.25; Homebrew
  `hunspell` ships no dictionaries and points to `~/Library/Spelling`.
- Benchmarks reproduce. `docs/decisions.md` has since grown to 13.3 KB, and the re-run gave:
  spellbook en_US load 7.9 ms, de 14.2 ms, suggest 83 / 130 ms per 5 words; harper first lint
  1.67 s, warm 72 ms, 10x 675 ms, init 491 ms; tree-sitter-md full 97 ms, incremental 84 ms,
  block-only 31 / 17 ms, 4 KB 3.5 ms; pulldown-cmark 0.68 ms. Binary sizes in the `sz-*` probes
  match (base 0.34 MB; comrak +0.89, pulldown-cmark +0.44, spellbook +0.37, tree-sitter-md +0.94,
  harper +15.6 MB).

**Corrected in place**
- comrak 0.56.0 **removed** the `tagfilter` extension (CHANGELOG, PR #845); it was listed as available.
- mail-builder 1.0.0 does **not** build `multipart/related`: `inline()` parts go into
  `multipart/mixed`. Fixed in the Summary, section 3 and the send pipeline.
- Alacritty gained curly/coloured underlines in 0.11.0, not 0.12.
- Neovim's spell download is now Lua `nvim.spellfile` (`package-spellfile`), URL
  `https://ftp.nluug.nl/pub/vim/runtime/spell`, not `spellfile.vim`.
- ratatui PR #2740: there is no formal "changes requested" review; a maintainer comment on
  2026-09-18 listed missing work.
- harper `src/linting`: 353 is the count of top-level entries (336 files + 17 directories).
- Added: harper's `thesaurus` is a default feature; without default features it is +11.2 MB.
- Added: edtui has `.` dot-repeat and also depends on ratatui-widgets; modalkit undo is capped
  whole-rope snapshots; Helix `transaction.rs` also needs `chars::char_is_word` and smallvec.
- Raised confidence: himalaya-tui uses edtui (confirmed in its source), ropey 2 byte indexing.
- Lowered to [low]: "Helix plans to use spellbook" (not in the README).

**Still uncertain**
- Gmail's ~102 KB clipping threshold (widely reported, not documented by Google).
- Outlook rendering details and the email-safe HTML rules (from caniemail, not re-tested).
- The tmux behaviour (`Smulx`/`Setulc` overrides) and the ConPTY/WSL undercurl claims.
- The distro dictionary paths for Debian and Fedora, and the WSL image contents (not inspected).
- Whether harper-core lints identically without the `concurrent` feature.
