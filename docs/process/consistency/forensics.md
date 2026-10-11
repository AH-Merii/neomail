# Forensics: root causes of the ~210 design contradictions, and the cheapest check for each

Scope: every contradiction and every `restates` entry (concerns and config keys) in
`target/workflows/arch5/extract/*.json`. Per-item data: `forensics.json` (555 items) next to this file.
Analyst: one forensics agent, one pass. The 211 contradiction labels are hand-assigned with a one-line
reason each (`cls_a.txt`, `cls_b.txt`); nobody else has checked them, so treat the per-cell counts as ±a few.

## Method

1. **Load.** `load.py` parses the 14 top-level extract files (actions, auth, blocks, classify-ext,
   config-platforms, core, decisions, features, plan, providers, quality, security, storage, tui). It reads
   them leniently with `json.JSONDecoder.raw_decode` in a loop, so it would accept concatenated documents.
   In practice each file held one document (`python3 load.py` prints `1` for each). The `parts/`,
   `*.parts/` and `parts-features/` directories and the `blocks_gen*.py` files were not read.
2. **Deduplicate contradictions.** There are 256 contradiction reports. `items.py` groups them by the
   unordered pair (`where`, `conflicts_with`), with each side reduced to its first file:line. Exact match
   gives **211 groups**, which matches the "~210". A ±2/±4/±8 line tolerance gives 203/198/195
   (`python3 items.py`). Exact match was kept, so near-duplicates seen from different sides stay separate,
   e.g. C009 and C066 (agent profile).
3. **Restatements.** There are 187 concern `restates` and 157 config-key `restates` (344 in all). `build.py`
   links a restatement to a contradiction when two things hold. First, they share a file, with a line
   within ±3 (keys) or ±2 (concerns) of a file:line the contradiction cites. Second, they share content
   words: at least 3, or at least 2 identifier-like ones; the best overlap wins. A key restatement is also
   linked when the contradiction names its dotted key and cites the same file. Linked restatements
   (**64**) take the contradiction's labels. Unlinked ones (**280**) are *latent*: consistent today, but a
   copy that can drift. They get the root `restated-fact-drift`, or `missing-definition` for the 7 keys
   that no extract file defines. Their check is `manifest+lint` if the text has a key, a dotted name or a
   number with a unit; otherwise it is `targeted semantic review`.
4. **Root causes** (axis a): RF restated-fact-drift, LD late-decision-not-propagated, ND naming-drift,
   OA ownership-ambiguity, BV dependency/boundary violation (crate DAG, process boundary, wave order),
   HR half-applied-revision, DR dangling-reference, SC semantic-rule-conflict, MD missing-definition,
   OT other. Each item gets one primary root: whatever, once fixed, removes the contradiction. Example: a
   stale threshold that cites a superseded decision is LD, not RF.
5. **Catching check** (axis b). "Caught" means the check, as the draft defines it plus the assumptions
   below, would have failed the merge that brought in either side. Preventing the restatement in the first
   place counts as a catch. Example: the fact-ownership lint rejects "5,000 messages" in 03 when 03 is
   first written, so the stale copy never exists. Each item is assigned the **cheapest** such check, in
   this order:
   - **L, manifest+lint** (R3, draft 1-2). Deterministic. It needs the manifest to carry: the glossary
     (types, keys, actions with their argument schema, profiles and their capabilities, env vars, ID
     prefixes, milestone labels); named constants with one owner; retired keys; a per-crate allow-list of
     third-party dependencies and `unsafe`; and one owner per concern, capability, action namespace binder,
     port home and WP. It flags: unknown names; a second value for a key or constant; a registered
     constant's value as a literal outside its owner; "X uses Y" or a dependency that is not a DAG edge;
     process tags against edges; a port with no conformance suite or no implementing WP; and a link or
     § reference that does not resolve to the heading title it cites.
   - **T, decision-traceability lint** (draft 3). It flags a section that cites a superseded ID, a live
     decision that no section cites, and two decisions with the same ID. *It only sees decisions recorded
     in the repo.*
   - **G, generated-table drift** (`gen --check`). It catches lists and tables that are hand copies of
     manifest data: crate and port tables, implementation lists, the CLI tree from the catalogue, preset
     lists and directory layouts.
   - **C, compile probe** (R4, R7: walking skeleton). It builds the code, runs the conformance suites and
     `cargo-deny`, and enforces invariants in types (for example, an RPC egress that only accepts
     `Redacted<T>`).
   - **S, targeted semantic review** (draft 4 and 7). An LLM reviews the impact set: the changed sections,
     their linked neighbours, and every section that cites the same concern, key, name or decision.
   - **A, routine full audit** (draft 7). It covers every file, plus facts outside the docs.
   - **N, nothing practical.**

## Counts: root cause x cheapest catching check

Codes: L manifest+lint, T decision-traceability lint, G generated-table drift, C compile probe, S targeted
semantic review, A routine full audit, N nothing practical. Each item is counted once, under its cheapest
check. The cumulative rows read: "share caught if every check up to and including this one runs".

### Contradictions (deduplicated): n = 211

| root cause | manifest+lint (L) | decision-traceability lint (T) | generated-table drift (G) | compile probe (C) | targeted semantic review (S) | routine full audit (A) | nothing practical (N) | total |
|---|---|---|---|---|---|---|---|---|
| restated-fact-drift (RF) | 15 | . | 9 | 1 | 3 | . | . | 28 |
| late-decision-not-propagated (LD) | 9 | 13 | . | . | 2 | . | . | 24 |
| naming-drift (ND) | 17 | 1 | 2 | . | 2 | . | . | 22 |
| ownership-ambiguity (OA) | 24 | . | . | . | 10 | . | . | 34 |
| dependency/boundary violation (BV) | 31 | . | . | 4 | 2 | . | . | 37 |
| half-applied-revision (HR) | 2 | . | 2 | . | 4 | . | . | 8 |
| dangling-reference (DR) | 4 | . | . | . | 1 | . | . | 5 |
| semantic-rule-conflict (SC) | 1 | . | . | 4 | 23 | 1 | . | 29 |
| missing-definition (MD) | 14 | . | 1 | 1 | 2 | . | . | 18 |
| other (OT) | . | . | . | 1 | 3 | 2 | . | 6 |
| **total** | **117** | **14** | **14** | **11** | **52** | **3** | **0** | **211** |

| check (cheapest first) | caught at this step | cumulative | cumulative share |
|---|---|---|---|
| manifest+lint (L) | 117 | 117 | 55.5 % |
| decision-traceability lint (T) | 14 | 131 | 62.1 % |
| generated-table drift (G) | 14 | 145 | 68.7 % |
| compile probe (C) | 11 | 156 | 73.9 % |
| targeted semantic review (S) | 52 | 208 | 98.6 % |
| routine full audit (A) | 3 | 211 | 100.0 % |
| nothing practical (N) | 0 | 211 | 100.0 % |

### Restatements tied to a contradiction: n = 64

| root cause | manifest+lint (L) | decision-traceability lint (T) | generated-table drift (G) | compile probe (C) | targeted semantic review (S) | routine full audit (A) | nothing practical (N) | total |
|---|---|---|---|---|---|---|---|---|
| restated-fact-drift (RF) | 7 | . | 2 | . | 1 | . | . | 10 |
| late-decision-not-propagated (LD) | 2 | 6 | . | . | . | . | . | 8 |
| naming-drift (ND) | 2 | . | . | . | . | . | . | 2 |
| ownership-ambiguity (OA) | 6 | . | . | . | 6 | . | . | 12 |
| dependency/boundary violation (BV) | 8 | . | . | 8 | . | . | . | 16 |
| semantic-rule-conflict (SC) | . | . | . | 2 | 9 | . | . | 11 |
| missing-definition (MD) | 2 | . | . | . | 3 | . | . | 5 |
| **total** | **27** | **6** | **2** | **10** | **19** | **0** | **0** | **64** |

| check (cheapest first) | caught at this step | cumulative | cumulative share |
|---|---|---|---|
| manifest+lint (L) | 27 | 27 | 42.2 % |
| decision-traceability lint (T) | 6 | 33 | 51.6 % |
| generated-table drift (G) | 2 | 35 | 54.7 % |
| compile probe (C) | 10 | 45 | 70.3 % |
| targeted semantic review (S) | 19 | 64 | 100.0 % |
| routine full audit (A) | 0 | 64 | 100.0 % |
| nothing practical (N) | 0 | 64 | 100.0 % |

### Latent restatements (consistent today): n = 280

| root cause | manifest+lint (L) | decision-traceability lint (T) | generated-table drift (G) | compile probe (C) | targeted semantic review (S) | routine full audit (A) | nothing practical (N) | total |
|---|---|---|---|---|---|---|---|---|
| restated-fact-drift (RF) | 192 | . | . | . | 81 | . | . | 273 |
| missing-definition (MD) | 7 | . | . | . | . | . | . | 7 |
| **total** | **199** | **0** | **0** | **0** | **81** | **0** | **0** | **280** |

| check (cheapest first) | caught at this step | cumulative | cumulative share |
|---|---|---|---|
| manifest+lint (L) | 199 | 199 | 71.1 % |
| decision-traceability lint (T) | 0 | 199 | 71.1 % |
| generated-table drift (G) | 0 | 199 | 71.1 % |
| compile probe (C) | 0 | 199 | 71.1 % |
| targeted semantic review (S) | 81 | 280 | 100.0 % |
| routine full audit (A) | 0 | 280 | 100.0 % |
| nothing practical (N) | 0 | 280 | 100.0 % |

Weighted by raw extractor reports (256 before deduplication): manifest+lint 150, decision-traceability lint 20, generated-table drift 16, compile probe 12, targeted semantic review 54, routine full audit 4.

## Examples: three per root cause, with sources

### restated-fact-drift (RF)

- **C081** `docs/architecture/13-config.md:80` vs `docs/architecture/11-images-and-attachments.md:103`: attachments.save_dir default ~/Downloads vs dirs::download_dir() else $HOME Check: **manifest+lint** - `attachments.save_dir` is a declared key with one default; lint flags a second default.
- **C167** `docs/architecture/04-providers-and-sync.md:149` vs `docs/architecture/05-auth-and-secrets.md:210`: Gmail size limit: resumable send up to 35 MiB (04, 11:158) vs identity policy limit Gmail 25 MB (05, 11:154, 04:264) Check: **manifest+lint** - the Gmail send limit would be one named constant; 35 MiB and 25 MB are two values for it.
- **C071** `docs/architecture/12-extensions-agents-mcp.md:12-13` vs `docs/architecture/01-process-and-crates.md:83`: Lua budgets: 12 gives rules 20 ms and other TUI callbacks 20 ms; 01 gives 50 ms per hook and 'everything else off-frame' Check: **manifest+lint** - Lua budget is a named constant; 20 ms vs 50 ms is a value restated outside its owner.

### late-decision-not-propagated (LD)

- **C024** `docs/architecture/13-config.md:116` vs `docs/decisions.md:76-79 (also 06-classification.md:181, 03-domain-and-storage.md:137, 18-risks-and-cut-line.md:42, docs/plan.md:1013)`: Same stale 5,000-message ask restated in config, classification, storage and risks docs (all now wrong) Check: **manifest+lint** - `classify.ask_above` would be a retired key after the decision; unknown key fails the lint.
- **C023** `docs/architecture/05-auth-and-secrets.md:182` vs `docs/decisions.md:76-79`: Onboarding state 9 still 'asks above 5,000 messages (P7)'; the binding owner decision replaces P7: classify last 365 days automatically without asking, capped at newest 1,000, then show an estimate for the rest with a one-action start Check: **decision-traceability lint** - 05:182 cites P7, which decisions.md:76 says it replaces; citing a superseded ID fails T.
- **C178** `docs/architecture/16-testing-ci-release.md:65` vs `docs/decisions.md:206`: Merge flow is auto-merge squash on green CI with no independent review agent step; provisional D5 requires an independent review agent pass before merge Check: **decision-traceability lint** - D5 exists only in the task brief; once recorded with an ID, T flags it as cited by no section.

### naming-drift (ND)

- **C063** `docs/architecture/06-classification.md:123` vs `docs/architecture/13-config.md:117`: redaction level key named privacy.redaction in 06 but classify.redaction in 13 (definition and security-key list) Check: **manifest+lint** - glossary has one redaction key; the other name is unknown and fails the lint.
- **C047** `docs/architecture/02-building-blocks.md:255` vs `docs/architecture/README.md:70`: daemon picker trait is named `DaemonPickerSource`; README names it `PickerSource` (daemon) Check: **manifest+lint** - one port name in the manifest; the other is an unknown name.
- **C079** `docs/architecture/13-config.md:53` vs `docs/architecture/08-tui.md:137`: hint setting named ui.hints in 13 but keymap.hints in 08 Check: **manifest+lint** - one key; `keymap.hints` vs `ui.hints` - the unknown key fails.

### ownership-ambiguity (OA)

- **C096** `docs/architecture/README.md:70` vs `docs/plan.md:250`: audit trait has two homes: AuditSink port in nm-ports vs 'audit and undo sink traits' in nm-api's Dispatcher Check: **manifest+lint** - manifest lists one audit port with one home crate; a second trait in nm-api fails port completeness.
- **C152** `docs/plan.md:391` vs `docs/plan.md:690`: remote-image fetching owned twice: nm-net 'remote-image client (no cookies, referer(false), size and type caps)' and nm-attach RemoteImages 'hardened fetch through Egress' Check: **manifest+lint** - one capability owner per crate in the manifest; two crates claim remote-image fetching.
- **C189** `docs/architecture/03-domain-and-storage.md:179` vs `docs/architecture/03-domain-and-storage.md:470`: One writer actor 'owns the only read-write connection to each file', yet nm-search 'owns the frecency table's writes through its own short transactions' (also plan.md:577 WP-49). sender_frecency is in index.db (03:377). Check: **targeted semantic review** - "writer actor owns the only RW connection" vs "nm-search writes frecency": prose ownership of a table, needs a reviewer.

### dependency/boundary violation (BV)

- **C065** `docs/plan.md:471` vs `docs/architecture/01-process-and-crates.md:183`: WP-10 puts mcp.redaction in nm-mcp, which may only depend on nm-types, nm-config, nm-api; and nm-mcp is a daemon client, so redaction would run after content left the daemon Check: **compile probe** - redaction must happen before text leaves the daemon; a `Redacted<T>`-only RPC type makes the skeleton fail to compile.
- **C030** `docs/architecture/05-auth-and-secrets.md:65` vs `docs/architecture/04-providers-and-sync.md:60-66; docs/architecture/14-security.md:104-108`: nm-auth is specified with oauth2 over a raw 'reqwest 0.13 AsyncHttpClient shim', while Egress is 'the only way any crate gets an HTTP client' and OAuth is an Egress purpose Check: **manifest+lint** - `reqwest` allowed only in nm-net by the manifest dep rules; nm-auth using it fails.
- **C163** `docs/architecture/04-providers-and-sync.md:338` vs `docs/architecture/README.md:89`: Scheduler tree places ClassifierPipeline and WriteBackPlanner under the global Scheduler (nm-sync-sched) while README assigns pipeline and write-back planner to nm-classify; the DAG forbids nm-sync-sched -> nm-classify Check: **manifest+lint** - nm-sync-sched -> nm-classify is not a DAG edge; the "X uses Y" lint fails.

### half-applied-revision (HR)

- **C190** `docs/architecture/03-domain-and-storage.md:173` vs `docs/architecture/README.md:82`: 03 §2 places blobs.db under nm-store (section title, writer per file, weekly sweep, view copies); README and plan give blobs.db, BlobStore, the sweep and the runtime view-copy manager to nm-blobs. 03 never names nm-blobs or BlobStore. Check: **targeted semantic review** - 03 predates the nm-blobs split; a reviewer reading the impact set of that split sees it.
- **C013** `docs/architecture/07-actions-api-cli.md:65` vs `docs/architecture/README.md:70`: Services port list in 07 omits TokenSource, PinStore, DecisionBackend, Ingest, BudgetBroker, Sender and the daemon picker port listed in README/plan; that port is PickerSource in README but DaemonPickerSource in 02-building-blocks.md:254 and plan.md:262 Check: **generated-table drift** - 07 port list is a hand copy; a generated list cannot drift.
- **C127** `docs/architecture/09-composer-and-spelling.md:26` vs `docs/architecture/01-process-and-crates.md:225`: 09 says GraphemeStr is rewritten safe so nm-text needs no unsafe exception; 01 lists nm-text among crates allowed a local unsafe allow for vendored GraphemeStr Check: **manifest+lint** - 09 was revised, 01 unsafe allowlist was not; per-crate unsafe allowlist in the manifest catches it.

### dangling-reference (DR)

- **C051** `docs/architecture/02-building-blocks.md:581` vs `docs/architecture/08-tui.md:346`: cross-ref '08 §9' for governor overrun dropping; 08 §9 is Learnability (governor is 08 §7.3, render perf 08 §8) Check: **manifest+lint** - § ref by anchor/title would not resolve.
- **C196** `docs/architecture/03-domain-and-storage.md:137` vs `docs/architecture/06-classification.md:180`: Cross-ref says the first-run cost estimate is in 06 §8; it is in 06 §7 (line 180). 06 §8 is 'Classify once'. Check: **manifest+lint** - same: 06 §8 is "Classify once".
- **C041** `docs/architecture/05-auth-and-secrets.md:144` vs `docs/architecture/17-docs-site-and-logo.md:32`: Help topic name: 05 points to 'nmail docs oauth-google#publish'; the docs site file is gmail-oauth.md Check: **manifest+lint** - link to a docs-site page that does not exist.

### semantic-rule-conflict (SC)

- **C134** `docs/architecture/10-search-and-pickers.md:116` vs `docs/architecture/09-composer-and-spelling.md:193`: <Tab> on the scope chip cycles scopes vs <Tab> multi-select in picker keys vs <Tab> selects completion in the shared engine used by the picker prompt Check: **targeted semantic review** - three meanings of <Tab> in one prompt; no shared key, needs a reviewer.
- **C186** `docs/architecture/14-security.md:30` vs `docs/architecture/03-domain-and-storage.md:502`: A4 says 'SQLCipher everywhere' with no residual, while storage.encrypt=false is a supported option Check: **targeted semantic review** - "SQLCipher everywhere" vs a supported `storage.encrypt = false`.
- **C172** `docs/architecture/16-testing-ci-release.md:35` vs `docs/architecture/14-security.md:112`: One Egress conformance suite is to run against the fake and every real implementation, but the real EgressClient must refuse 127.0.0.1 unless listed while FakeEgress is 'loopback only' and the test Egress panics on non-loopback; the same contract cannot hold f... Check: **compile probe** - one conformance contract cannot hold for both fake and real Egress; running the suite in the skeleton shows it.

### missing-definition (MD)

- **C017** `docs/architecture/07-actions-api-cli.md:185` vs `docs/architecture/13-config.md:24`: events.include_subjects is defined only in 07; 13 claims to show every section with defaults but has no [events] section Check: **manifest+lint** - key used in 07 but absent from the config owner; undefined key fails.
- **C203** `docs/architecture/08-tui.md:123` vs `docs/architecture/07-actions-api-cli.md:221`: keymap example binds message.open_links, not in the closed v0.1 catalogue (message namespace has no open_links); gen --check fails on unknown preset actions Check: **manifest+lint** - action not in the closed catalogue; unknown name fails (already `gen --check` per 08).
- **C166** `docs/architecture/04-providers-and-sync.md:372` vs `docs/architecture/03-domain-and-storage.md:261`: Ingest quarantine sets `state = quarantined`, but the messages DDL has no ingest/quarantine state (only body_state 0..2 and classify_state) Check: **compile probe** - `state = quarantined` has no column in the DDL; a migration/compile probe fails.

### other (OT)

- **C171** `docs/architecture/18-risks-and-cut-line.md:13` vs `/home/a_merii/.local/share/neomail-dev/bin/nm-heavy:8`: Risk row sizes the box as 16-core, 46 GB for 10–15 agents; the nm-heavy wrapper caps 2 heavy commands machine-wide at 3G each (nm-heavy:8-13) and `free -g` reports 15 GB total, 7 GB available; also 16:42 '15 branches', plan.md:3 '10–15 AI agents', 01:91 '10–15... Check: **routine full audit** - machine facts (RAM, agent count) drifted from reality outside the docs; only an audit that runs `free -g` sees it.
- **C118** `docs/decisions.md:13` vs `docs/architecture/01-process-and-crates.md:217`: Licence: decision says dual-licensed MIT OR Apache-2.0; design ships an MPL-2.0 crate nm-text in the repo Check: **compile probe** - design overrides a binding licence decision; cargo-deny licence check in the skeleton catches it.
- **C120** `docs/decisions.md:37` vs `docs/architecture/05-auth-and-secrets.md:99`: Decision: device code flow for headless/SSH; Google uses paste-URL instead (device flow lacks Gmail scopes) Check: **targeted semantic review** - design deviates from a decision with a reason but records no superseding decision; reviewer of the decision impact set.

## What the deterministic checks cannot catch

Deterministic checks (L, T, G) catch 145 of 211 (68.7 %); the compile probe adds 11 (73.9 %). The other
55 need a reader. Their shapes:

1. **Prose rules with no shared key, name or number** (23 of 29 SC, all S). Examples: three meanings of
   `<Tab>` in one prompt (C134, `10-search-and-pickers.md:116` vs `09-composer-and-spelling.md:193`);
   "SQLCipher everywhere" against a supported `storage.encrypt = false` (C186); PROMPT accepting only
   terminal input while its band accepts mouse (C202). No glossary entry ties the two sentences together.
2. **Ownership stated in prose about things the manifest does not model** (10 OA, all S). Who writes
   `keymap.toml` (C011, C208), a database table (C189: `03-domain-and-storage.md:179` vs `:470`) or a word
   list (C012); which section owns consent (C035) or motion rules (C058, C126); what the `agent` profile
   allows (C009, C066). Adding persisted files and tables with a single writer crate, and profiles with
   their capability sets, to the manifest would move about half of these to L. The rest are rules, not
   facts.
3. **Process boundaries and data flow.** The DAG lint sees `nm-mcp -> nm-classify` only where a doc
   states it as a dependency (C064, C076, C094, C110, C147: L). It cannot see that redaction runs *after*
   text has left the daemon (C065, C145: `plan.md:471`, `12-extensions-agents-mcp.md:190` vs
   `01-process-and-crates.md:183`), nor that one conformance contract cannot hold for the fake and the
   real Egress (C172, C173). Those need invariants in types and a walking skeleton (C) or a reviewer.
4. **Decisions that live outside the repo.** D1, D3, D4 and D5 come from a workflow task brief.
   `grep -n "D[1-5]\b" docs/decisions.md docs/architecture/*.md docs/plan.md` finds nothing, yet 7
   contradictions (C104, C106, C158, C177-C180) are against them. T can only catch these once each
   decision is recorded with an ID (draft step 4, first action).
5. **A decision that was recorded but cited by no section.** The 2026-10-11 first-run decision
   (`docs/decisions.md:76-79`) is cited nowhere: `grep -n "2026-10-11" docs/architecture/*.md docs/plan.md`
   finds nothing, and no doc mentions the 1,000 cap. T flags the decision itself, but it does not find the
   stale sites. Those come from the impact set: a grep for `ask_above`, "5,000" and P7 finds
   `03:137`, `05:182`, `06:181`, `13:116`, `18:42` and `plan.md:1013`
   (`grep -n "5,000\|5000\|ask_above" docs/architecture/*.md docs/plan.md`). This one decision accounts for 16 contradiction groups (C016, C023, C024, C037, C059-C062,
   C074, C075, C095, C107-C109, C146, C184) plus a wrong § reference to it (C196): 33 of the 256 raw
   reports.
6. **References that resolve to the wrong place.** A numbered § that exists but holds another topic
   (C185: `16-testing-ci-release.md:18` cites 14 §10, which does not cover approval binding). Only
   anchor-or-title references make this mechanical; C019, C051 and C196 are counted under L on that
   assumption.
7. **Facts outside the docs.** Machine size and agent count (C103, C171: `18-risks-and-cut-line.md:13`
   says 16-core / 46 GB; according to the extract, `free -g` reported 15 GB) have drifted from reality.
   Only an audit that runs commands sees them (A).
8. **Matching number literals gives false positives.** "5,000" also appears as an IMAP chunk size
   (`04-providers-and-sync.md:202`), and "5,000,000" as a token budget (`06-classification.md:178`). The
   constant-value check needs unit-aware matching and an allow-list, or it will be noisy.
9. **Latent copies.** 280 restatements agree with their owner today. 199 of them carry a key, name or
   number (L would force a link instead). 81 are prose paraphrases of a rule (for example, the full
   `derive_group` rule list in both 03 and 06), which no deterministic check can see. These are where the
   next drift will come from.

## Which lint rules do the work (the 117 L items, approximate)

The split comes from keyword-tagging each item's one-line reason, then hand-assigning the 13 untagged
ones: single owner of a concern, capability, binder or port home ≈ 38; dependency, DAG, process tag or
wave order ≈ 31; glossary (unknown name, key, action, env var or ID prefix, or a retired key) ≈ 28; a
named constant's value restated ≈ 14; links and § references 3; port completeness 2; per-crate `unsafe`
allow-list 1. Many of these depend on the extra manifest fields listed under L in the method (binders,
capability owners, third-party deps, env vars, ID prefixes, profiles, retired keys). Without those fields
the items move to S; that share was not counted.

## Reading for the process

- **Prevention beats detection.** Restated facts are the raw material for RF, LD and ND (74 groups).
  Draft steps 1-2 (glossary, named constants, no restating) remove them at write time, before any
  decision can make them stale.
- **Most of the semantic work is targeted, not full.** 52 of 211 need a reviewer, and in every one the
  conflicting sentence shares a concern, key, link or decision with the other side, so an impact-set
  review sees both (my judgement, item by item, not machine-checked). Only 3 (1.4 %) need a full audit: C091, which shares no concern, and the two
  external-fact items. The full audit is a backstop and a trend measure, not the main net.
- **Decision intake is the single largest failure.** One unpropagated decision produced 33 raw reports.
  Draft step 4 has to start by recording the decision in `docs/decisions.md` with an ID: decisions held
  only in task briefs (D1-D5) cannot be traced.
- **Type invariants and the walking skeleton carry the boundary rules** (C in 11 contradictions, plus
  10 linked restatements). Redaction placement belongs in types, not prose.
