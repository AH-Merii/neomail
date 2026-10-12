# Research: fuzzy-search

> Collected 2026-10-09/10 by a research agent for neomail's design. Facts carry a confidence
> level and a source; re-verify anything time-sensitive before relying on it. Numbers marked
> "measured here" come from benchmarks run for this note on the Enron corpus (517,401 messages,
> 951 MB of body text) on a 16-thread Linux machine with SQLite 3.53.4, nucleo 0.5.0,
> nucleo-matcher 0.3.1, frizbee 0.13.0 and tantivy 0.26.2. The benchmark scripts were kept
> outside the repository.

## Summary

Helix's speed comes from two primitives:
- **nucleo-matcher**, an fzf-compatible Smith-Waterman matcher with a one-row matrix, memchr
  prefilters and an ASCII fast path;
- **the nucleo engine**, which provides wait-free streaming injection, a rayon worker that only
  re-scores the previous matches when the query grows, cancellation by atomic flag, and a
  `tick(timeout)` / `Snapshot` contract that never blocks the UI.

Helix adds versioned injectors (for cancelling producers), debounced "dynamic" pickers that
re-run grep, and per-path preview caches that are highlighted asynchronously.

Benchmarks run for this note on the Enron corpus (517k messages, 951 MB of text):
- nucleo's engine narrows 517k sender+subject strings in 0.7–9 ms per keystroke and re-scores
  everything in 15–22 ms on 16 threads.
- frizbee is 1.4 to 7.5 times faster per thread. It is MIT and typo-tolerant, but has no engine
  and no Unicode normalisation.
- For bodies, SQLite FTS5 with a unicode61 index (514 MB, 0.54× the text) returns the newest 50
  matches in ≤ 0.1 ms, *if rowids are time-ordered*. bm25 or date-sorted results take
  10–300 ms.
- A trigram index with detail=full is not worth it (3 GB). With detail=none it is a cheap
  optional substring/CJK index (365 MB).
- tantivy is faster for ranked queries (≤ 2.7 ms, 336 MB), but is a second store with 125
  crates. Not for v1.

Recommendations:
- nucleo behind a seam, with an object-safe `PickerSource` provider;
- an FTS candidate producer plus an in-memory fuzzy layer;
- time-ordered rowids;
- Firefox-style exponential-decay frecency stored as one column;
- scope as picker state;
- a Gmail-subset query language shared by the TUI, CLI and MCP.

MPL-2.0 is fine for an MIT OR Apache-2.0 project. It needs a per-crate cargo-deny exception and
a source pointer in the release notices.

## 1. How Helix's picker works

Sources: nucleo at commit `8c16d47` (2026-06-23) and Helix `master` at 2026-09-29, both read
directly.

### 1.1 The matcher (`nucleo-matcher` 0.3.1)

- **Same scoring model as fzf, better alignment.** It uses fzf's scoring constants
  (`SCORE_MATCH = 16`, `PENALTY_GAP_START = 3`, `PENALTY_GAP_EXTENSION = 1`, a boundary bonus of
  8, a camelCase/digit bonus of 5, a consecutive bonus of 4, and double weight on the first
  character's bonus). The difference is that nucleo implements real Smith-Waterman with affine
  gaps (two matrices). fzf's v2 algorithm approximates this. So nucleo picks the intuitive
  alignment more often: for `foo` in `xf foo` it matches `x__foo`, while fzf matches `xf_oo`.
- **Memory.** While ranking, it keeps one matrix row instead of a full matrix. The row is only
  `n - m + 1` wide, because the p-th needle character needs p-1 characters before it and m-p
  after it. A full matrix holding two direction bits per cell is built only when the caller asks
  for match indices, for highlighting. fzf allocates the full `m × n` matrix even when it only
  needs a score.
- **Optimisations that matter most:**
  1. Unicode is segmented once, when an item is added. Items are stored as `Utf32String`, which
     is either ASCII bytes or `char`s.
  2. Aggressive `memchr` prefiltering rejects haystacks that cannot match before any dynamic
     programming. For case-insensitive ASCII it uses `memchr2`.
  3. An ASCII fast path keeps bytes instead of `char`s, so more of the haystack fits in cache.
  4. Very long matches fall back to a greedy O(n) matcher, avoiding the O(mn) blow-up.
- **Query syntax** follows fzf: space-separated atoms, with `'exact`, `^prefix`, `suffix$`,
  `!negate` and `\ ` escapes. Smart case and Unicode normalisation are options. Normalisation is
  why `a` matches `á`.
- **Graphemes.** Each grapheme is matched on its first code point, and grapheme indices are
  returned, so highlights never split a cluster. fzf and skim work on code points instead.

### 1.2 The engine (`nucleo` 0.5.0)

- **`Nucleo<T>`** owns a rayon `ThreadPool` (all cores by default), a lock-free append-only item
  store (`boxcar`, adapted from the MIT `boxcar` crate), a `MultiPattern` (one pattern per column)
  and a `Snapshot<T>` the UI reads from.
- **`Injector<T>`** is a cheap `Clone`. `push(value, |&value, cols| …)` fills the matcher
  columns (`Utf32String`s) once, appends the item wait-free, and calls the `notify` callback. Any
  number of producer threads can push at once without extra buffering. That is how Helix streams
  results from parallel directory walkers.
- **`tick(timeout_ms)`** is called by the UI on every redraw. It:
  1. checks whether the pattern changed. If so, it sets a `canceled` atomic so the running pass
     stops early;
  2. tries for up to `timeout` ms to take the worker lock. Helix uses 10 ms; the docs recommend
     10 ms;
  3. if the worker finished, swaps its results into the `Snapshot`;
  4. if there is new work (a pattern change or new items), starts another pass on the pool.

  It returns `Status { changed, running }`. While `running` is true the UI shows a spinner, and
  the worker calls `notify` (Helix passes `request_redraw`) when it finishes, which triggers the
  next tick.
- **Incremental re-matching.** `pattern.reparse(col, text, case, norm, append)`:
  - If `append` is true (the new text extends the old) and the change is safe, the column is
    marked `Update`. The next pass then re-scores *only the current matches*, in parallel, with
    `par_iter_mut().take_any_while(!canceled)`. Items that no longer match get `idx = u32::MAX`
    and are truncated after sorting.
  - Otherwise the column is marked `Rescore` and every item is matched again.
  - Items that arrive while a pass is running stay "in flight" and are matched in the next pass.
- **Sorting.** Results are sorted with a cancellable parallel quicksort (`par_sort.rs`), by score
  and then by index (or by index only with `sort_results(false)`). Measured sorting cost is
  negligible next to matching (author's note in the README).
- **`restart(clear_snapshot)`** drops every item, for example when a live query changes the
  candidate set. `Snapshot::matched_items(range)` gives the visible window without copying
  everything.

### 1.3 Helix's picker on top of nucleo

- **Columns.** Pickers declare `Column`s, some of which take part in filtering. In the query,
  `%column text` targets a specific column, and plain text targets the primary one
  (`ui/picker/query.rs`). This is "scoped query syntax" built from the same primitive.
- **Versioned injectors for cancellation.** Helix wraps nucleo's injector with
  `version: usize` and `picker_version: Arc<AtomicUsize>`. `push()` fails with
  `InjectorShutdown` once the picker's version moves on, and the producer stops. A
  `RequestRedrawOnDrop` marker repaints when the last producer finishes, which clears the
  spinner.
- **Dynamic (live) pickers** such as global search re-run the producer when the query changes.
  `DynamicQueryHandler` debounces (global search uses 275 ms, and the default is 100 ms), bumps
  the version to cancel the old run, restarts nucleo and runs the new producer.
- **Global search.**
  1. A `grep_regex::RegexMatcher` (smart case, multi-line) is built from the query.
  2. `grep_searcher::Searcher` runs with `BinaryDetection::quit(0)`.
  3. `ignore::WalkBuilder::build_parallel()` walks the tree. Each walker thread runs a `UTF8`
     sink that pushes `FileResult { path, line }` into the injector and returns
     `WalkState::Quit` once the injector reports shutdown.
  4. Files that are already open are searched from their in-memory buffer, not from disk.
  5. The fuzzy matcher then filters those streamed lines with an optional second pattern on the
     path column.
- **Previews.**
  - Previews are cached per path in `HashMap<Arc<Path>, CachedPreview>`. The variants are
    `Document`, `Directory`, `Binary`, `LargeFile` (over 10 MiB) and `NotFound`.
  - The binary check reads only the first 1 KiB into a reused `read_buffer`.
  - Syntax highlighting is not done on the render path. A `PreviewHighlightHandler`, debounced
    by 150 ms, parses the document on a background task. It drops the result if the picker has
    closed, and requests a redraw when it finishes.
  - The preview is drawn plain first and highlighted when the background parse finishes.

**Lessons for neomail:**
1. The matcher thread never owns the UI. The UI polls with a short, bounded wait.
2. Producers stream into a lock-free store and are cancelled by a version counter.
3. Re-running the producer ("live" search) and fuzzy-filtering its output are two separate
   layers.
4. Previews are cached per item and decorated asynchronously.

## 2. Matchers compared: nucleo, fzf, skim / fuzzy-matcher, frizbee

| | nucleo-matcher 0.3.1 | fzf (Go) | fuzzy-matcher 0.3.7 (skim's legacy `skim_v2` scorer) | frizbee 0.13.0 |
|---|---|---|---|---|
| Algorithm | Smith-Waterman, affine gaps, two matrices, one row kept | fzf v2: approximate SW, full `m×n` matrix | Smith-Waterman variant ("skim v2") | SW with affine gaps, row-wise SIMD (SSE/NEON/AVX2/AVX-512), u8 scores when they fit |
| Text model | Pre-segmented UTF-32 (ASCII stored as bytes) | Code points | Code points; bonuses and case folding only for ASCII | Raw UTF-8 bytes; Unicode path only when the needle is non-ASCII |
| Unicode normalisation (`a` matches `á`) | Yes (option) | Yes | No | **No** (documented limitation) |
| Typo tolerance | No | No | No | Yes (`max_typos`) |
| Streaming engine | Yes (`nucleo` crate) | Built-in | skim app | No (matcher plus `match_list_parallel` only) |
| Licence | MPL-2.0 | MIT | MIT | MIT |
| Last release | 2024-02-20 (engine 0.5.0: 2024-04-02) | active | 2020-10-04 | 2026-08-13 |

Published benchmarks (other people's machines):
- **nucleo README, Linux kernel paths:** nucleo takes 2.1 to 9.5 ms where skim takes 17 to 35 ms,
  about 3.7 to 8 times faster. Pasting `///.` into a list of about 3 million items takes about
  1 s in fzf and about one frame (1/30 s) in nucleo, on a Ryzen 5950X.
- **frizbee BENCHMARKS.md, Ryzen 9950X3D:**
  - Chromium paths, 1.41 million items, needle `linux`:
    - single-threaded: nucleo 90.5 ms, fzf 120.8 ms, frizbee 22.4 ms;
    - 8 threads: nucleo 36.8 ms, fzf 16.4 ms, frizbee 3.5 ms;
    - frizbee with 1 typo allowed: 60.8 ms single-threaded and 9.5 ms on 8 threads.
  - Korean text (281k sentences): nucleo 104.0 ms, fzf 114.5 ms, frizbee 2.5 ms,
    single-threaded. (Arabic, 286k sentences: nucleo 119.6 ms, fzf 165.9 ms, frizbee 2.6 ms.)
  - Note that the nucleo baseline there runs with normalisation turned off. The "8 threads"
    nucleo figure *does* use nucleo's own engine (`Nucleo::new(…, Some(8), 1)`, then
    `reparse(…, append = false)` and `tick(1000)` until done), so it is a full re-score
    through the engine, not the incremental-narrowing case.
- **Adoption.** Helix uses nucleo. television (MIT), atuin and blink.cmp depend on frizbee;
  fff.nvim uses a fork (`neo_frizbee`). skim 5.x (MIT) compiles frizbee in through its default
  `frizbee` cargo feature, but only as an optional `--algo frizbee`; skim's default algorithm
  is its own `arinae`. television wrote its
  own ~1,600-line engine around frizbee (`television/matcher/`):
  - an append-only store;
  - a worker thread;
  - passes published progressively in chunks of doubling size;
  - frecency "promotion".

  That is the code neomail would have to write if it chose frizbee.
- **Quality.**
  - nucleo and fzf share a scoring model, so their rankings are close. nucleo finds the optimal
    alignment more often.
  - frizbee uses its own bonuses: prefix, delimiter, capitalisation, matching case and exact
    match.
  - fuzzy-matcher's bonuses are ASCII-only (nucleo README), so it is the weakest of the four
    for non-English names.
  - frizbee's typo tolerance helps people's names ("jef skiling"). It can be capped, for example
    at needle length / 4.

**Measured here** on 517,401 `sender + subject` strings (mean about 60 characters). Every run was
warm, used smart case, and was single-threaded unless marked otherwise:

| Query | nucleo-matcher, 1 thread | frizbee, 1 thread | frizbee, 16 threads | Hits (nucleo / frizbee) |
|---|---|---|---|---|
| `e` | 41 ms | 30 ms | 6.6 ms | 515,825 / 515,825 |
| `enron` | 257 ms | 78 ms | 10.0 ms | 467,305 / 467,305 |
| `meet` | 170 ms | 48 ms | 7.9 ms | 291,727 / 291,727 |
| `conf call` | 116 ms | 15.5 ms | 2.6 ms | 43,978 / 8,687 |
| `gas pric` | 80 ms | 12.6 ms | 2.4 ms | 24,227 / 5,272 |
| `jeff skil` | 25 ms | 6.6 ms | 1.9 ms | 2,456 / 888 |
| `zzqx` | 11.5 ms | 4.7 ms | 1.8 ms | 5 / 5 |

How to read the table:
- frizbee is 1.4 to 7.5 times faster per thread.
- nucleo splits on spaces into AND-ed atoms that can appear in any order. frizbee's
  `Matcher::new` treats the space as part of a single needle, which explains the different hit
  counts. `Matcher::from_query` gives atom semantics.
- Through nucleo's *engine* on 16 threads, the same 517k items need:
  - 15–22 ms for a full re-score after a non-append edit;
  - 9 ms for `je`, 6 ms for `jef` and 0.7–3 ms for longer appends (incremental narrowing);
  - 46 ms to inject everything.

  In every case the first `tick(10)` returned within 10.1 ms. Both libraries already fit a
  16 ms frame on this machine. On a 4-core laptop, expect roughly four times the engine numbers
  for full re-scores. Incremental narrowing stays under one frame.

## 3. Licences: depending on MPL-2.0 (nucleo) from an MIT OR Apache-2.0 project

- **It is fine.** The MPL is a *file-level* copyleft. Files that contain no MPL code are not
  "Modifications" and can carry any licence. Combining MPL code into a "Larger Work" is allowed
  (MPL 2.0 §3.3), including static linking (Mozilla MPL FAQ Q11). Helix is itself MPL-2.0, and
  many MIT/Apache Rust TUIs depend on nucleo.
- **Obligations arise only when distributing.**
  - Using unmodified MPL code needs nothing (FAQ Q5).
  - When neomail ships binaries containing nucleo, it must tell recipients where to get the
    source of the MPL-covered files (FAQ Q8 and Q10, §3.2). In practice, list nucleo, its
    version, MPL-2.0 and the upstream/crates.io URL in a third-party notices file shipped with
    each release. `cargo about` (or `cargo-bundle-licenses`) can generate it.
  - If neomail *modifies* nucleo files (for example by vendoring a fork), those modified files
    must stay MPL-2.0 and their source must be published. Keep any fork as a separate crate with
    its own MPL licence, and never copy nucleo code into neomail's MIT/Apache files.
  - neomail's own licence is unaffected. Downstream users of neomail inherit the same light
    notice duty.
- **frizbee is MIT,** so it needs only the usual notice. The `boxcar` code inside nucleo is
  MIT-derived and already attributed in its source.
- **cargo-deny.** The `copyleft` key has been removed: every licence is denied unless allowed.
  Either add `"MPL-2.0"` to `[licenses].allow`, or (stricter, and recommended) allow it only for
  the crates that need it:

  ```toml
  [licenses]
  allow = ["MIT", "Apache-2.0", "Apache-2.0 WITH LLVM-exception", "BSD-2-Clause",
           "BSD-3-Clause", "ISC", "Unicode-3.0", "Zlib", "CC0-1.0"]
  confidence-threshold = 0.93

  [[licenses.exceptions]]
  crate = "nucleo"
  allow = ["MPL-2.0"]

  [[licenses.exceptions]]
  crate = "nucleo-matcher"
  allow = ["MPL-2.0"]
  ```

  Other crates that commonly need MPL-2.0 are `option-ext` 0.2.0 (pulled in through
  `dirs`/`dirs-sys`), `smartstring` 1.0.1 (`MPL-2.0+`; whether an `MPL-2.0` allow entry covers the `+` form was not
  verified) and `colored` 3.1.1. `webpki-roots` and `webpki-root-certs` 1.0.9 use
  `CDLA-Permissive-2.0`, which also needs allowing if rustls's bundled roots are used. Run `cargo deny check licenses` once the dependency tree exists, then add an
  exception for each crate it reports.

## 4. Searching mail bodies at 100k to 1M messages (measured here)

**Corpus.** Enron maildir: 517,401 messages, 951 MB of body text and 14.7 MB of subjects. The
base SQLite table takes 1,237 MB. Each FTS5 table indexes `(subject, body)` as an
external-content table (`content='m'`), so the text is not stored twice. Every query was run
twice and the second (warm-cache) run was timed.

| Index | Build | Index size | Size ÷ text | Typical query |
|---|---|---|---|---|
| FTS5 `unicode61 remove_diacritics 2`, detail=full | 44 s | 514 MB | 0.54× | count 0.3–3.8 ms; first or newest 50 by rowid 0.0–0.1 ms; **bm25 top-50: 8.5 ms (6.5k hits), 22 ms (phrase, 11k), 30 ms (14.6k), 76 ms (62k), 311 ms (`enron`, ~250k)** |
| same plus `prefix='2 3'` | 101 s | 1,171 MB | 1.23× | `skil*` bm25 15.5 ms; `ga*` bm25 143 ms; `conf* call` first-50 14 ms |
| FTS5 `trigram`, detail=full | 306 s | 3,036 MB | 3.2× | bm25 top-50 39–164 ms; count 3–55 ms; `LIKE '%skilli%'` count 62 ms |
| FTS5 `trigram`, **detail=none** | 136 s | **365 MB** | 0.38× | `LIKE '%…%'` newest-50 **0.2–6.3 ms**; LIKE count 179–272 ms. No phrase queries, and MATCH tokens can be at most 3 characters |
| No index, `LIKE '%skilli%'` over `m.body` | none | 0 | none | 1,206 ms |
| tantivy 0.26.2 (subject and body as `TEXT` with positions; id `FAST`) | **9.8 s** (multi-threaded writer, 1 GB budget) | **336 MB** | 0.35× | **BM25 top-50 plus count: 0.1–2.7 ms**, including `enron` (250k hits) in 1.8 ms and a phrase in 2.7 ms |

More measurements on the word index:
- Ordering hits **by `m.date`** through a join costs 11 ms (6.5k hits), 25 ms (11k), 88 ms (62k)
  and 244 ms (250k). Even with an index on `date`, SQLite has to collect every hit first.
- Ordering **by FTS rowid** (`ORDER BY rowid DESC LIMIT 50`) costs 0.0–0.1 ms whatever the hit
  count, because FTS5 walks the doclist backwards.
- Fetching 2,000 candidate rowids takes 0.4–1.1 ms. Fetching 2,000 candidates *with* their
  `subject` and `sender` rows takes 3.7–5.3 ms.
- FTS combined with a metadata filter (`sender LIKE 'jeff%'`), ordered by rowid, limit 50:
  2.7 ms.

What this means:
- **Make the FTS rowid time-ordered.** If the message's integer key sorts by date (for example
  `(internal_date_secs << 16) | tiebreak`, assigned at insert so backfilled old mail gets an old
  key), then "newest matching mail first" costs 0.1 ms at 500k messages. Sorting by date the
  usual way costs up to 244 ms. Mail search is recency-first for almost everyone (Gmail's default
  sort is date).
- **bm25 is fine for selective queries and slow for broad ones.** Treat relevance as a sort mode
  toggled in the picker, not the default. For bm25 top-k, add a `WHERE rowid > (cutoff)` window
  or cap the candidates first.
- **Trigram with detail=full is not worth it.** It is six times the size of the word index for
  slower ranked queries. **Trigram with detail=none** is small (0.38× the text) and gives true
  substring and CJK matching through `LIKE`/`GLOB`. That makes it a good *optional* second
  index, for CJK users or "match inside words". Queries shorter than 3 characters fall back to a
  scan. Do not set `remove_diacritics` on that trigram index: the SQLite docs say LIKE/GLOB are
  only indexed when it is unset.
- **Prefix indexes** (`prefix='2 3'`) more than double the index for a modest gain. Without
  one, appending `*` to the last token while typing gives these newest-50 times:
  - `skil*` 1.2 ms, `pipel*` 1.6 ms;
  - `conf* call` 15 ms, `ga*` 25 ms;
  - `s*` 1,061 ms.

  So add `*` only once the last token has at least 3 characters. Shorter prefixes are left to
  the in-memory fuzzy layer.
- **Size at 1M messages.** Scaling linearly gives roughly 1.0 GB for the word index (0.7 GB for
  the optional trigram detail=none index) on top of the bodies. Real mailboxes have more
  HTML-derived text, so index the *plain-text rendering* of each body, not the raw HTML, and cap
  very large bodies (for example the first 256 KiB).
- **tantivy** is 10–100× faster for *ranked* queries, about 35% smaller than FTS5, and builds in
  parallel (4.5× faster). It also has fuzzy term queries (Levenshtein automata) and faceting.
  The costs:
  - it is a second store that must stay transactionally in step with SQLite;
  - it adds a large dependency: 125 other crates in its normal dependency tree across all
    targets (108 on x86_64 Linux), against 16 for nucleo;
  - commits and segment merges have to be managed.

  **Verdict: not for v1.** FTS5 with time-ordered rowids already meets the one-frame budget for
  the default newest-first view, with one database and one transaction. Keep `SearchIndex` a
  provider trait so tantivy can be added later if users ask for relevance-ranked search over
  more than 1M messages.

### Candidate generation vs everything in memory

- **Everything in memory with nucleo** is the right model for *short fields*: senders,
  subjects, folders, labels, accounts, keymaps, actions and themes.
  - 517k "sender + subject" strings take about 36 ms to convert to UTF-32 and 46 ms to inject
    into nucleo.
  - Memory is roughly 4 bytes per character for non-ASCII and 1 byte for ASCII, plus per-item overhead. That is an estimated 40–70 MB here, not measured.
  - A full 16-thread pass takes 15–22 ms, and incremental narrowing takes 0.7–9 ms (section 7).
- **Bodies must never go to the fuzzy matcher.** 951 MB of text is too large to hold or scan
  per keystroke, and fuzzy subsequence matching over long text matches nearly everything. In
  this test `meet` fuzzy-matched 291,727 of 517,401 *sender+subject* strings.
- **Hybrid (recommended for "live text search over all mail"):**
  1. FTS5 generates candidates newest-first, in pages of 500–2,000 rowids (under 5 ms each).
  2. They are streamed into a nucleo instance whose columns are `subject`, `from` and `snippet`.
  3. A second, optional fuzzy pattern narrows or re-ranks them in memory. This is snacks.nvim's
     `search` (re-runs the finder) vs `pattern` (filters with the matcher) split, and Helix's
     dynamic picker.
  4. Editing FTS terms restarts the producer, and editing the fuzzy part only re-matches.
- **Metadata filters** (`from:`, `account:`, dates, flags) become SQL `WHERE` clauses on indexed
  columns. When the query has no free text, they run without FTS.

## 5. Frecency for ranking senders

| Scheme | Formula | Storage | Notes |
|---|---|---|---|
| **Firefox (current)** | `score = (Σ wᵢ · e^(−λ(t_ref − tᵢ)) / samples) · visit_count` over the last 10 sampled visits (`numSampledVisits`), with `λ = ln 2 / 30 days`. The weight bucket depends on the visit type (very high, high, medium, low) | **One number per item:** `frecency = t_ref + ln(score)/λ`, "the number of days from the Unix epoch until the score would decay to 1" | Replaced the legacy algorithm, whose scores depended on when they were computed, needed daily decay of every row and had too many coefficients. Sorting by the stored number *is* sorting by current score, with no decay jobs |
| Firefox (legacy) | Up to 10 recent visits sampled. Each visit gets a recency bucket weight (≈ 4 days: 100, 14 days: 70, 31 days: 50, 90 days: 30, older: 10; unverified, see the fact-check) times a visit-type bonus. Then visit count × the average | Per-visit rows plus a cached score | Recomputed by idle jobs; what the original telescope-frecency copied |
| zoxide | +1 per access. The rank is multiplied by 4 if accessed within the last hour, 2 within a day, ½ within a week, ¼ otherwise | rank plus last-access time per entry | When the total exceeds `_ZO_MAXAGE`, every rank is scaled to about 90% of it and entries below 1 are removed |
| telescope-frecency v1 | `count × Σ bucket(ageᵢ) / #timestamps`. Buckets: 4 h: 100, 1 day: 80, 3 days: 60, 1 week: 40, 1 month: 20, 90 days: 10. At most 10 timestamps | count plus timestamps | Superseded by v2 |
| **telescope-frecency v2 and `fre`** | Each access adds 1 at the reference time; `score(now) = stored / 2^((now − t_ref)/half_life)` | `{reference_time, half_life}` per database, plus `score`, `num_accesses` and `last_accessed` per item | Default half-life is **3 days** in both |
| smart-open.nvim | A weighted sum of sigmoid-normalised signals (fuzzy path 140, name 131, frecency 17, recency 9, proximity 13, project 10, open 3, alt 4) | The weights themselves are stored | Weights are **learned online**: after each pick, items ranked above the chosen one are penalised (`ADJUSTMENT_POINTS = 0.6`) |

**Recommendation for neomail: Firefox's exponential decay, stored as a single "date of score 1"
value.** It needs one indexed `REAL` column, never needs a background decay pass, and is the
same algorithm Firefox, fre and telescope-frecency v2 converged on.

```text
half-life h      = 30 days (configurable, contacts change slower than files)
current score    s(now) = 2^((F − now) / h)      # F is the stored column
on an event (w)  s' = s(now) + w ;  F = now + h · log2(s')
sort descending by F == sort descending by current score
```

- **Table:** `sender_frecency(address TEXT PRIMARY KEY, account_id, frecency REAL,
  last_event INTEGER, events INTEGER)` with `INDEX(frecency DESC)`. Rows are keyed on the
  normalised address (lower-case, `+tag` stripped only for display grouping).
- **Event weights** come from the action registry, so extensions can add their own:
  - replying to or sending mail to the address: 4;
  - choosing it in a picker: 3;
  - starring or flagging its mail: 2;
  - opening one of its messages: 1;
  - receiving mail from it: 0 (or 0.1). Volume alone must not rank a newsletter above a person.
  - Marking mail from the sender as spam deletes the row.
- **Seeding.** On first sync, replay the `Sent` folder (sending is the strongest signal) and the
  read flags of the last 90 days through the same function, using their historical timestamps.
- **Combining with the fuzzy score.**
  - With an empty query, sort by `F`. An index scan returns the top 100 in microseconds.
  - With a query, nucleo sorts by fuzzy score only. Re-rank the **top window** (for example the
    first 200 of the `Snapshot`) on the UI thread with
    `final = fuzzy + α · log2(1 + s(now))`, where α is about 8 (one boundary bonus). This is
    cheap and stable.
  - Ties keep nucleo's order. Items are injected in descending-`F` order, so ties also favour
    frecency.

  television does the same with "promoted" frecency entries. smart-open's online weight learning
  is an interesting later option, but it makes rankings hard to explain.

## 6. Scopes and query syntax

**Scopes are picker state, not query text.** The scope is shown as a chip in the prompt
(`[thread] ▸ budget q3`). The scope keys from the decisions doc map to:

| Scope | Meaning | How a source applies it |
|---|---|---|
| `draft` / `message` | Current buffer | The source switches to buffer lines (an in-memory nucleo pass over lines, like Telescope's `current_buffer_fuzzy_find`) |
| `thread` | Messages in the current thread | `WHERE thread_id = ?` |
| `folder` / `group` | Current folder, label or jev group | `WHERE mailbox_id = ?` or `group = ?` |
| `account` | One account | `WHERE account_id = ?` |
| `all` | Everything | No filter |

- Switching scope cycles the scope (proposed keys: `<C-s>`/`<C-S-s>`, or `<Tab>` on the chip)
  while keeping the query text. The scope becomes extra structured filters. For SQL-backed
  sources it restarts the producer; for in-memory sources it re-runs the matcher only.
- Sources declare which scopes they support (`fn scopes(&self) -> &[ScopeKind]`). The picker
  skips the ones a source does not support.

**Query grammar (Gmail-compatible subset, so it is guessable):**

```text
query   := term*
term    := ['-'] (filter | phrase | word)
filter  := key ':' value          value := word | "quoted" | '(' word* ')'
key     := from | to | cc | subject | account | group | folder | label | in
         | is   (unread|read|starred|flagged|draft|sent|replied)
         | has  (attachment|link|calendar)
         | before | after   (YYYY-MM-DD, YYYY/MM/DD, today, yesterday)
         | older_than | newer_than   (Nd | Nw | Nm | Ny)
         | filename | larger | smaller
phrase  := '"' … '"'
```

- **Filters** compile to SQL `WHERE` clauses on indexed columns. `from:`/`to:` match name or
  address. `from:` values get address-book completion from the senders picker, which reuses the
  same completion float.
- **Free text** has two meanings depending on the source:
  - In **message search**, words and phrases become an FTS5 `MATCH`, with the last token
    prefix-expanded once it has 3 or more characters.
  - In **in-memory pickers** (senders, folders, actions), the free text is the nucleo pattern
    and keeps fzf atoms (`'exact ^prefix suffix$ !neg`).
  - Message search can take an optional *second* fuzzy stage after a separator (proposal: `//`,
    or toggled with `<C-g>` as in snacks.nvim's `toggle_live`). It filters the streamed
    candidates in memory, for example `from:jeff budget // q3 rev`.
- **Column targeting** like Helix's `%column text` falls out of the same parser: `subject:` and
  `from:` are columns.
- `OR` and parentheses can come later. Gmail supports them, but most users never type them.
- **The same parser serves the CLI** (`neomail search 'from:jeff is:unread budget' --json`) and
  agents, so pickers, CLI and MCP share one query language. That satisfies "agents and the CLI
  can query the same sources".
- **Prior art:** Gmail operators (`from: to: cc: subject: after: before: older_than:
  newer_than: label: has:attachment filename: is: in: larger:`, `-`, `OR`, `"…"`, `( )`),
  notmuch/Xapian prefixes (`from: to: subject: tag: date:..`), aerc and himalaya filters.

## 7. Recommended design for neomail

### Crates
- **`nucleo` 0.5.0 + `nucleo-matcher` 0.3.1 (MPL-2.0)** for v1, behind a `FuzzyEngine` seam:
  - it already has the streaming engine, incremental narrowing and cancellation;
  - it supports Unicode normalisation, which matters for names like *Müller* and *José*;
  - Helix uses it in production.

  The risk is that it has had no release since 2024-04. It is stable but unmaintained, so be
  ready to fork it as a separate MPL crate.
- **`frizbee` 0.13 (MIT)** is the upgrade path. It is 1.4 to 7.5 times faster per thread, has
  typo tolerance and is MIT. But it lacks normalisation and has no engine, so neomail would
  have to write the ~1,600-line engine television wrote. Revisit if profiling ever shows
  matching as the bottleneck. On these numbers it is not.
- **`rusqlite` with `bundled`** (FTS5 compiled in): a `unicode61 remove_diacritics 2`
  external-content index over `subject, from, to, body_text, attachment_names`. An optional
  `trigram detail=none` index over `body_text`, as a setting.
- No `grep-searcher` in v1. Draft-scope search runs over the in-memory rope (`regex` or nucleo
  over lines). grep-searcher only pays off for searching files on disk.
- Not in v1: tantivy (see section 4).

### Threading
```text
UI thread (ratatui loop, owns PickerState)
  ├─ on key: parse query → if FTS/scope part changed: generation += 1, nucleo.restart(true),
  │                         spawn producer(gen) ; else pattern.reparse(append?)
  ├─ on each frame: nucleo.tick(2)  → render snapshot.matched_items(visible window)
  │                 compute match indices only for visible rows (Pattern::indices)
  └─ notify callback = wake the event loop (same channel as other redraw requests)
nucleo rayon pool: available_parallelism − 1 threads (leave a core for UI + sync)
producers: tokio spawn_blocking, each with a read-only SQLite connection (WAL, pooled ×2–4)
           page loop: WHERE rowid < :cursor … ORDER BY rowid DESC LIMIT n, n = 100, 400, 1600 …
           stop when injector.push() errors (generation moved) or limit_live (10,000) reached
           cancel in-flight SQL with rusqlite InterruptHandle::interrupt()
previewer: debounced 60 ms on selection change; LRU(64) of rendered preview *buffers*;
           decorate (quote/link/address highlights) on a background task, swap in when done
```

### Latency budget (60 Hz, 16.7 ms frame)

| Step | Budget | Evidence |
|---|---|---|
| Echo the keystroke into the prompt | same frame | UI thread does no I/O |
| Parse the query and diff it against the previous one | < 0.2 ms | |
| `tick()` wait | ≤ 2–4 ms (Helix waits 10 ms; neomail renders more per frame) | the first tick returned within 10.1 ms for 517k items |
| In-memory picker, 500k items, append keystroke | 1–9 ms | measured |
| In-memory picker, full re-score | 15–22 ms on 16 threads, so it can spill over one frame and the last snapshot stays visible | measured |
| FTS first page (newest 100) | < 5 ms (0.1–1.2 ms typical; up to 25 ms for 2-character prefixes) | measured |
| Preview of a cached message | same frame (plain text), decorations by the next frame | Helix pattern |

**Debounce:** none for in-memory pickers. For live FTS, run immediately and cancel the run it
supersedes. Debounce 50 ms only if the previous run is still in progress (Helix uses 275 ms
because grep walks the disk; SQLite FTS does not).

### The `PickerSource` provider (object-safe, so Lua sources and the CLI can use it)

```rust
pub trait PickerSource: Send + Sync + 'static {
    fn id(&self) -> &str;                          // "mail.search", "senders", "actions"
    fn columns(&self) -> &[ColumnSpec];            // name, matchable, primary, width hint
    fn scopes(&self) -> &[ScopeKind];              // supported scopes, in cycle order
    fn mode(&self) -> SourceMode;                  // Static: items once, fuzzy-only
                                                   // Live: re-run when filters/FTS text change
    /// Blocking; runs on the producer pool. Push until done or `sink` says cancelled.
    fn produce(&self, req: &SourceRequest, sink: &mut dyn ItemSink) -> Result<(), SourceError>;
    /// Row rendering through the shared decoration primitives (highlights, virtual text, signs).
    fn render(&self, item: &PickerItem, cx: &mut RowCx<'_>);
    fn preview(&self, item: &PickerItem) -> Option<PreviewKey> { None }
    fn default_action(&self) -> ActionId;          // from the action registry
    fn actions(&self) -> &[ActionId] { &[] }
}
pub struct SourceRequest { pub query: ParsedQuery /* filters + fts text */, pub scope: Scope,
                           pub generation: u64, pub limit: usize }
pub struct PickerItem { pub key: ItemKey /* MessageId(i64) | Address(Arc<str>) | Action(ActionId)
                        | Line(u32) | Lua(RegistryKey) */, pub cols: SmallVec<[Utf32String; 3]>,
                        pub rank_hint: f32 /* frecency etc., used for ties + top-window re-rank */ }
pub trait ItemSink { fn push(&mut self, item: PickerItem) -> Result<(), Cancelled>; }
```

- **Items are small keys plus their match text.** Everything else is loaded lazily for the
  visible rows and the preview. By estimate, that keeps 1M items near 100 MB and makes Lua sources
  trivial: a Lua function returns `{key=…, text={…}}` tables.
- **One picker implementation serves every source.** The picker is a buffer whose window
  layout is a float, and it fires events (`PickerOpen`, `PickerSelect`, `PickerClose`).
  Selections feed frecency through the action registry.
- **CLI and agents:**
  `neomail pick <source> --query '…' --scope account:work --limit 50 --json` calls `produce`, runs
  a one-shot `Pattern::match_list` and prints JSON. The ranking is the same as the TUI's.

## Key facts

- **[high]** nucleo-matcher implements Smith-Waterman with affine gaps using fzf's scoring constants. It keeps one `n−m+1`-wide row while scoring, prefilters with memchr, has an ASCII byte path, and falls back to a greedy matcher for long matches. — <https://github.com/helix-editor/nucleo/blob/master/README.md>
- **[high]** The `Nucleo` engine runs on a rayon pool. `Injector::push` is wait-free and calls `notify`. `tick(timeout)` waits at most `timeout` ms for the worker lock and returns `Status{changed, running}`. The docs recommend 10 ms. — <https://github.com/helix-editor/nucleo/blob/master/src/lib.rs>
- **[high]** When the pattern changes, `reparse(…, append=true)` only re-scores existing matches, in parallel with a cancel flag. A non-append change re-scores every item. — <https://github.com/helix-editor/nucleo/blob/master/src/worker.rs>
- **[high]** nucleo 0.5.0 (2024-04-02) and nucleo-matcher 0.3.1 (2024-02-20) are the latest releases. Both are MPL-2.0, and Helix master still uses nucleo 0.5.0. — <https://crates.io/crates/nucleo>
- **[high]** Helix's picker cancels producers with a version counter (`InjectorShutdown`), debounces dynamic queries (global search 275 ms, default 100 ms), calls `tick(10)` each render, caches previews per path (with a 10 MiB limit and a 1 KiB binary sniff), and highlights previews on a 150 ms-debounced background handler. — <https://github.com/helix-editor/helix/blob/master/helix-term/src/ui/picker.rs>
- **[high]** Helix global search runs grep-searcher (`BinaryDetection::quit(0)`) inside `ignore`'s parallel walker. The sinks push into the picker injector and return `WalkState::Quit` once it is shut down. Open buffers are searched from memory. — <https://github.com/helix-editor/helix/blob/master/helix-term/src/commands.rs>
- **[high]** frizbee 0.13.0 (MIT, 2026-08-13) is a SIMD Smith-Waterman matcher with typo tolerance. It does no Unicode normalisation and does not match `a` against `á`. Its benchmark on 1.41M Chromium paths: nucleo 90.5 ms, fzf 120.8 ms, frizbee 22.4 ms single-threaded. — <https://github.com/saghen/frizbee/blob/main/BENCHMARKS.md>
- **[high]** television (MIT) depends on frizbee 0.13; skim 5.7 (MIT) only offers it as an optional `--algo frizbee` (default algorithm: `arinae`). television wrote its own chunked, progressively published engine around it, with frecency promotion. — <https://github.com/alexpasmantier/television/tree/main/television/matcher>
- **[medium]** (not independently reproduced) Measured here on 517k sender+subject strings: nucleo-matcher took 11–257 ms single-threaded and frizbee 4.7–78 ms single-threaded or 1.8–10 ms on 16 threads. The nucleo engine took 15–22 ms for a full re-score and 0.7–9 ms for incremental narrowing. — local benchmark, see section 2
- **[high]** MPL-2.0 is file-level copyleft. Static linking into a larger work under another licence is allowed. Binary distributors must tell recipients where to get the source of the MPL files. Unmodified use carries no obligations. — <https://www.mozilla.org/en-US/MPL/2.0/FAQ/>
- **[high]** cargo-deny removed `copyleft`, so every licence must be allowed explicitly. `[[licenses.exceptions]]` takes `crate`/`name`, `version` and `allow`. — <https://embarkstudios.github.io/cargo-deny/checks/licenses/cfg.html>
- **[medium]** (not independently reproduced) Measured here, FTS5 on 517k Enron messages (951 MB of text):
  - unicode61 index: 514 MB, built in 44 s;
  - trigram, detail=full: 3,036 MB, built in 306 s;
  - trigram, detail=none: 365 MB;
  - bm25 top-50: 8.5–311 ms;
  - newest-50 by rowid: ≤ 0.1 ms;
  - ordering by a joined date column: 11–244 ms.

  — local benchmark, see section 4
- **[high]** The FTS5 trigram tokenizer cannot match substrings shorter than 3 characters. With detail=none or detail=column, MATCH tokens can be at most 3 characters, while LIKE/GLOB still work. — <https://www.sqlite.org/fts5.html>
- **[medium]** Another 2M-row benchmark found trigram 3.9× the size of the list and 9× slower to build than a word index. Trigram was faster for rare terms and substrings, and slower for common and multi-word queries. — <https://github.com/Ninja-FSE/dccore-dev/issues/1127>
- **[medium]** (benchmark not independently reproduced) Measured here, tantivy 0.26.2 on the same corpus: 336 MB, built in 9.8 s, BM25 top-50 plus count in 0.1–2.7 ms. Its normal dependency tree has 125 crates besides tantivy across all targets (108 on x86_64 Linux), against 16 for nucleo (re-counted with `cargo tree -e normal`). — local benchmark
- **[high]** Firefox's current frecency is the mean of `w·e^(−λΔt)` over the last 10 sampled visits times the visit count, with a 30-day half-life (`places.frecency.pages.halfLifeDays = 30`), stored as `t_ref + ln(score)/λ` (the day the score decays to 1). It replaced a bucketed algorithm that needed daily decay passes. — <https://firefox-source-docs.mozilla.org/browser/urlbar/ranking.html>
- **[high]** zoxide adds 1 per access, multiplies by 4 / 2 / ½ / ¼ depending on the last access (hour, day, week, older), and ages every entry by scaling to 90% of `_ZO_MAXAGE`. — <https://github.com/ajeetdsouza/zoxide/wiki/Algorithm>
- **[high]** telescope-frecency v2 and `fre` store a decayed score relative to a reference time, with a 3-day default half-life. — <https://github.com/nvim-telescope/telescope-frecency.nvim/blob/master/lua/frecency/v2/table.lua>
- **[high]** smart-open.nvim ranks by a weighted sum of normalised signals and learns the weights from the user's picks (`ADJUSTMENT_POINTS = 0.6`). — <https://github.com/danielfalk/smart-open.nvim/blob/main/lua/telescope/_extensions/smart_open/weights.lua>
- **[low]** Firefox's legacy frecency recency buckets were 4/14/31/90 days with weights 100/70/50/30 and 10 for older visits, with up to 10 visits sampled. These are from memory of the old `places.frecency.*` prefs; neither the cited page nor current Firefox source lists them any more. — <https://firefox-source-docs.mozilla.org/browser/urlbar/ranking.html>
- **[high]** snacks.nvim separates `search` (passed to the finder; live sources re-run on change) from `pattern` (filtered by the matcher). It has `toggle_live` (`<c-g>`), `limit_live = 10000` and matcher `frecency`/`history_bonus` options. — <https://github.com/folke/snacks.nvim/blob/main/docs/picker.md>
- **[high]** Gmail search operators include `from: to: cc: bcc: subject: after: before: older_than: newer_than: label: category: has: list: filename: in: is: larger: smaller:`, plus `OR`, `-`, quotes and parentheses. — <https://support.google.com/mail/answer/7190>

## Recommendations

1. **One generic picker on top of `nucleo`**, used by every picker (built-in, Lua and CLI)
   through the object-safe `PickerSource` provider in section 7. Wrap nucleo behind a
   `FuzzyEngine` seam so that frizbee, or a fork, can replace it without touching sources.
2. **Two layers for mail search:**
   - an FTS5 candidate producer (filters and words, newest-first, paged and cancellable);
   - an in-memory fuzzy layer for short fields and an optional second pattern.

   Never fuzzy-match bodies.
3. **Make the message rowid time-ordered.** This one storage decision turns "newest matching
   mail" into an O(limit) query (≤ 0.1 ms at 500k messages, against up to 244 ms when sorting
   by date). Put it in the architecture's storage chapter.
4. **FTS5 setup:**
   - `unicode61 remove_diacritics 2`, external-content, detail=full, over the plain-text
     rendering of each body;
   - no prefix index; prefix-expand the last token only once it has 3 or more characters;
   - bm25 only as an explicit "relevance" sort;
   - an optional `trigram detail=none` substring/CJK index, behind a setting.
5. **Leave tantivy out of v1.** Keep `SearchIndex` a provider so it can be added if ranked
   search over more than 1M messages becomes a requirement.
6. **Sender frecency:**
   - Firefox-style exponential decay, stored as one indexed "date the score falls to 1"
     column;
   - 30-day half-life;
   - event weights come from actions (send/reply 4, pick 3, star 2, open 1);
   - seed it from the Sent folder;
   - combine it with the fuzzy score by re-ranking the visible top window.
7. **Scope is picker state** (draft → thread → folder/group → account → all) and becomes
   structured filters. The query language is a Gmail-compatible subset shared by the TUI, the
   CLI and MCP.
8. **Budgets and cancellation:**
   - `tick(2–4 ms)` per frame, with notify-driven redraws;
   - no debounce for in-memory pickers;
   - cancel-on-supersede, with `InterruptHandle`, for SQL;
   - the generation counter on injectors from Helix;
   - preview debounce of about 60 ms, with an LRU of rendered preview buffers decorated
     asynchronously.
9. **Licences:** allow MPL-2.0 per crate in `deny.toml` (nucleo, nucleo-matcher). Ship a
   generated third-party notices file (cargo-about) with each release, naming the nucleo source
   URL. Never copy nucleo code into neomail's own files.

## Open questions

- Should neomail adopt frizbee's typo tolerance for the senders picker (names are often
  misspelt) despite its lack of normalisation? A hybrid is possible: nucleo for everything and
  frizbee only for the address-completion source. Decide after real-mailbox testing.
- How many cores can the fuzzy pool use when the daemon is syncing and classifying at the same
  time? Measure on a 4-core laptop. These benchmarks used 16 threads.
- Should the time-ordered rowid use the provider's internal or received date or the `Date:`
  header? Received date resists spoofed or missing `Date:` headers. Gmail and Graph expose
  internal dates.
- Should the optional trigram index switch on automatically when CJK text is detected in the
  mailbox?
- Is "relevance" (bm25) ranking needed at all in v1? The default should be newest-first.
- Is the fuzzy-stage separator in the query `//`, a key toggle, or both?
- Should frecency be per account or global? The proposal is global per address with an
  `account_id` column for filtering.

## Fact-check (independent)

Checked 2026-10-10 against crates.io's API, cloned sources (nucleo `8c16d47`, Helix `ba40e54`,
frizbee `5fa0fa4`, television `main`, smart-open.nvim `main`) and the official pages cited.

**Confirmed as written**
- crates.io versions, dates and licences: nucleo 0.5.0 (2024-04-02, MPL-2.0), nucleo-matcher
  0.3.1 (2024-02-20, MPL-2.0), frizbee 0.13.0 (2026-08-13, MIT), fuzzy-matcher 0.3.7
  (2020-10-04, MIT), tantivy 0.26.2 (MIT), skim 5.7.x (MIT), option-ext 0.2.0 (MPL-2.0),
  smartstring 1.0.1 (`MPL-2.0+`), colored 3.1.1 (MPL-2.0), webpki-roots / webpki-root-certs
  1.0.9 (CDLA-Permissive-2.0). Helix's workspace pins `nucleo = "0.5.0"`.
- nucleo-matcher scoring constants (16/3/1, boundary 8, camel/digit 5, consecutive 4, first-char
  ×2), memchr/memchr2 prefilters, greedy fallback, `n−m+1` row, grapheme handling, the
  `xf foo` example and the README benchmark ratios (≈3.7–7.9× vs skim; `///.` on 3M items,
  Ryzen 5950X). The "sorting is negligible" remark is in an HTML comment in the README.
- nucleo engine: `Nucleo::new(config, notify, num_threads, columns)`, `Injector::push(value,
  fill_columns)` calling `notify`, `tick(timeout)` (docs recommend 10 ms), `Status { changed,
  running }`, `restart`, `sort_results`, `reparse(col, text, case, norm, append)`,
  `take_any_while(!canceled)`, `idx = u32::MAX`, score-then-index sort, MIT `boxcar` origin.
- Helix: `tick(10)`, `InjectorShutdown`/`picker_version`, `RequestRedrawOnDrop`, dynamic-query
  debounce default 100 ms and 275 ms for global search, 150 ms preview-highlight debounce,
  10 MiB preview limit, 1 KiB binary sniff into `read_buffer`, `CachedPreview` variants,
  `BinaryDetection::quit(b'\x00')`, `build_parallel`, `sinks::UTF8`, `WalkState::Quit`, `%field`.
- frizbee: SIMD widths, u8/u16 scoring, Smart Unicode path, no normalisation (`a` ≠ `á`),
  `max_typos`, `Matcher::new` / `from_query` / `match_list_parallel`; Chromium figures
  (1,406,941 paths, 90.53 / 120.84 / 22.36 ms sequential; 36.78 / 16.36 / 3.48 ms on 8 threads;
  1 typo 60.76 / 9.50 ms).
- television's matcher is 1,623 lines, append-only, with chunks starting at 512 Ki items and
  doubling up to 8 Mi, plus a promote table.
- MPL FAQ Q5, Q8, Q10, Q11 (Q11 explicitly allows static linking into a larger work);
  cargo-deny `copyleft` removal and `[[licenses.exceptions]]` with `crate`/`name`.
- SQLite trigram notes (fewer than 3 characters never match; detail=none/column limits MATCH
  tokens to 3 characters while LIKE/GLOB still work). zoxide, telescope-frecency v1 buckets and
  v2 3-day half-life, fre's 3-day default, smart-open weights and `ADJUSTMENT_POINTS = 0.6`
  (sigmoid normalisation confirmed in its fzf/fzy scorers), snacks `limit_live = 10000`,
  `<c-g>` `toggle_live`, `frecency`/`history_bonus`. Gmail operators including `bcc:`,
  `category:`, `list:`, `larger:`/`smaller:`, `OR`, `AROUND`. The Ninja-FSE issue numbers
  (3.9× size, ≈9× build time) match the paraphrase.

**Corrected in place**
- frizbee's "Korean" figures were actually its Arabic row. Korean is nucleo 104.0 ms, fzf
  114.5 ms, frizbee 2.5 ms.
- frizbee's "8 threads" nucleo figure uses nucleo's own engine (8-thread `Nucleo`, full
  re-score), not rayon over `match_list`.
- skim did not move to frizbee: its default algorithm is `arinae`; frizbee is an optional
  `--algo`. fff.nvim uses the `neo_frizbee` fork.
- fuzzy-matcher is skim's legacy `skim_v2` scorer, not a "v1" scorer. frizbee also has a prefix
  bonus.
- Firefox's current score is an average over the last 10 sampled visits times the visit count,
  not a plain sum.
- tantivy has 125 other crates across all targets (108 on x86_64 Linux); nucleo has 16 besides
  itself.
- Added: the trigram index must not use `remove_diacritics` if LIKE/GLOB indexing is wanted.

**Still uncertain**
- All "measured here" numbers (scripts not in the repo; not reproduced). They are lowered to
  medium.
- Firefox's legacy bucket values (lowered to low): no current primary source lists them.
- Whether cargo-deny's `MPL-2.0` allow entry also covers `MPL-2.0+`.
- That FTS5 falls back to a full scan for LIKE patterns under 3 characters: the docs wording is
  vaguer, though the Ninja-FSE benchmark observed the fallback.
