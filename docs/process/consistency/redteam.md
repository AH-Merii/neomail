# Red team: the draft consistency process

Target: the 10-item draft process (manifest glossary and constants, fact ownership, decision
traceability, decision-change workflow, staging and atomic promotion, CI gates, routine semantic
audit, ICRs, contract-first fan-out, review-until-dry), on top of retro rules R1–R7.

Method: every contradiction in `target/workflows/arch5/extract/*.json` was dumped and
classified, and the cited lines were spot-checked against `docs/`. Each attack asks one question:
would the draft, as written, have stopped this contradiction, and would it survive many AI agents
working in parallel?

Corpus numbers (a Python pass over the 14 extract files; it parses concatenated JSON documents):

- 256 contradiction records, and 183 distinct (file pair, line) locations. So about 30 % are
  repeats of the same finding by different extractors.
- 28 records mention `5,000`, `5000` or `ask_above`. All of them come from one stale owner
  decision.
- 34 records mention a `§` reference or a cross-ref, 34 mention a dependency, the DAG or a
  cross-crate call, 19 mention two owners, and 15 mention egress or `reqwest`.
- Concern roles: 257 `defines`, 187 `restates`, 153 `references`.
- 24 config keys have more than one distinct `defines` default. For example, `storage.encrypt`
  appears as `true`, `on`, `true (security key)` and `true (false is the escape hatch)`.

Likelihood and impact use high / medium / low. "Fix" is the cheapest change that closes the gap.

## Summary

| # | Attack | Likelihood | Impact | Fix cost |
|---|---|---|---|---|
| A1 | Lint knows only the new value, so stale old values pass | high | high | low |
| A2 | Number and value restatement lint is evaded by formatting, and noisy | high | medium | low |
| A3 | Semantic contradictions with no shared name or number | high | high | medium |
| A4 | Data-flow and process-boundary errors pass the DAG lint | medium | high | low |
| A5 | Rules kept as prose tables (profiles, keymaps, CLI verbs) | high | high | medium |
| A6 | Decisions and facts that live outside the repo | high | medium | low |
| A7 | Decision-ID collisions and ambiguity | medium | medium | low |
| A8 | A citation does not prove compliance | high | high | low |
| A9 | `§` numbers rot when sections are renumbered | high | low | low |
| A10 | Hand-written config examples bypass the manifest | high | medium | low |
| B1 | Agents weaken a failing lint | high | high | low |
| B2 | Generated blocks hand-edited, or markers deleted | medium | medium | low |
| B3 | Staging copies diverge from main | medium | high | low (drop the mechanism) |
| B4 | Decision-change runs race with build waves | high | high | medium |
| B5 | Audits drown in duplicates and re-reported findings | high | medium | low |
| B6 | Routine audits cost too much and add latency | high | medium | low |
| B7 | ICRs pile up at the stewards, and deviations go silent | high | high | medium |
| B8 | Behaviour drifts from code once implementation starts | high | high | low |
| B9 | Contract-first fan-out stalls, or names get invented locally | high | medium | low |
| B10 | Review-until-dry never runs dry | medium | medium | low |
| B11 | `manifest.toml` becomes a merge-conflict hotspot | high | medium | low |
| C1–C4 | Draft items that are not worth their cost as written | — | — | — |

---

## A. Contradictions that would still get through

### A1. The lint knows the new value, but the stale text holds the old one

- **Scenario.** The owner changes a threshold. Item 2's lint "flags a named constant's value
  appearing outside its owner", but it only knows the current value (1,000). The stale text
  says 5,000, so nothing matches. A removed key (`classify.ask_above`) is simply not in the
  manifest any more. Item 1's "unknown keys fail" catches that only inside parsed key contexts,
  and never in prose like "asks above 5,000".
- **Evidence.**
  - `docs/decisions.md:76-79` replaces P7 with "capped at the newest 1,000".
  - The stale text survives at `13-config.md:116` (`ask_above = 5000`), `06-classification.md:181`,
    `05-auth-and-secrets.md:182`, `03-domain-and-storage.md:137`, `18-risks-and-cut-line.md:42`
    and `plan.md:1013`.
  - 28 of 256 contradiction records come from this one root cause (command above).
- **Likelihood / impact.** High / high. This is exactly the failure that produced the largest
  cluster.
- **Fix.** Every decision record carries `retires = { keys = [...], values = [...], phrases = [...] }`.
  The manifest keeps these as tombstones, and the lint fails on any tombstoned key, value or
  phrase anywhere except `decisions.md` history. The decision-change workflow (item 4) must fill
  `retires` before it computes the impact set. The tombstone grep then becomes the impact set.

### A2. The restatement lint is evaded by formatting, and is noisy on common numbers

- **Scenario.** Exact-value matching misses `5,000` / `5000` / `5 k`, `25 MB` / `25 MiB`,
  `365 days` / `a year`, and `true` / `on`. It also fires on every unrelated `20 ms`, `1,000` or
  `25`. Agents then learn to spell numbers as words to silence it (see B1).
- **Evidence.**
  - `storage.encrypt` appears with 8 textual default variants across the extract files (command
    above).
  - Gmail's limit appears as "35 MiB" (`04-providers-and-sync.md:149`) and as "25 MB"
    (`05-auth-and-secrets.md:210`).
  - Lua budgets: 20 ms (`12-extensions-agents-mcp.md:12-13`) against 50 ms
    (`01-process-and-crates.md:83`).
- **Likelihood / impact.** High / medium.
- **Fix.** Keep restatement, but make it generated. Write
  `<!--c:classify.first_run.cap-->1,000<!--/c-->` and let `gen` fill the value from the
  manifest, so `gen --check` fails on drift. Readers still see the number inline, which matters
  for agents that read one file. The bare-number lint then only warns, and only on
  unit-bearing numbers (`\d[\d,.]*\s?(ms|s|MiB|MB|GiB|days|messages|%)`) outside `<!--c-->` markers.

### A3. Semantic contradictions with no shared name or number

- **Scenario.** Two statements conflict in meaning: ordering, lifecycle, or what a process can
  see. No glossary name or constant connects them. Only item 7 can catch these. But item 7
  extracts "changed files and their linked neighbours", and the two halves often sit in files
  that do not link to each other.
- **Evidence.**
  - Onboarding is "persisted in `kv` so a crash resumes it" (`05-auth-and-secrets.md:166`), but
    `kv` lives in `index.db`, which is created in state 2 (`03-domain-and-storage.md:403`, auth.json).
  - `JEV_API_KEY` "pre-fills state 6" (`05-auth-and-secrets.md:185`), but the daemon is spawned
    with `env_clear()` and "nothing inherits it" (`05-auth-and-secrets.md:57-59`).
  - `SearchIndex` is "swappable for tantivy" (`03-domain-and-storage.md:482`), but FTS writes go
    through `Store::index_text` (`03-domain-and-storage.md:469`).
  - One Egress conformance suite cannot hold for both the loopback-only fake and the real client
    (`16-testing-ci-release.md:35` against `plan.md:289`, quality.json).
  - The env config layer never reaches an autostarted daemon (`13-config.md:10` against
    `01-process-and-crates.md:26`).
- **Likelihood / impact.** High / high. These become runtime bugs, not doc typos.
- **Fix.**
  1. Define item 7's neighbour set as files that share any manifest concern, glossary term or
     port with the changed file. This is an index lookup, not Markdown links.
  2. Persist the extract's `concerns` (defines / restates / references) as that index.
  3. Add two standing audit lenses: **lifecycle / ordering** (what exists when; crash and restart
     paths) and **process visibility** (what each process can see: env, files, sockets, keys).

### A4. Data-flow and process-boundary errors pass the DAG lint

- **Scenario.** "X uses Y" checking is regex over prose. Text like "applies the classifier's
  redaction to tool results" names no crate, so no edge is checked. And the real defect is data
  flow, not a dependency edge: mail text crosses the daemon-to-MCP-client boundary before it is
  redacted.
- **Evidence.**
  - `12-extensions-agents-mcp.md:190` ("`standard` applies the classifier's redaction to tool
    results").
  - `plan.md:471` puts `mcp.redaction` in `nm-mcp`, a daemon client that may depend only on
    `nm-types`, `nm-config` and `nm-api` (`01-process-and-crates.md:183`).
  - `14-security.md:79` lists redaction only "for jev".
  - 12 contradiction records concern MCP redaction (command above).
- **Likelihood / impact.** Medium / high. This is a privacy leak to agents.
- **Fix.**
  1. Add a `[[boundary]]` table to the manifest. Each entry gives the boundary (daemon→jev,
     daemon→MCP client, daemon→logs, daemon→desktop notification, daemon→CLI JSON), the required
     payload type (`RedactedText`, `SafeText`, IDs only), and the crate and section that own the
     conversion.
  2. The lint requires each boundary's owner section to name that type.
  3. R4 puts the type in the API signatures, so the compiler enforces it later.

### A5. Rules kept as prose tables: the glossary knows the names, not the contents

- **Scenario.** Item 1's glossary registers that the profile `agent` exists. It does not record
  what `agent` may do. So two sections can hold the name correctly and still give it different
  rules. The same applies to approval policy, CLI verbs, default keymaps and the `NMAIL_*` env
  vars.
- **Evidence.**
  - Agent profile: "= `draft` with no exec and no config writes" (`07-actions-api-cli.md:278`)
    against "no send, delete, secrets, exec or config writes without approval"
    (`12-extensions-agents-mcp.md:195`). `agents.default_profile = "agent"` has no definition
    (`13-config.md:150`, actions.json).
  - `mail_trash` needs approval (`12-extensions-agents-mcp.md:160`), but `ConfirmPolicy::Approval`
    covers only send and delete (`07-actions-api-cli.md:120`).
  - The CLI is "generated from the catalogue" (`07-actions-api-cli.md:156`), yet its verbs differ
    from it: `keymap set/unset` against `bind/unbind/reset`, and `task add/remove` against
    `create/delete` (`07-actions-api-cli.md:329` against `:241`).
  - Key collisions: in the picker, `<C-a>` cycles accounts (`10-search-and-pickers.md:110`), but
    the Lua example binds it to `message.archive` (`10-search-and-pickers.md:223`). `<C-p>` and
    `<Tab>` collide with the shared completion engine (`10-search-and-pickers.md:116-118` against
    09, tui.json).
  - The `NMAIL_*` list at `13-config.md:14` misses about 9 vars that are used elsewhere
    (config-platforms.json).
- **Likelihood / impact.** High / high. Permission rules are security boundaries.
- **Fix.** Keep rule tables as data, not prose:
  - `data/profiles.toml`: profile → capabilities → approval.
  - The action catalogue: action → capability, confirm policy, CLI verb, binder crate.
  - `data/keymap.default.toml`: per mode and context.
  - `[env]`: `NMAIL_*` var → reader crate.
  - `gen` renders the doc tables from these files. The lint checks that every CLI verb maps to a
    catalogue action, that no key is bound twice in one context, and that every profile used in
    config is defined. These files are inputs the code needs anyway (07 §6 already generates
    clap from the catalogue), so the cost is moving the work earlier, not new work.

### A6. Decisions and facts that live outside the repo

- **Scenario.** Item 3 tracks only `docs/decisions.md` and plan §10. In practice, binding
  inputs also arrive as:
  - workflow task briefs ("provisional D1/D4/D5");
  - owner chat messages;
  - facts about the machine.

  Agents cite these, but no lint can resolve them, and the next run never sees them.
- **Evidence.**
  - Contradictions cite "task brief D1/D4/D5" (core.json; `18-risks-and-cut-line.md:31`;
    `16-testing-ci-release.md:65`), but `grep -c 'D[1-5]' docs/decisions.md` returns 0.
  - The risk row assumes a "16-core, 46 GB box" (`18-risks-and-cut-line.md:13`), while
    `free -g` reports 15 GB (core.json, quality.json). `grep -rn nm-heavy docs` returns nothing.
  - This run's owner message ("they can send and receive … brand new emails") matches nothing in
    `decisions.md` (`grep -i 'send and rec\|dummy\|test account'` returns 0).
- **Likelihood / impact.** High / medium.
- **Fix.**
  1. **Record-first rule.** The orchestrator appends any owner ruling to `decisions.md` with an
     ID before any workflow consumes it. Workflow prompts pass decision IDs, never paraphrases.
  2. The lint fails on any decision-like token (`D\d+`, `P\d+`, `DEC-…`) that does not resolve.
  3. Machine facts go in a manifest `[env]` table with a `probe` command (`free -g`, `nproc`).
     Steward phases re-run the probes and flag drift.

### A7. Decision-ID collisions and ambiguity

- **Scenario.** IDs are short and unscoped. Parallel agents mint IDs at the same moment, and the
  same short ID ends up naming different things.
- **Evidence.**
  - `P7` is "Provider registries" in the patterns table (`02-building-blocks.md:18`) and
    "Classification window" in the plan (`plan.md:1013`).
  - `05-auth-and-secrets.md:182` cites "(P7)" without saying which.
  - Provisional `P45` is cited from prose (`12-extensions-agents-mcp.md:190`, `plan.md:1051`).
- **Likelihood / impact.** Medium / medium.
- **Fix.** Use typed prefixes: `DEC-` (owner), `PROV-` (provisional), `PAT-` (pattern), `WP-`,
  `ICR-`. IDs are date plus slug (`DEC-2026-10-11-first-run`), so concurrent agents cannot
  collide and IDs are never reused. The lint checks uniqueness across all registries.

### A8. A citation does not prove compliance

- **Scenario.** Item 3's lint passes when each live decision is cited somewhere and nothing
  cites a superseded one. Two things slip through:
  - A section can cite the live ID and still state stale content.
  - A section can restate the decision's subject without citing anything.
- **Evidence.**
  - `06-classification.md:181` and `03-domain-and-storage.md:137` restate the first-run
    behaviour and cite nothing.
  - `13-config.md:116` sets the key with only a comment.
  - Only `05-auth-and-secrets.md:182` cites "(P7)". So the superseded-citation check would catch
    one of the 6 stale locations.
- **Likelihood / impact.** High / high.
- **Fix.**
  1. Each decision lists `affects = [concern ids, key ids, constant ids]` in the manifest.
  2. The lint requires that every owner section of those concerns cites the decision.
  3. A "decision compliance" audit lens reads each citing section against the decision text.
     This is cheap because the set is bounded by the citations.
  4. Combined with A1 tombstones, uncited restatements of retired values fail mechanically.

### A9. `§` numbers rot when sections are renumbered

- **Scenario.** The lint can check that `07 §4` exists, but not that it is the right section.
  Adding `19-motion.md` and moving motion out of 08 §7 will renumber sections, and every numeric
  reference silently points at the wrong text.
- **Evidence.**
  - 34 records mention `§` or a cross-ref.
  - `03-domain-and-storage.md:368` cites "07 §4" for events, which are in §5.
  - `02-building-blocks.md:581` cites "08 §9" for the governor, which is 08 §7.3
    (`08-tui.md:286`, while `08-tui.md:346` is §9 Learnability).
  - `03-domain-and-storage.md:137` cites "06 §8", which should be §7.
  - `16-testing-ci-release.md:18` cites "14 §10", which does not list approval binding.
- **Likelihood / impact.** High / low. Each error is cheap alone, but they mislead agents.
- **Fix.** References use stable anchors (`07-actions-api-cli.md#events`). `gen` may append the
  current `§` number as display text. The lint resolves the anchors and rejects bare numeric `§`
  references.

### A10. Hand-written config examples bypass the manifest

- **Scenario.** `13-config.md` claims to show "every section; defaults shown", but it is
  hand-written, so keys defined in other files never reach it. Item 1 generates *tables*, but
  says nothing about fenced TOML examples.
- **Evidence.**
  - The claim is at `13-config.md:24`.
  - Keys missing from it: `events.include_subjects` (`07-actions-api-cli.md:185`),
    `render.show_invisible` (`03-domain-and-storage.md:51`), `keymap.hint_delay` and
    `keymap.timeout` (`08-tui.md:226`), and `[[tasks]]` `max_targets` (`07-actions-api-cli.md:376`).
  - One setting has two names: `ui.hints` / `keymap.hints` (`13-config.md:53` against `08-tui.md:137`).
- **Likelihood / impact.** High / medium.
- **Fix.**
  1. Generate the full example in `13-config.md` from the manifest.
  2. The lint parses every other fenced `toml` block in `docs/` and fails on unknown keys or
     non-default values unless the block is tagged `example-override`.

---

## B. Where the process breaks down with parallel AI agents

### B1. Agents weaken a failing lint

- **Scenario.** A WP agent hits a red archlint and is scored on "CI green". The cheapest way to
  green is to weaken the check, not fix the docs:
  - add an allowlist entry or an `archlint: ignore` comment;
  - register a second owner;
  - loosen a rule in `tools/archlint`;
  - spell the number as words.

  With auto-merge on green CI, nobody sees it happen.
- **Evidence.** Merge is `gh pr merge <pr> --auto --squash` once CI is green
  (`16-testing-ci-release.md:65`; `plan.md:53`). `decisions.md:206` says changes are "merged to
  `main` once CI passes". The independent review step the extract cites as provisional D5 is not
  in the repo (A6).
- **Likelihood / impact.** High / high.
- **Fix.**
  1. Make these paths protected: `docs/architecture/manifest*.toml` ownership fields,
     `tools/archlint/**`, `xtask/**`, the suppression list and the CI workflow files. A CI job
     fails when a non-`steward/*` branch touches them.
  2. Every suppression carries an ICR or decision ID and an expiry date.
  3. CI prints the suppression count and fails if it grows without a `steward/*` branch.
  4. Mechanical, about 30 lines in archlint.

### B2. Generated blocks hand-edited, or markers deleted

- **Scenario.** `gen --check` catches edits *inside* markers. It does not catch:
  - an agent deleting the markers and keeping a hand-edited table;
  - a new hand-written table next to the generated one;
  - a commit helper "fixing" the generated block by hand.
- **Evidence.** The draft has no list of the generated blocks that must exist (items 1 and 6).
- **Likelihood / impact.** Medium / medium.
- **Fix.**
  1. The manifest lists every generated block (file and block id), and the lint fails if one is
     missing.
  2. The commit helper refuses diffs inside marker ranges unless `gen` produced them (it re-runs
     `gen` and compares).
  3. Each generated block gets a one-line "edit manifest.toml, not this" banner.

### B3. Staging copies diverge from main

- **Scenario.** A decision-change run copies the docs at base B. While it re-derives them, other
  PRs merge to main. "Atomic promotion" of the staging copy then silently reverts those merges,
  or promotion fails half-way with no clear rebase story.
- **Evidence.** Item 5 says "staging copy". The repo already runs on GitButler branches and PRs
  (`plan.md:53`), which give atomicity and three-way merge for free.
- **Likelihood / impact.** Medium / high.
- **Fix.** Drop the bespoke staging copy (see C1).
  1. The run works on its own GitButler branch and commits checkpoints there.
  2. Promotion is a PR merge, so CI re-runs archlint on the merge result.
  3. The PR records base B, and CI requires a rebase if any impact-set file changed on main
     since B.

### B4. Decision-change runs race with build waves

- **Scenario.** A WP agent is implementing the first-run flow from the old contract
  (`ask_above`) while a decision-change run retires it. The WP merges green because its code
  matches the manifest *it* started from. The rewrite merges green as well. Main now holds code
  for the old decision and docs for the new one.
- **Evidence.**
  - The first-run decision arrived after the design (`decisions.md:76`, "2026-10-11; replaces
    provisional P7").
  - Waves run several agents in parallel, with stewards between them (`plan.md:10-15`).
- **Likelihood / impact.** High / high.
- **Fix.**
  1. Each manifest concern carries `rev`, and a decision change bumps the `rev` of every concern
     in its `affects` (A8).
  2. A WP branch records the concern revs it started from as a PR trailer. CI fails the merge
     when a touched crate's concern `rev` has moved on, which forces a re-read.
  3. Non-urgent decision changes apply only at steward phases. The owner can mark one urgent.

### B5. Audits drown in duplicates and re-reported findings

- **Scenario.** Every audit re-extracts from scratch, so known, accepted or already-fixed items
  come back. Fixers chase the same root cause in N places, and owners stop reading. On the other
  side, refute-by-default verifiers drop real cross-file findings because each half looks fine
  alone.
- **Evidence.**
  - 256 records collapse to 183 distinct locations.
  - 28 records trace to one decision (A1), and 12 to MCP redaction (A4).
- **Likelihood / impact.** High / medium.
- **Fix.**
  1. Fingerprint each finding as (concern or key id, sorted locations), and cluster by root cause.
  2. Keep a findings ledger (`target/consistency/ledger.jsonl`, state: open / fixed / accepted
     with reason and decision ID). Each run reports only new fingerprints.
  3. Track root causes, not raw records, as the trend.
  4. A verifier gets both locations and must quote both before refuting.

### B6. Routine audits cost too much and add latency

- **Scenario.** A full extraction took 14 extractor outputs over 6,081 doc lines (`wc -l` over
  `docs/architecture/*.md`, `plan.md` and `decisions.md`), and code will multiply that. Item 7
  runs it weekly, at every steward phase and after every decision change. During build waves
  that is several runs a day, mostly over unchanged text.
- **Evidence.** `ls target/workflows/arch5/extract/` shows 14 JSON outputs plus part files for
  one pass.
- **Likelihood / impact.** High / medium.
- **Fix.**
  1. Trigger on diffs: no docs or contract change means no audit. Drop the calendar run (C2).
  2. Cache extraction per section content hash, and re-extract only changed sections plus their
     concern neighbours (A3).
  3. The mechanical lint runs on every commit, because it is cheap. The LLM audit runs at steward
     phases and milestones only, with a fixed lens count.

### B7. ICRs pile up at the stewards, and deviations go silent

- **Scenario.** Stewards apply every ICR, so WP agents block waiting. Under time pressure they
  deviate and plan to "file the ICR later". Nothing mechanical links a deviation to an ICR.
- **Evidence.** Item 8 sends all ICRs through stewards. The plan has one steward agent per
  steward phase (`plan.md:15`, "S0 | WP-00S | steward … | 1").
- **Likelihood / impact.** High / high.
- **Fix.** Split ICRs by blast radius:
  - **Additive** (a new key, type or action owned by the WP's own crate): the WP edits the
    manifest fragment it owns and the owner section in the same PR, and the lint enforces it.
  - **Breaking** (a port signature, a cross-crate edge, anything in a decision's `affects`):
    steward only, and the WP is blocked.

  CI makes silent deviation impossible: code-generated reference docs and `check-dag` disagree
  with the manifest until the ICR lands. The steward phase starts by draining the ICR queue
  oldest first.

### B8. Behaviour drifts from code once implementation starts

- **Scenario.** `check-dag` and `gen --check` cover structure: crates, edges, keys and
  signatures. Behaviour such as "classify the newest 1,000 without asking", "agent profile
  cannot send" or "MCP output is redacted" has no mechanical link to code. Drift shows up only
  in an audit, if ever.
- **Evidence.** These rules live only in prose (`decisions.md:76-79`;
  `07-actions-api-cli.md:278`; `12-extensions-agents-mcp.md:190`).
- **Likelihood / impact.** High / high.
- **Fix.**
  1. Each live decision and prose invariant in the manifest is marked `test = "required"`, and
     the tests that check it carry its ID in the name or doc attribute
     (`dec_2026_10_11_first_run_cap`).
  2. The lint lists required IDs whose owning crate exists but has no referencing test. This is a
     grep over `crates/**/tests`.
  3. A decision change reports the tests that cite it, which become part of the impact set.

### B9. Contract-first fan-out stalls, or names get invented locally

- **Scenario.** Writers find new shared facts mid-task. "Goes through the contract owner" means a
  blocking round-trip, so agents invent a local name instead.
- **Evidence.**
  - The daemon picker port is `PickerSource` (`README.md:70`) and `DaemonPickerSource`
    (`02-building-blocks.md:255`, `plan.md:262`).
  - `BudgetBroker` is missing from the nm-ports list (`README.md:70` against `plan.md:263`).
- **Likelihood / impact.** High / medium.
- **Fix.** Writers never block. A new name goes into their own `manifest.d/<writer>.toml` as
  `status = "proposed"`. The lint accepts a proposed name only in the proposing writer's file.
  The synthesiser must resolve every proposal before promotion: accept, rename or merge. Two
  proposals for the same port or crate slot are flagged mechanically.

### B10. Review-until-dry never runs dry

- **Scenario.** LLM reviewers always find something. With no stopping rule, R5 and item 10
  either loop for a long time or stop arbitrarily, and each round re-reports earlier nits.
- **Evidence.** Item 10 and R5 define no termination condition.
- **Likelihood / impact.** Medium / medium.
- **Fix.**
  1. "Dry" means a round with zero new, verified findings of medium or higher severity, checked
     against the ledger fingerprints (B5).
  2. Cap the loop at 3 rounds, then escalate what is left to the steward or owner as a list.

### B11. `manifest.toml` becomes a merge-conflict hotspot

- **Scenario.** Every WP, writer and ICR touches one TOML file, so parallel branches conflict
  constantly. GitButler then holds conflicted commits, and agents "resolve" by keeping their own
  side.
- **Evidence.** R3 makes one file the single source of truth for crates, deps, ports, concerns,
  keys and invariants.
- **Likelihood / impact.** High / medium.
- **Fix.** Single source of truth by *meaning*, not by file. Split it into
  `docs/architecture/manifest.d/<area>.toml`, one per owner area. The lint merges the fragments
  and rejects duplicate ids, so a conflict becomes a lint error instead of a text merge.

---

## C. Draft items not worth their cost as written

### C1. Item 5's bespoke staging copy and atomic promotion: drop it

- Git branches and PR merges already give atomic promotion and crash safety.
- A separate copy adds the divergence risk in B3.
- The original failure, an agent dying mid-rewrite, only hurt because the work was not isolated
  on a branch.
- **Keep instead:** "every multi-file rewrite runs on its own GitButler branch, commits
  checkpoints there, and lands by PR".

### C2. Item 7's calendar weekly audit: drop it

- A weekly run with no docs diff re-reads unchanged text.
- Diff-triggered audits (steward phase, decision change, milestone) cover every real change.
- See B6.

### C3. Item 2's "never restated" rule with a bare-value lint: replace it

- An absolute ban on restating hurts agents that read one file.
- Exact-value matching is both evadable and noisy (A2).
- **Replace with** generated inline constants (`<!--c:id-->`), tombstones for retired values
  (A1), and a warn-only lint for unit-bearing numbers.

### C4. Item 1's "unknown names fail the lint": narrow it

- Applied to every backticked identifier, it fires on `Vec`, `reqwest::Client`, `tokio` and
  every third-party type.
- Agents will then add these names to the glossary or suppress the rule (B1).
- **Narrow it** to namespaces we own:
  - `nm-*` crates;
  - `NMAIL_*` vars;
  - dotted config keys under manifest tables;
  - `ns.verb` action names;
  - port and owned-type names in the manifest;
  - decision, WP and ICR IDs.

Items 3, 4, 6, 8, 9 and 10 are worth keeping with the fixes above. Items 3 and 4 are the
cheapest high-value pieces, once A1, A7 and A8 are fixed.

---

## D. Top 10 gaps, ranked by likelihood × impact ÷ fix cost

1. **A1** Stale old values pass. Fix: `retires` tombstones on each decision, and the lint fails
   on retired keys, values and phrases.
2. **B1** Agents weaken the lint, and green CI auto-merges. Fix: protected paths, suppressions
   that need an ID and an expiry, and a CI check on the suppression count.
3. **A8** A citation is not compliance. Fix: decisions list `affects`, every owner section of
   those concerns must cite them, and a compliance lens reviews the citers.
4. **B4** Decision changes race WP merges. Fix: per-concern `rev`, a PR trailer with the revs
   the branch started from, and CI blocks stale merges.
5. **A5** Rules live in prose tables. Fix: profiles, catalogue (with CLI verbs), keymaps and env
   vars as data, generated tables, and collision and mapping checks.
6. **A4** Boundary data flow is invisible to the DAG lint. Fix: a manifest `[[boundary]]` table
   with the required payload type, enforced by R4 types.
7. **B8** Behaviour drifts from code. Fix: `test = "required"` on decisions and invariants, with
   a grep for tests that carry the ID.
8. **A3** Semantic contradictions without shared names. Fix: concern-index neighbours instead of
   links, and lifecycle and process-visibility lenses.
9. **A6** Decisions outside the repo. Fix: the record-first rule, unresolved decision tokens fail
   the lint, and `[env]` facts with probes.
10. **B7 + B11** ICR and manifest bottlenecks. Fix: additive ICRs go in the WP's own PR, and the
    manifest is split into `manifest.d/` fragments that the lint merges.
