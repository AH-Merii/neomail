# Keeping the design consistent

Decided 2026-10-11 by the consistency lead on the owner's instruction; binding for every workflow,
agent and CI job that writes `docs/` or code. Builds on retro rules R1–R7. Evidence, all in this
directory: `forensics.md` / `forensics.json` (211 contradiction groups, 344 restatements),
`prior-art.md` (P1–P24), `redteam.md` (A1–A10, B1–B11, C1–C4). Revision 2 answers skeptic round 2
(K1–K32, §12). "§" means this file's sections.

## 1. What went wrong, measured

256 raw reports collapse to 211 contradiction groups (`forensics.md:15-19`). Each group has one
root cause and the cheapest check that would have failed the merge that brought it in
(`forensics.md:69-91`; one analyst, so each cell is ±a few).

| Failure class | Groups | Share | L / T / G / C / S / A | Example (source) | Closed by |
|---|---|---|---|---|---|
| dependency / boundary violation | 37 | 17.5 % | 31 / · / · / 4 / 2 / · | C065: `mcp.redaction` in client crate nm-mcp, after text left the daemon (`plan.md:471` vs `01-process-and-crates.md:183`) | L0 aliases + boundaries, L1, L5 |
| ownership ambiguity | 34 | 16.1 % | 24 / · / · / · / 10 / · | C152: remote images owned by nm-net and nm-attach (`plan.md:391` vs `:690`) | L0 owners, L1 |
| semantic rule conflict | 29 | 13.7 % | 1 / · / · / 4 / 23 / 1 | C134: three meanings of `<Tab>` in one prompt (`10-search-and-pickers.md:116` vs `09-composer-and-spelling.md:193`) | L0 keymap data, L2 |
| restated fact drift | 28 | 13.3 % | 15 / · / 9 / 1 / 3 / · | C071: Lua budget 20 ms vs 50 ms (`12-extensions-agents-mcp.md:13` vs `01-process-and-crates.md:83`) | L0 inline constants, L1 |
| late decision not propagated | 24 | 11.4 % | 9 / 13 / · / · / 2 / · | C024: `ask_above = 5000` after the 1,000 cap (`13-config.md:116` vs `decisions.md:76-79`) | W0, L1, W1 |
| naming drift | 22 | 10.4 % | 17 / 1 / 2 / · / 2 / · | C063: `privacy.redaction` vs `classify.redaction` (`06-classification.md:123` vs `13-config.md:117`) | L1 |
| missing definition | 18 | 8.5 % | 14 / · / 1 / 1 / 2 / · | C017: `events.include_subjects` absent from the config owner (`07-actions-api-cli.md:185`) | L1 |
| half-applied revision | 8 | 3.8 % | 2 / · / 2 / · / 4 / · | C190: blobs.db under nm-store vs nm-blobs (`03-domain-and-storage.md:173` vs `README.md:82`) | §8 staging, L2 |
| other (facts outside the docs) | 6 | 2.8 % | · / · / · / 1 / 3 / 2 | C171: "46 GB box" (`18-risks-and-cut-line.md:13`); `free -g` prints 15 total | W2 probes |
| dangling reference | 5 | 2.4 % | 4 / · / · / · / 1 / · | C051: "08 §9" is Learnability, not the governor (`02-building-blocks.md:581`) | L1 anchors |

L manifest + lint, T decision-traceability lint, G generated-table drift, C compile probe,
S targeted semantic review, A routine full audit. Cumulative: L 55.5 % → T 62.1 % → G 68.7 % →
C 73.9 % → S 98.6 % → A 100 % (`forensics.md:83-91`). What this forces:

- **S must run before merge, on neighbours, for every docs PR.** Of the 52 S groups, 42 have their
  sides in different files and only 2 come from a decision (query over `forensics.json`: `S 52
  cross-file 42 LD 2`). C134's two sides share no link: `grep -n "09-composer\|02 §5.6\|09\b"
  docs/architecture/10-search-and-pickers.md` prints nothing. Decision- or link-triggered review
  cannot see them (K1).
- **Owner rulings bypass the repo.** Two binding rulings exist only in a gitignored script
  (`arch5-rederive.js:36-37`; `git check-ignore` → `.gitignore:2:/target`), D1–D5 only in a brief
  (`arch5-inventory.js:35-39`), and the first-run ruling is still uncommitted (`git diff
  docs/decisions.md`: 4 added lines) (K16).
- **Drift has a different value.** C071 (20 vs 50 ms), C167 (35 MiB vs 25 MB) and C168 (25 vs
  150 MB) are copies whose value moved; an equality check is silent on all three (K2, K19).
- **BV is written with aliases**, not crate names: "`standard` applies the classifier's redaction"
  (`12-extensions-agents-mcp.md:190`); 1 of 37 BV lines names two `nm-*` crates with a verb (K3).
- **Decision intake is the largest single failure:** one ruling gave 17 groups and 33 raw reports,
  and its stale sites hold the *old* value (`forensics.md:222-229`, `redteam.md:57-75`).
- **Concurrent docs writers share one working tree.** `but-commit-paths` picks
  `.uncommittedChanges[]` by path prefix with no author check
  (`~/.local/share/neomail-dev/bin/but-commit-paths:13-18`); the motion lead edits 02, 08 and 13
  (`motion/integration.md:49,77,120`) while arch5 re-derives them (K18).
- **Latent copies are the next wave:** 280 restatements agree today; 199 carry a key, name or
  number, 81 are reworded rules (`forensics.md:240-243`).
- **LLM reviewers miss more than they invent:** recall 0.296, or 0.566 step by step, at precision
  ≥ 0.90 (arXiv 2504.00180, `prior-art.md:395-397`). Recall is measured with canaries.

## 2. The decisions in one screen

1. **Record first, one channel.** A ruling exists only as a committed `DEC-` row in
   `docs/decisions.md`. Workflow scripts carry IDs and load the text at run time (§6).
2. **Facts are data; prose is rationale.** Every closed-set fact (crates, edges, ports, keys,
   constants, actions, events, profiles, capabilities, stores, boundaries, decisions) is a row in
   `manifest.d/` or a rule table; one declared source per fact kind (§4 `[[source]]`); tables,
   inline values and code constants are generated from that source.
3. **One owner per fact, and definitions have a shape.** A definition-shaped construct outside the
   owner anchor fails; a number in a sentence that names a constant must be the inline value.
   Retired names, keys and values are tombstoned and fail anywhere.
4. **Every docs PR gets a neighbour review before merge** (W4): the impact set is computed from
   the diff and the fact index; both sides are reviewed until dry.
5. **Change classes by path.** Class A (manifest, rule tables, port contracts, concern-owner
   anchors) lands only in a steward window during build waves; Class B (other prose, links,
   generated output) lands any time. Audits and decision runs prepare PRs during waves; the
   steward only merges them.
6. **One docs writer at a time.** Docs-mutating runs stage under `target/<run>/staging`, take
   the docs lock to promote, and promote in one commit (§8).
7. **archlint has errors only.** A rule that cannot be an error is removed and its class goes to a
   review lens. The gates cannot be weakened by the agents they gate.

## 3. Layered defence

| Layer | Checks | When | Run by | Catches (forensics share) |
|---|---|---|---|---|
| **L0 Prevent by construction** | manifest rows, rule tables as data, generated tables, `<!--c:id-->` values, generated "Depends on / used by" lines, anchors, DEC rows before use, contract-first fan-out (W3) | while writing | writers; the contract owner fixes the contract first | 199 of 280 latent copies become lint-visible (71.1 %) |
| **L1 Deterministic gates** | archlint AL-01…AL-36, `gen --check`, ratchet baseline, protected paths; from WP-00 also `xtask check-dag`, `cargo deny check bans licenses`, doc trait snippets compiled | `wp-publish` before the PR; CI on every PR | CI jobs `archlint`, `gen`, `dag`, `deny` | L + T + G = 145 / 211 (68.7 %); + C from WP-00: 156 (73.9 %) |
| **L2 Pre-merge neighbour review** | W4 docs stage: diff + fact-index neighbours, lens review both sides, refute-by-default verifier | every PR that changes `docs/` | W4 reviewer agents; read-only | S = 52 (24.6 %), cumulative 98.6 % |
| **L3 Decision change** | W1: impact set from `affects`, tombstones, a one-time old-value grep with triage, neighbours | every ruling (after W0) | W1 orchestrator | LD and the latent copies a ruling touches |
| **L4 Routine audit** | W2: incremental during each build wave, full at milestones, canaries, machine probes | waves; milestones | W2, read-only; output is PRs | A = 3 (1.4 %); measures recall |
| **L5 Code ↔ design** | generated Rust from the manifest, generated docs from code, `check-dag`, ICRs, `Concern-Revs` | every WP PR; steward | CI; WP reviewer; steward | C = 11 (5.2 %) today; later code drift |

## 4. The manifest and the rule tables

`docs/architecture/manifest.d/<area>.toml`: one fragment per architecture file plus
`decisions.toml`. archlint merges them; IDs are unique across fragments (`redteam.md:434-444`).
Every row has `id`, `owner` (`file.md#anchor`) and `status` (`proposed` | `live` | `retired`).

| Table | Fields beyond id / owner / status | Generates |
|---|---|---|
| `[[source]]` | fact kind → `manifest` or `code` (path of the exporter) | which side `gen` writes |
| `[[crate]]` | `kind` (root, interface, impl, test), `path`, `process` (daemon, tui, client, ext-host, build), `wp`, `deps` (`{crate, features}`), `third_party`, `unsafe`, `licence` | crate map, DAG, "Depends on / used by" lines, `check-dag` edges, `deny.toml` |
| `[[thirdparty]]` | `name`, `exclusive_wrapper` (crate or none), `licence` | `deny.toml` bans and licence list |
| `[[wp]]` | `wave`, `owned_paths` | wave table (plan §1) |
| `[[service]]` (R1 capability) | `owner_crate`, `consumers`, `alternative` | R1 check: ≥ 2 consumers or an alternative ⇒ a port |
| `[[port]]` | `home`, `contract` anchor, `suite`, `impls`, `consumers` | port lists (README, 02, 07), conformance list (16) |
| `[[concern]]` | `rev`, `kind` (fact, policy, transform), `boundary` (transform only), `key` | concern index |
| `[[key]]` | `type`, `default`, `security`, `env`, `reader` | full example in 13, key tables, Rust defaults |
| `[[constant]]` | `value`, `unit`, `aliases` (phrases), `concern`, `security` | `<!--c:id-->` values, Rust constants |
| `[[name]]` | `kind` (type, preset, completion-source, picker-source, replaceable-fn, process-mode, config-file, help-topic, milestone, id-prefix), `home` crate, `fields` (types) | glossary |
| `[[alias]]` | phrase ("MCP", "the classifier", "parse worker", "Scheduler"…) → crate | AL-13 resolution |
| `[[store]]` | `file` or `table`, `writer` crate | single-writer table (03) |
| `[[boundary]]` | `from`, `to` (process), `payload` type, `converter` crate | boundary table (14) |
| `[[env]]` | `NMAIL_*` name, `reader`, `security` | env table (13 §1) |
| `[[retired]]` | token, kind (key, name, value with unit, phrase), `replaced_by`, `decision` | tombstone list |
| `[[decision]]` | §6 | decision table (18 §4), plan §10 (PROV rows) |
| `[[generated]]`, `[[probe]]`, `[[data]]` | block + generator; command + expect; path + schema + `wp` | presence check; machine facts; rule-table registry |
| `max_depth` (top-level) | longest crate path (5, `01-process-and-crates.md:203`) | — |

**Rule tables as data** (`redteam.md:143-174`) in `docs/architecture/data/`: `profiles.toml`
(profile → capabilities → approval), `catalogue.toml` (action → `args`, capability, confirm policy,
CLI verb, binder crate; plus `[[event]]`, `[[method]]` and `[[mcp_tool]]` rows, each mapped to an
action), `keymap.default.toml` (mode, context, key → action). The 07 matrix and CLI tree, the 08 key
tables and the 12 profile table are generated from them. At WP-00 each moves, in one commit, to
the path its loader reads; `[[source]]` then says `code` for actions and events, and `xtask` exports
JSON to `docs/generated/` that archlint reads instead (K24; `plan.md:122-123`).

**Closed catalogues only** (K22): keys, env vars, actions, events, methods, MCP tools, profiles,
capabilities, exit codes, crates and ID prefixes must resolve. Types are registered only when used in
two or more files. Fan-out writers may write `TBD:Name`; archlint allows it only under
`target/*/staging`, and promotion fails while any is left.

**Markup in prose.** Owner units carry `<a id="slug"></a>`; IDs never encode numbers. Links are
`file.md#slug`. Inline values: `<!--c:classify.first_run.cap-->1,000<!--/c-->`. Generated blocks
sit between `<!-- gen:begin ID -->` and `<!-- gen:end -->`. Every ```` ```toml ```` fence carries a
schema: `toml config`, `toml keymap`, `toml cargo`, `toml profiles`, `toml catalogue`, or
`toml example-override` (K10). Decisions are cited as links to `../decisions.md#dec-…`.

**Fact index** `docs/architecture/facts.jsonl`: per section, its content hash and the extracted
concerns, keys, names, actions, keybindings and numbers with role (defines, restates, references),
in the arch5 extract schema. Each restate/reference fact is also an implicit link in `links.lock`
(K8). W4 re-extracts changed sections in the same PR; AL-36 fails a stale hash.

## 5. archlint rules

`tools/archlint` (stdlib Python, R3). Errors only: there is no warning severity (K31). Findings are
fingerprinted by rule + entity ID or anchor pair, never by line; a finding fails CI unless its
fingerprint is in the baseline, which only shrinks. "Outside its owner" always means outside the
entity's owner anchor, generated blocks and `<!--c-->` values.

| ID | Fails when | Class |
|---|---|---|
| AL-01 | a fragment does not parse, a row misses a required field, an ID repeats, or an ID matches the pattern of another prefix (`DEC-`, `PROV-`, `PAT-`, `ICR-`, `WP-` are disjoint) | all |
| AL-02 | an entity (concern, service, port, store, key, constant, action namespace, rule table) has ≠ 1 owner anchor; a **definition shape** for it appears outside its owner: a `toml config` key line, a table row whose first cell is the entity, or an "X is / means / defaults to" sentence about it; a concern has ≠ 1 `key` (K5) | OA, RF |
| AL-03 | a key, constant, port or concern has no consumer (citing section, crate or test) | MD |
| AL-04 | a port lacks home, contract anchor, suite or implementing WP; a service with ≥ 2 consumers or `alternative = true` has no port (R1) | OA, BV |
| AL-05 | a crate is owned by ≠ 1 WP, or a WP's owned path is not in `[[crate]]` / `[[data]]` | OA |
| AL-06 | a token in a closed catalogue does not resolve: every backticked dotted lowercase token against keys ∪ actions ∪ events ∪ methods ∪ MCP tools ∪ profiles ∪ capabilities (unknown first segments fail; tokens with a file extension are paths); `nm-*`; `NMAIL_*`; decision, WP and ICR IDs; `<prefix>_…` IDs against `id-prefix` names; `Type.field` against `fields`; `nmail …` spans against the generated CLI tree (K9, K11, K12) | ND, MD |
| AL-07 | a backticked CamelCase name absent from the glossary shares its last two words with a glossary name | ND |
| AL-08 | a `[[retired]]` token appears outside `docs/decisions.md`; TOML table context is resolved (`ask_above` under `[classify]` is `classify.ask_above`), and so is a unit-less number on a line whose key is retired (K6) | LD, ND |
| AL-09 | a ```` ```toml ```` fence has no schema info string, or fails its schema; a `toml config` block has an unknown key or a non-default value without `example-override` (K10) | MD, RF |
| AL-10 | a number with a unit sits in a sentence or table row that names a constant's ID, key or alias, outside `<!--c-->`, **whatever its value**; a line naming a registered key with `= <literal>` or "default <literal>" differs from the manifest (K2, K19) | RF |
| AL-11 | a `<!--c:id-->` names an unknown constant or key, or its value differs | RF |
| AL-12 | a link, anchor or help topic does not resolve, or a bare `NN §N` / `§N` appears outside link text | DR |
| AL-13 | a sentence, table row or diagram tree names two components (crate or `[[alias]]`) joined by a dependency verb (uses, calls, imports, depends on, reads, writes, applies, owns, generates from, reuses, hands to, via) or a possessive ("nm-fx's X", "the classifier's redaction"), and no `deps` edge joins them that way; a type is attributed to a crate other than its `home` (K3) | BV |
| AL-14 | a `client` or `tui` crate converts a boundary leaving the daemon; a converter sits outside the boundary's `from` process; a `transform` concern bound to a boundary has a key whose `reader` is not that boundary's `converter` (C065) | BV |
| AL-15 | a boundary's owner section does not name its `payload`, or a section describing `from → to` names another type | BV |
| AL-16 | a third-party crate or `unsafe` is attributed to a crate whose lists lack it; a crate other than the `exclusive_wrapper` lists that third-party crate (C113) | BV |
| AL-17 | a crate other than the store's `writer` is said to write it | OA |
| AL-18 | rule tables: a profile used is undefined; a capability is absent from the catalogue; a CLI verb maps to ≠ 1 action; an action lacks `args`, capability or confirm policy; an event, method or MCP tool maps to no action; a key is bound twice in one (mode, context); a binding names an unknown action | SC, ND |
| AL-19 | a `[[generated]]` block is missing, its markers are gone, or a hand table repeats a generated header row | HR, RF |
| AL-20 | a decision row breaks §6: status, both-way supersession, `retires` on a superseding row; a `creates` entry does not resolve to a live row once the decision is live; a `sets` entry (`key = value`) differs from the manifest (K6, K21) | LD |
| AL-21 | a section cites a superseded or rejected ID (history in `docs/decisions.md` excepted) | LD |
| AL-22 | an owner section of an entity in a live decision's `affects` does not cite that decision | LD |
| AL-23 | a bare `D\d+` or `P\d+[a-z]?` token appears (legacy IDs, after migration) | LD, ND |
| AL-24 | a citation's or implicit link's fingerprint in `links.lock` differs from the owner unit's normalised text (suspect link) | LD, HR |
| AL-25 | a branch not named `steward/*` or `decision/*` edits `tools/archlint/**`, `xtask/**`, `.github/workflows/**`, the baseline (except removals) or `suppressions.toml`; anyone re-blesses a suspect link without a ledger verdict quoting both sides (K8) | process |
| AL-26 | a suppression lacks rule, fingerprint, reason, a `DEC-`/`ICR-` ID and an expiry ≤ 30 days out, is expired, or the count grew outside `steward/*` | process |
| AL-27 | a finding is not in `archlint-baseline.json`, or the baseline gained an entry against the merge base | all |
| AL-28 | (WP-00) a `test = "required"` decision or invariant whose crate exists has no test naming its ID | SC |
| AL-29 | (WP-00) a WP PR's `Concern-Revs:` trailer is older than `main` for a concern its crates touch | LD |
| AL-30 | a `root` crate binds an action or owns a concern; an `interface` crate owns a `transform` concern; the DAG is deeper than `max_depth` (C003, C048, C098; K4) | BV |
| AL-31 | a dep's WP is in a later wave than the dependent's; an edge A → B where A is linked into a process B is not (C100, C052; K4) | BV |
| AL-32 | a crate or third-party licence is not allowed by the licensing decision, or a `toml cargo` block sets another (C118; K4) | BV |
| AL-33 | an edge's `features` names an undeclared feature, or a `test` crate is a normal dependency of a non-test crate (C156) | BV |
| AL-34 | a `TBD:` placeholder exists outside `target/*/staging` (K22) | ND |
| AL-35 | a Class A path (`manifest.d/**`, `data/**`, port contract or concern-owner anchors) changes on a `WP-*` branch, or on any branch but `steward/*` while a wave is open (`waves.lock` present) (K17, K22) | process |
| AL-36 | a section's content hash differs from its `facts.jsonl` entry (stale fact index) | process |

`gen --check` covers generated blocks, inline values and the Rust generated from the manifest;
`xtask check-dag` covers code edges by crate path, all targets, normal / build / dev edges apart
(`prior-art.md:289-313`). The deterministic canaries (§7 W2) are archlint test fixtures, so CI
proves on every archlint change that a moved threshold, a renamed key, a second owner and a
superseded citation each fail (K15).

## 6. Decisions

| Prefix | Meaning | Form | Recorded in |
|---|---|---|---|
| `DEC-` | owner decision (statuses in rule 6) | `DEC-YYYY-MM-DD-slug` | `docs/decisions.md` entry with anchor + `decisions.toml` row |
| `PROV-` | provisional default from plan §10 | `PROV-NN`, keeping the plan number | `decisions.toml` only; plan §10 is generated from it (K13) |
| `PAT-` | building-block pattern | `PAT-NN` (was `P7` in `02-building-blocks.md:18`) | 02 + `[[name]]` row |
| `WP-`, `ICR-` | work package, interface change | unchanged | `plan.md`, `docs/icr/` |

A `[[decision]]` row holds `id`, `status`, `date`, `source` (owner chat with date, or review URL),
`text` anchor, `supersedes`, `superseded_by`, `affects` (entity IDs only, never a document
section; K14), `creates` (new entity IDs), `sets` (`key = value`), `retires` (tokens that become
`[[retired]]` rows), `test` (required or none), `urgent`, `class` (value, owner, flow; §7 W1).

1. **One channel** (K16): a ruling exists only once committed to `docs/decisions.md` with an ID,
   by W0. Workflow scripts and prompts carry IDs and load the text from `docs/decisions.md` at run
   time; they never carry decision text.
2. **Append-only:** accepted text never changes, only `status` and `superseded_by`; a change is a
   new row (PEP 1, Nygard; `prior-art.md:170-211`).
3. **Both directions:** `supersedes` on the new row, `superseded_by` on the old; a superseding row
   lists `retires` or gives `retires_none = "<reason>"`.
4. **Decisions win** (`README.md:6-7`): a design that must deviate records a `proposed` DEC and
   follows the live one until the owner accepts. Every proposed DEC appears in the owner digest
   with the owner sections of its `affects`, so a pending conflict is counted, never silent (K7).
5. **No "every decision cited" rule** (K21): process decisions such as the logo
   (`decisions.md:205`) have no implementing section. Citation is checked only where `affects`
   names an entity (AL-22).
6. **Statuses:** `proposed` (from a brief or an agent, awaiting the owner), `recorded` (the owner
   ruled; propagation pending), `live`, `superseded`, `rejected`. W0 records; W1's promotion flips
   `recorded` to `live` in the PR that propagates it, so `main` never holds a live ruling that the
   lint fails on. AL-08 and AL-22 apply to `live` rows.

**First intake (W0, now, before any other workflow):**

- `DEC-2026-10-11-first-run`, class flow, from the uncommitted entry in `docs/decisions.md`:
  supersedes `PROV-07`; retires key `classify.ask_above`, value "5,000 messages", phrases "asks
  above" and "cost confirmation above"; affects action `classify.run` and namespace `classify`
  (`07-actions-api-cli.md:226`), onboarding state 9, `WP-07`, threat T23 (`14-security.md:69`), the
  cut-line row (`18-risks-and-cut-line.md:42`), the `[classify]` key table (`13-config.md:109-117`);
  creates the cap key and constant and the one-action backlog start; sets the cap to 1,000. The
  manifest v1 compiler fixes the entity IDs.
- `DEC-2026-10-11-main-usable` and `DEC-2026-10-11-test-accounts`, live, with the owner's words
  from `arch5-rederive.js:36-37` and this session's chat ("they can send and receive … brand new
  emails"). The test-accounts row affects a new concern `live-test-policy`, owned once in 16, whose
  stale owner text is `16-testing-ci-release.md:22` (C177).
- D1, D2, D3 and D5 (`arch5-inventory.js:35-39`) as `proposed` rows
  (`DEC-2026-10-11-walking-skeleton`, `-crate-reasons`, `-jev-offline`, `-merge-gate`). D4 is
  recorded `superseded` by test-accounts. D5 adds an independent review pass that
  `decisions.md:206` ("merged to `main` once CI passes") does not require: the owner chooses at
  intake whether D5 refines or replaces that line.
- Migrate every `P\d+` token: plan rows to `PROV-NN`, 02 patterns to `PAT-NN`.

## 7. Workflows

Every docs-mutating workflow stages and promotes as §8 says. During build waves (`waves.lock`
present) Class A changes reach `main` only through the steward (W5); Class B changes merge any time.

### W0 intake (any owner input: chat, brief, review)

1. Quote the owner's words verbatim into a `docs/decisions.md` entry with anchor; add the row with
   `class`, `affects`, `creates`, `sets`, `retires`, status `recorded` (or `proposed` for a brief).
2. Commit both files on `decision/<slug>` with `but-commit-paths`, then open the PR. Exit: AL-01,
   AL-20 pass.
3. Before launching any workflow script, the orchestrator runs `wf-lint <script>` (dev helper,
   `~/.local/share/neomail-dev/bin`): it fails on "OWNER DECISION", "Owner input" or "(binding)"
   without a `DEC-` ID that resolves in `docs/decisions.md` (K16).

### W1 decision change (branch `decision/<slug>`)

The `class` sets the cost (K25, K26, K27).

| Class | What changes | Re-derive | Review |
|---|---|---|---|
| value / name | manifest rows, `[[retired]]` rows, `gen --write` | none | one lens (decision compliance) over the impact set |
| owner change | new owner unit; old definitions removed; old names tombstoned (`ui.animation` → `ui.motion`, `motion/integration.md:141-142`) | the old and new owner units | lenses below, until dry |
| flow / behaviour | the rule itself | owner anchor + citing sections + neighbours, **section by section, never whole files** | lenses below, until dry |

| Stage | Does | Exit condition |
|---|---|---|
| 1 Impact set | script: owner sections of `affects`; citers of superseded IDs; AL-08 hits; suspect links; fact-index neighbours; for owner change, every section under the old owner and every use of the concern's keys and types; **one-time grep of the old values and old names**; writes `impact.json` | every hit listed |
| 2 Triage | one agent marks each grep hit "same fact" or "unrelated" with a quote (`04-providers-and-sync.md:202` "chunks of 5,000" is unrelated) (K20) | every hit assigned to one file's writer, or "no change" with a reason |
| 3 Edit | one writer per file in staging; prompt = decision text loaded by ID + manifest slice + the file's impacted sections | AL clean on the staging file |
| 4 Gate | archlint + `gen --check` over staging | zero new findings, suspects or tombstone hits |
| 5 Review | lenses: decision compliance, ownership, process visibility, lifecycle and ordering, naming; the verifier refutes by default and must quote both sides; the file's writer fixes | a round with zero new verified findings of major or higher; at most 4 rounds, then the open list goes to the owner |
| 6 Promote | §8; PR; bump `rev` of each affected concern; flip `recorded` → `live` | merged; Class A part queued for W5 during waves |

**Severity is set by class, not by the reviewer** (K25; `arch5-rederive.js:131,140` dropped
minors after round 1): two locations giving different values or names for one fact, and any
security-flow issue, are at least major. Minors are fixed in the same pass and stay in the ledger.

### W2 audit (read-only; output is PRs)

Trigger: incremental **during each build wave**, over sections changed since the SHA the last audit
recorded, plus their fact-index neighbours; full at milestones (end of each design round, including
this cleanup and the 19-motion merge, and each alpha tag). No weekly run and no steward-window run
(K17, K30; `plan.md:121-127`).

| Stage | Does | Exit condition |
|---|---|---|
| 1 Deterministic | archlint, property queries over manifest + fact index, `[[probe]]` commands (`free -g`, `nproc`) | report written |
| 2 Extract | facts from changed sections into `facts.jsonl`, cached by content hash | index covers every changed section |
| 3 Detect | deterministic comparisons first (two defaults, two definers, edge not in DAG); then one reviewer per concern cluster and lens, step by step | findings with both locations |
| 4 Verify | refute by default; refuting needs a quote of both sides | verdict per finding |
| 5 Dedupe | fingerprint by root cause (entity ID + sorted locations) against `audit-ledger.jsonl` | only new root causes remain |
| 6 Fix in staging | one fixer per file; each finding a rule or generation could have caught is tagged `lintable` and gets an AL rule, manifest field or tombstone | AL clean on staging |
| 7 Promote | §8; Class B PR now; Class A as ICRs for W5 | PRs open, ledger updated |

**Canaries.** Deterministic canaries are archlint fixtures (§5). Before each full audit, a script
plants 3 semantic canaries on a scratch branch (a boundary flip, a prose rule conflict with no link
between its sides, a lifecycle-order break); the audit counts only if it finds ≥ 2 of 3; otherwise
the prompts are revised and it reruns. The same set runs through the W4 docs review once per
milestone. The scratch branch is deleted, never merged (`prior-art.md:403-409`).

### W3 contract-first fan-out (design tasks with > 1 writer; unsplittable tasks get one writer, `prior-art.md:339-356`)

| Stage | Does | Exit condition |
|---|---|---|
| 1 Contract | the lead commits the manifest slice, closed catalogues, owner map and live decision IDs; one file per writer | archlint clean |
| 2 Write | writers in parallel, each in its staging copy, contract injected verbatim; a new shared name is written `TBD:Name` and appended to `target/<run>/requests.jsonl`, never asked as a blocking question (K22) | AL clean on staging except AL-34 |
| 3 Register | the contract owner (one agent) accepts, renames or merges each request, writes the rows, replaces every `TBD:` | zero `TBD:` |
| 4 Review | the W4 docs review over the written files **and their fact-index neighbours** (K1) | as W1 stage 5 |
| 5 Promote | §8; PR | merged (Class A via W5 during waves) |

### W4 every PR

- **CI:** archlint, `gen --check`, `dag`, `deny` (bans, licenses), conformance suites, AL-28, AL-29.
- **Docs review, required for any PR that changes `docs/`** (K1). The `docs-impact` script
  re-extracts the changed sections into `facts.jsonl` in the same PR, then builds the review set:
  changed sections, plus every section that shares a concern, key, name, action or keybinding with
  them in the fact index, plus suspect links. The W1 lenses review both sides of each pair; the
  verifier refutes by default; the PR's author fixes; repeat until dry (≤ 4 rounds). The run posts
  the required status `docs-review` and writes a ledger verdict for every link it re-blesses.
- **Code review:** the WP reviewer checks behaviour against the contract anchors the WP cites.
  Values, defaults and names are generated from their `[[source]]`, so reviewers do not check them
  (K24). A deviation is an ICR carrying any manifest patch; the WP goes on with a local adapter and
  `#[ignore = "ICR-NNN"]` (`plan.md` §2.2).
- **Helpers:** `but-commit-paths` refuses `manifest.d/**` and `data/**` on `WP-*` branches and
  runs no lint (K28: it holds the global `but.lock` for up to 600 s, `but-commit-paths:10-11`).
  `wp-publish` runs `archlint check` over the whole repo before opening the PR.
- **No cargo on the docs side** (K32): docs and audit agents never run cargo; code checks run in
  GitHub CI (`nm-heavy:8,13`: two heavy slots machine-wide).

### W5 steward window (S0–S5; one agent, no fan-out, `plan.md:121-127`)

In order: merge queued Class A PRs and ICRs oldest first, each with its code; `gen --write`;
archlint; the walking-skeleton probe (the only cargo run on the docs side); shrink the baseline and
expire suppressions; post the owner digest (proposed DECs with their `affects` owner sections,
`recorded` → `live` lag, §10 metrics). Exit: nothing in the Class A queue older than one phase,
zero suspects, baseline no larger than at the last window. An urgent ruling during a wave opens an
unscheduled window: no new WP starts, running WPs reach a checkpoint, then W5 runs.

## 8. Staging, the docs lock and atomic promotion

GitButler applies every branch to one working tree, and `but-commit-paths` commits every
uncommitted hunk under a path whoever wrote it (`but-commit-paths:13-18`). So (K18):

1. **Stage privately.** A docs-mutating run copies the files it needs to `target/<run>/staging/`
   and edits only there. Its checkpoint is `target/<run>/journal.json` (base SHA, per-file state),
   not a commit: `target/` is gitignored (`.gitignore:2`). A crash loses nothing on `main` and the
   run resumes from the journal.
2. **Promote under one lock.** `docs-promote <run>` (dev helper) takes
   `flock ~/.local/share/neomail-dev/docs.lock`; if a staged file changed since the base SHA, it
   three-way merges (`git merge-file`) and sends conflicts back to the run; it copies every staged
   file, runs `archlint check`, makes one `but-commit-paths` call and releases the lock. Only the lock
   holder has uncommitted `docs/` edits, so no hunk is stolen; re-running a promotion is idempotent.
3. **Nobody edits `docs/` in the working tree directly**, including WP agents and the motion lead;
   other runs produce staging copies or change-request files for a run that does.

## 9. Agent rules (into `CLAUDE.md` via WP-00)

1. Take every key, number, name, owner and edge from `manifest.d`; write a link to its owner anchor
   or `<!--c:id-->`, never a retyped copy.
2. A new shared name, key or constant is `TBD:Name` plus a request; only steward and decision
   branches write manifest rows.
3. An owner ruling enters `docs/decisions.md` through W0 before any workflow uses it; scripts and
   prompts carry `DEC-` IDs, never decision text.
4. Never edit an accepted decision; supersede it with a row that lists what it retires.
5. Cite decisions and sections by anchor link; never a bare `§N`.
6. Never edit `docs/` in the shared working tree; stage under `target/<run>/staging` and promote
   with `docs-promote`.
7. Outside `steward/*` and `decision/*`, never edit generated blocks, `links.lock`, the baseline,
   suppressions, `manifest.d`, `data/`, `tools/archlint`, `xtask` or CI files.
8. Never turn a check green by weakening it: a suppression, a reworded number, a second owner, or
   a re-blessed suspect link without a quoted verdict.
9. Contract deviations are ICRs with a local adapter.
10. Tests for `test = "required"` decisions and invariants carry the ID in their name.
11. Findings and refutations quote both sides with `path:line`; severity follows the class (§7 W1).
12. Docs and audit agents never run cargo; code checks run in CI.

## 10. Cadence and metrics

Every PR: L1 gates, plus the W4 docs review when `docs/` changes; `wp-publish` runs archlint
first. Every owner input: W0, then W1. Every build wave: W2 incremental. Every steward window: W5
with probes. Every milestone: W2 full with canaries. Weekly: lychee over external URLs only (P13),
never a gate.

| Metric | Target | If missed |
|---|---|---|
| archlint findings not in the baseline | 0 (gate); the baseline only shrinks, 0 before WP-00 | the steward fixes before opening a wave |
| AL-10 baseline entries (restated numbers not inline) | only shrinks; ≤ 20 at S0. A drifted copy is a new finding that fails CI, so drift raises the count instead of hiding in it (K2) | convert to `<!--c-->` or a link |
| verified audit root causes per W2 run (ledger, deduplicated) | full: ≤ 5 at S0, 0 from S2; incremental: 0 open at the next window | each `lintable` one gets a rule, field or tombstone |
| escapes (ledger `caught_by` later than `cheapest`) | falling every milestone | add the rule, field or lens |
| propagation lag (`recorded` → `live`) | urgent ≤ 1 day; others by the next steward exit | W5 cannot exit |
| `proposed` DECs older than one phase | 0 | the owner digest asks again |
| suspect links on `main`; expired suppressions | 0; 0 (≤ 10 live) | gate; steward review |
| audit recall | semantic canaries ≥ 2/3 in W2 and W4; deterministic fixtures 4/4 | rerun with revised prompts |
| review rounds | ≤ 4 per W1, W3 or W4 run | the open list goes to the owner |

## 11. What we deliberately do not do

| Not doing | Why | Source |
|---|---|---|
| Sphinx-needs, StrictDoc, Doorstop or OpenFastTrace as tools | each forces a format migration of 19 files; we copy their mechanisms | `prior-art.md:150-168` |
| A calendar-weekly LLM audit | audits trigger on diffs and waves; unchanged text is not re-read | `redteam.md:359-373` |
| Equality-only value matching, or bare-number matching | equality misses every drifted copy (C071, C167, C168); bare `\b20\b` hits 34 unrelated lines in 13 files (K19) | §1; K19 evidence |
| An all-names glossary | 282 CamelCase types and 220 dotted tokens; closed catalogues + types in ≥ 2 files instead | K22 evidence |
| "Every live decision is cited" | process decisions have no implementing section; it invites token citations | `decisions.md:199,205,206`; K21 |
| Warning-level lint rules, commit-time lint | warnings rot; commit-time lint blocks checkpoints and holds `but.lock` | K28, K31 |
| Whole-file re-derivation | the agent that died mid-rewrite was doing it; 02 has 618 lines, plan 1,065 | `wc -l`; K26 |
| "The branch is the staging copy" (rev 1) | GitButler shares one working tree across applied branches; replaced by §8 | `but-commit-paths:13-18`; K18 |
| Vale; Alloy, TLA+ or P; freshness dates; requirement-to-test matrices | archlint knows the owners; the defects are names, owners and edges; staleness is event-driven; only `test = "required"` traces | `prior-art.md:215-226, 315-335, 413-421, 87-96` |
| One "read everything, list contradictions" prompt | recall 0.296 to 0.566 | `prior-art.md:391-409` |

## 12. Rejected challenges

All other challenges (K1–K32) are applied above.

- **K19, "remove the value lint": rejected in part.** `12-extensions-agents-mcp.md:13` ("other TUI
  callbacks 20 ms") has no `= literal`, no "default" and no retired token, so (b) and (c) alone miss
  C071. AL-10 stays, scoped to sentences that name the constant, key or alias, and fires whatever the
  value. The noise figure (34 hits for `\b20\b`) is for unscoped matching. (b) and (c) are adopted.
- **K31, "delete AL-13": rejected in part.** The headline case says "`standard` applies the
  classifier's redaction" (`12-extensions-agents-mcp.md:190`). It becomes checkable once "the
  classifier" resolves through `[[alias]]` (K3). Generated "Depends on / used by" lines and the
  errors-only policy are adopted.
- **K23, "the permission matrix is not in the manifest": already covered.** Rev 1 held it as rule
  tables (`profiles.toml`; `catalogue.toml` with capability and confirm policy) checked by AL-18 and
  generating the 07 matrix. Rev 2 adds only K12's naming fix.
- **K29 and K30 (trend metric, weekly trigger): already covered.** Rev 1 tracked new verified
  fingerprints per run and escapes, not a raw count, and rejected a calendar audit
  (`redteam.md:359-373`). Rev 2 adds root-cause dedupe, the `lintable` tag and K17's trigger move.
- **K7, "D5 directly clashes with `decisions.md:206`": not shown.** Line 206 makes CI passing the
  merge condition; D5 adds an agent review pass. Whether that refines or replaces the line is the
  owner's call, so it is asked at intake rather than assumed (§6).
- **K15, "run the canaries against L1 at each steward phase": rejected.** A steward window runs
  nothing else (`plan.md:126-127`). The deterministic canaries are archlint fixtures instead, so they run
  in CI on every archlint change, which is more often.

## 13. Rollout

1. **Now:** W0 first intake (§6), including the D5 question to the owner.
2. **This cleanup** finishes as a W3 run: manifest v1 with every §4 table, the rule tables, owner
   anchors, and `facts.jsonl` seeded from `target/workflows/arch5/extract/`.
3. **19-motion** merges as a W1 owner-change run against manifest v1: 08 and 02 motion text moves
   to 19 and `ui.animation` is tombstoned. The motion lead stages; it does not edit `docs/` in place.
4. **`steward/archlint`:** archlint v1 (all rules except AL-28 and AL-29), `gen`, and the helpers
   `docs-impact`, `docs-promote` and `wf-lint`. The baseline starts at today's findings, and W2
   fixers drive it to zero before WP-00.
5. **WP-00:** AL-28, AL-29, `check-dag`, `deny` (bans, licenses), Rust generated from the manifest,
   code exporters for actions and events, `archlint` and `docs-review` as required checks, §9 into
   `CLAUDE.md`. `waves.lock` is created when wave 0 opens.
6. **A full W2 with canaries before S0.** S0 is the first W5.
