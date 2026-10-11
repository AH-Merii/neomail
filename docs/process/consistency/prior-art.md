# Prior art: keeping a large multi-agent design consistent with itself and with code

Researcher: prior-art agent, 2026-10-11. Scope: practices from regulated industries, mature open-source
processes, docs-as-code tooling, architecture testing, lightweight formal methods and 2024-2026
multi-agent LLM reports, judged for a solo owner running AI agents on neomail.
Every claim carries a source; estimates of cost for neomail are marked "estimate".

## 0. What actually went wrong locally (the failure classes to design against)

Source: `target/workflows/arch5/extract/*.json`, parsed leniently with a Python `raw_decode` loop.

- 256 contradiction records across 14 extract files, 233 unique `(where, conflicts_with)` pairs
  (command output: `actions.json 23 ... tui.json 16`, `total 256`, `unique where/conflicts pairs 233`).
- Keyword heuristic over the descriptions (overlapping, first-match bucket; a rough guide, not a
  classification): dependency/boundary 65, name/key 59, decision-related 30, number/threshold 17,
  rule/permission 13, other 49. Any-match counts: dep/boundary 96, name/key 68, rule/permission 37,
  decision 30, number 23.
- Example record (extract/actions.json, first item): "Dispatcher lives in nm-api and must write audit
  records ... but nm-api cannot name nm-ports types where AuditSink and Clock live; plan.md:249-250
  therefore adds separate audit/undo sink traits to nm-api, giving two audit traits",
  where `docs/architecture/07-actions-api-cli.md:64`, conflicts with `docs/architecture/README.md:70`.

So most defects are structural (who may name what, which crate owns which type, which key is called
what). Those are cheap to catch mechanically. The minority (rules, permissions, process-boundary
placement) need semantic review. The process should spend deterministic checks on the first group and
agent review on the second.

## 1. Verdict table (mapped to draft items 1-10)

| # | Practice (source family) | Verdict | Draft item(s) |
|---|---|---|---|
| P1 | Suspect links via content fingerprints (Doorstop) / revisioned IDs (OpenFastTrace) | ADOPT (in archlint) | 3, 4, 6, 7 |
| P2 | Stable IDs on every normative unit, anchors independent of section numbers (Ferrocene FLS, StrictDoc) | ADOPT | 1, 3 |
| P3 | Bidirectional traceability + orphan detection (DO-178C, ISO 26262, IEC 62304) | ADAPT (keys, constants, decisions, ports only) | 1, 2, 3 |
| P4 | Change impact analysis before a change (AC 20-115D, IEC 62304 cl. 6-8) | ADOPT (already item 4) | 4 |
| P5 | Dedicated RM tools: Sphinx-needs, StrictDoc, Doorstop, OpenFastTrace | SKIP the tools, copy the mechanisms | 1, 3 |
| P6 | Decision records are append-only history; status + Superseded-By/Replaces both ways (Nygard ADR, PEP 1, Rust RFCs, KEPs) | ADOPT | 3 |
| P7 | Machine-validated decision metadata in presubmit (kep.yaml + kepval) | ADOPT | 3, 6 |
| P8 | Ratchet/freeze baseline: existing violations recorded, only new ones fail (ArchUnit FreezingArchRule; KEP PRR rollout) | ADOPT | 6, 7 |
| P9 | One model, many generated views (Structurizr; Kubernetes API reference from OpenAPI) | ADOPT (already R3/item 1) | 1, 2 |
| P10 | Codegen-as-test with "do not edit" header (matklad, rust-analyzer `cargo xtask codegen`) | ADOPT | 1, 6 |
| P11 | Compile code snippets in docs (rustdoc doctests, mdbook test, include_str harness) | ADAPT (after walking skeleton) | 8 |
| P12 | Prose linters (Vale) | SKIP (archlint already owns the checks) | 2 |
| P13 | Link/anchor checker (lychee `--offline --include-fragments`) | ADAPT (weekly external-URL run only) | 6, 7 |
| P14 | Fitness functions: triggered vs temporal (Ford/Parsons/Kua) | ADOPT as the vocabulary for gates vs audits | 6, 7 |
| P15 | Dependency rules: cargo-deny `bans.deny` + `wrappers`; xtask allowed-edge table | ADOPT | 6 |
| P16 | Lightweight formal methods (Alloy, TLA+, P) | SKIP for docs; revisit for one protocol | none now |
| P17 | Property checks over a facts database | ADOPT (archlint queries over manifest + extract) | 7 |
| P18 | Single-threaded writing / shared full context (Cognition) and explicit task contracts (Anthropic) | ADOPT | 9 |
| P19 | Multi-level verification, not one pass (MAST taxonomy) | ADOPT (already R5/item 10) | 7, 10 |
| P20 | Blackboard shared state for agents | ADAPT (read-all, single-writer manifest) | 9 |
| P21 | Read-only cross-artifact analysis before implementation; constitution injected every step (GitHub Spec Kit) | ADOPT | 7, 9 |
| P22 | LLM contradiction detection: high precision, low recall | ADAPT (structured extraction + seeded canaries to measure recall) | 7 |
| P23 | Freshness dates + owners on docs (Google g3doc) | SKIP dates; keep owners; use P1 instead | 2, 7 |
| P24 | Staging a multi-file change | ADAPT: the branch is the staging copy, the merge is the atomic promotion | 5 |

Sections 2-9 give the evidence and cost behind each row. Section 10 lists the concrete edits this
implies for the draft process.

## 2. Requirements traceability (regulated industries and their tools)

### P3/P4. Regulated practice: bidirectional trace + change impact analysis

What it is. DO-178C requires trace data between system requirements, high-level requirements,
low-level requirements, source code and tests, in both directions; it is a named lifecycle data item
(Trace Data, section 11.21 in the FAA DO-178B/C Differences Tool,
https://www.faa.gov/sites/faa.gov/files/aircraft/air_cert/design_approvals/air_software/differences_tool.pdf).
For modified software the FAA/EASA guidance asks the applicant to "conduct a software change impact
analysis (CIA) to determine the extent of the modifications, the impact of those modifications, and
what verification is required" (AC 20-115D, https://www.faa.gov/documentLibrary/media/Advisory_Circular/AC_20-115D.pdf).
IEC 62304 requires the development plan to address traceability between system requirements,
software requirements, system tests and risk controls, and puts change control in clause 8.2
(consolidated Ed. 1.1 text,
https://po-files.ks3-cn-beijing.ksyun.com/5f445d6af346fb3c83c3de37_6a3c46d361fc.pdf; clause list
also summarised at https://en.wikipedia.org/wiki/IEC_62304). ISO 26262-8 clause 6 asks for safety
requirements that are unambiguous, atomic, internally and externally consistent, with no duplication
(secondary sources only, the standard is paywalled:
https://www.diva-portal.org/smash/get/diva2:470633/FULLTEXT01.pdf and
https://ldra.com/wp-content/uploads/ldra/Implementing_ISO_26262_second_edition_white_paper.pdf;
treat the exact clause wording as unverified).

Evidence it works. The backward direction is what finds orphans: an automated check in a DO-178C UAV
study flags "uncovered system requirements and orphan HLRs ... which must be resolved before
baselining" (https://www.sciencedirect.com/science/article/pii/S0950584926000571, via search summary).
"No duplication" in ISO 26262 is exactly draft item 2.

Cost. Full matrices (requirement -> design -> code -> test) are the expensive part of certification
programmes; nothing in neomail needs certification evidence.

Verdict. ADAPT. Keep the two cheap ideas: (a) bidirectional checks on the things the manifest already
names: every config key, named constant, port, concern and live decision must have exactly one
defining owner and at least one consumer (section, crate or test), and every reference must resolve;
an entity with no consumer is an orphan, a reference with no definition is a dangling link.
(b) Impact analysis is mandatory before an owner decision is applied (item 4 already does this).
Skip requirement-to-test matrices for prose until code exists; at that point the conformance suites
of R1 are the "test" end of the trace for ports.

### P1. Suspect links (Doorstop) and revisioned IDs (OpenFastTrace)

What it is. Doorstop stores each item as YAML in git. "The value of the reviewed attribute indicates
the fingerprint of the item when it was last reviewed"; "a link consists of two parts, the parent item
UID and the fingerprint of the parent when it last reviewed"; when the parent changes, the stored
fingerprint no longer matches and the link is reported as suspect until someone runs `doorstop clear`
(https://doorstop.readthedocs.io/en/latest/reference/item.html; https://github.com/doorstop-dev/doorstop).
OpenFastTrace (OFT) puts a revision in every ID (`req~example~1`) and reports a link as "Outdated"
when the target has a higher revision, "Orphaned" when the target is missing and "Ambiguous" when
the ID is duplicated (https://github.com/itsallcode/openfasttrace/blob/main/doc/spec/design.md;
user guide https://github.com/itsallcode/openfasttrace/blob/develop/doc/user_guide.md).

Evidence. This mechanism is the direct answer to neomail's main failure: owner decisions "patched into
some sections but not all". With suspect links, changing the owner text of a decision or constant
automatically marks every citing section as needing review; nothing depends on an agent remembering
to grep. Doorstop's own docs warn that a tag move can flag "all breaking changes ... with reported
suspect links" (https://codethinklabs.gitlab.io/trustable/trustable/trudag/using-doorstop.html),
i.e. the noise cost is real but bounded.

Cost. Estimate: ~150 lines of stdlib Python in archlint (hash the normalised text of each owner
unit, store `{citing_unit: {cited_id: hash}}` in a committed lock file, report mismatches, and an
`archlint review <unit>` command that rewrites the hash after a reviewer accepts it). Typo fixes in
an owner section create review noise; OFT's manual revision number avoids that noise but relies on
the author remembering to bump it, which is the discipline that already failed here.

Verdict. ADOPT, fingerprint form (automatic) rather than OFT revision form (manual). CI rule: no
suspect links on the default branch; a reviewer clears them only by editing the citing section or by
an explicit `review` with a one-line justification in the commit. This turns draft item 4's "grep of
affected keys, names and numbers" into a computed impact set, and gives item 7 its incremental scope.

### P2. Stable IDs independent of numbering (Ferrocene FLS, StrictDoc)

What it is. Every paragraph of the Ferrocene Language Specification carries a permanent ID such as
`:dp:\`fls_8obn3dtzpe5f\``; tests are annotated with those IDs, and a traceability matrix is generated
from test-runner output, reporting e.g. 329 linked + 20 informational sections, 100% completion
(https://github.com/rust-lang/fls; https://public-docs.ferrocene.dev/main/qualification/traceability-matrix.html).
StrictDoc made its link anchors no longer based on section numbers "so they stay stable when
StrictDoc documentation is linked from outside" (release notes v0.7.0,
https://strictdoc.readthedocs.io/en/latest/latest/docs/strictdoc_04_release_notes-TRACE.html).

Evidence. The FLS changelog records spec edits by ID ("Paragraph 6.1.2:34 replaces Paragraph
fls_6g7c1kjrmfnr", https://spec.ferrocene.dev/changelog.html), which is what makes supersession
mechanical. The limitation noted on Hacker News, that 100% coverage means every section has a
linked test, not that the spec is satisfied (https://news.ycombinator.com/item?id=37769285), applies
to us too: traceability proves linkage, not agreement.

Cost. Low: explicit heading anchors (`{#ports.mail-store}`-style or HTML anchors) plus an archlint
rule that § references resolve to an ID, not to a number.

Verdict. ADOPT. neomail docs use "§" cross-references; renumbering after a rewrite silently retargets
them. Require stable anchors on owner sections and resolve every § reference through the anchor.

### P5. Dedicated requirements tools

- Sphinx-needs: needs objects in Sphinx/reST; `needs_warnings` filters plus `-W` turn violations into
  build failures; dead link detection; conditional links (8.0.0) that check the target's status
  (https://sphinx-needs.readthedocs.io/en/latest/configuration.html;
  https://sphinx-needs.readthedocs.io/en/latest/changelog.html). Caveat from its own changelog: an
  ignored link-condition warning drops the link "silently reducing coverage", hence `-W` is required.
- StrictDoc: `.sdoc` format with UIDs and RELATIONS, Markdown relations since v0.19.0 (March 2026),
  Tree-sitter source tracing (https://strictdoc.readthedocs.io/en/stable/stable/docs/strictdoc_01_user_guide.html).
- Doorstop: YAML item per requirement (above). OpenFastTrace: Java, Markdown items, coverage tags in
  code, LSP server (https://github.com/itsallcode/openfasttrace-language-server). OFT 4.10.0 fixed a
  Markdown importer bug that "silently dropped all specification items after a fenced code block"
  (https://github.com/itsallcode/openfasttrace/issues/582): import-format fragility is a real cost.

Verdict. SKIP the tools. Each would force a document-format migration (reST, .sdoc, YAML-per-item or
OFT item syntax) of 19+ architecture files, and neomail already decided on manifest.toml plus a stdlib
lint (R3). Take their mechanisms instead: stable IDs (P2), suspect fingerprints (P1), status-
conditioned links (Sphinx-needs: a section may not cite a decision whose status is superseded, i.e.
draft item 3), warnings-as-errors (`-W`), and coverage reports generated rather than written.

## 3. Decision records and RFC processes (P6, P7)

What they do.
- Nygard ADRs: Title, Status, Context, Decision, Consequences; status "Proposed, Accepted, Deprecated,
  or Superseded by ADR-NNNN" (https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions).
  Fowler: an accepted ADR "should never be reopened or changed", it is superseded instead
  (https://martinfowler.com/bliki/ArchitectureDecisionRecord.html). adr-tools `adr new -s 12 ...`
  updates both records' status links at once (summarised in the same search; tool at
  https://github.com/npryce/adr-tools).
- PEP 1: "In general, PEPs are no longer substantially modified after they have reached the Accepted,
  Final, Rejected or Superseded state"; headers `Replaces:` and `Superseded-By:`; "Formal
  documentation of the expected behavior should be maintained elsewhere, such as the Language
  Reference" (https://peps.python.org/pep-0001/).
- Rust RFCs: "once accepted, RFCs should not be substantially changed ... More substantial changes
  should be new RFCs, with a note added to the original RFC"; and "we cannot expect every merged RFC
  to actually reflect what the end result will be" (https://github.com/rust-lang/rfcs/blob/master/README.md).
- KEPs: machine-readable `kep.yaml` with `status: provisional|implementable|implemented|deferred|
  rejected|withdrawn|replaced` and `replaces:` (https://github.com/kubernetes/enhancements/blob/master/keps/NNNN-kep-template/kep.yaml),
  validated by `kepval` in presubmit (https://github.com/kubernetes/enhancements/blob/master/docs/kepval.md).

Evidence. The split "decision record is history, reference doc is the living spec" is the common
answer across Python, Rust and Kubernetes; it is exactly what neomail lacked (owner decisions applied
as patches into some living sections). Counter-evidence on adoption: a practitioner report says
almost every team tried ADRs but "almost none still maintain them two years later", and that the
surrounding process, not the template, decides survival
(https://hidekazu-konishi.com/entry/architecture_decision_records_templates_and_operations.html; seen via search summary, page not fetched).
That argues for lint-enforced metadata (KEP style) rather than convention.
Kubernetes' own experience of tightening the validator: enabling the PRR check for implemented KEPs
"caused 115 KEPs to fail validation" (https://github.com/kubernetes/enhancements/pull/3007), and the
earlier discussion asked for checks to be "nonblocking as a presubmit so people can slowly fix these
things" (https://github.com/kubernetes/enhancements/issues/617). See P8 for the ratchet answer.

Cost. Low: a `[[decision]]` table in manifest.toml (id, status, supersedes, superseded_by, date,
owner_sections) plus 40-60 lines of archlint (estimate).

Verdict. ADOPT, with three specifics for draft item 3:
1. docs/decisions.md entries become append-only; only `status` and `superseded_by` change after
   acceptance (Nygard/Fowler, PEP 1). The living truth is the owner section in docs/architecture.
2. Both directions must agree (`supersedes` on the new record, `superseded_by` on the old one), checked
   by archlint the way adr-tools writes both at once.
3. Status-conditioned citation (Sphinx-needs conditional links): citing a superseded or rejected ID
   is an error; a live decision with no implementing owner section is an error (orphan, P3).

## 4. Docs-as-code linting and ratchets (P8, P12, P13)

### P12. Prose linters (Vale)
What it is. Vale applies YAML rules (`existence`, `substitution`, and others) to Markdown and other
formats; Datadog runs it as a GitHub Action on PRs and calls its rule repo "the single source of truth
for documentation style" (https://vale.sh/;
https://www.datadoghq.com/blog/engineering/how-we-use-vale-to-improve-our-documentation-editing-process/;
rule examples https://posthog.com/handbook/wizard-and-docs/vale).
Fit for neomail. A `substitution` rule is the textbook tool for "the same key under two names", and an
`existence` rule can reject a constant's value outside its owner. But both lists would have to be
generated from manifest.toml anyway, and archlint already parses the manifest and knows which file
owns each fact, which Vale cannot (its rules are file-agnostic regexes).
Verdict. SKIP. Implement draft item 2's "value outside owner" and item 1's "unknown name" checks in
archlint, generated from the manifest glossary. Revisit Vale only if the owner wants prose-style rules.

### P13. Link and anchor checking (lychee)
What it is. lychee checks links in Markdown/HTML; `--include-fragments` checks `#anchors`, and
`--offline` restricts to local links so "a flaky third-party site can't redden a PR"
(https://lychee.cli.rs/recipes/anchors/; https://lychee.cli.rs/guides/cli/). Broken anchors "do not
generate HTTP errors, which means they're harder to detect" (same recipe page).
Verdict. ADAPT. archlint already owns internal links and § references (it must, to resolve stable IDs
from P2). Use lychee only for a weekly online run over external URLs, because the claims-need-sources
rule fills the docs with URLs that rot. Not a PR gate (network flakiness).

### P8. Ratchet / freeze baselines
What it is. ArchUnit's `FreezingArchRule` records existing violations in a committed store and fails
only on new ones; recommended CI settings are `allowStoreCreation=false` and `allowStoreUpdate=false`
so nothing silently blesses new violations, and fixed violations drop out so "the baseline only
shrinks" (https://deepwiki.com/TNG/ArchUnit/2.3.2-freezing-architecture-rules; CI settings from search
summaries of ArchUnit guides, not fetched). Kubernetes hit the
opposite failure mode: turning a check on for a previously exempt set failed 115 KEPs at once
(https://github.com/kubernetes/enhancements/pull/3007).
Evidence for neomail. ~233 unique contradictions exist today (section 0). Turning archlint on as a
hard gate before they are fixed blocks every PR; turning it on as advisory lets the count grow.
Verdict. ADOPT. `archlint --baseline docs/architecture/archlint-baseline.json`: CI fails on any finding
not in the baseline; the baseline file may only shrink (CI compares counts against the merge base);
regenerating it is a reviewed, owner-visible change. This replaces draft item 7's "contradiction
counts tracked as a trend" with a gate: the trend is enforced, not just plotted.

## 5. Single source and generation (P9, P10, P11)

### P9. One model, many views
What it is. Structurizr: "a single DSL file defines a consistent model from which multiple diagrams
... are generated"; elements are defined once and views reference them
(https://docs.structurizr.com/; https://deepwiki.com/structurizr/structurizr.github.io/3.1-structurizr-dsl).
Kubernetes generates its API reference from the OpenAPI spec, and "if you find bugs in the generated
documentation, you need to fix them upstream"
(https://kubernetes.io/docs/contribute/generate-ref-docs/kubernetes-api/).
Verdict. ADOPT, already R3 and draft item 1. The lesson to add: the generated output must say where to
fix it (Kubernetes) and must never be hand-edited (P10).

### P10. Codegen as a test, with a "do not edit" header
What it is. matklad's pattern: a `#[test]` regenerates a marked region (`// region:sourcegen`) and
fails if the file changed (https://matklad.github.io/2022/03/26/self-modifying-code.html).
rust-analyzer's `xtask/src/codegen.rs` writes files headed "Generated by `cargo xtask codegen {cg}`,
do not edit by hand", and `ensure_file_contents` updates then fails
(https://rust-lang.github.io/rust-analyzer/test_utils/index.html;
https://fossies.org/linux/rustc/src/tools/rust-analyzer/xtask/src/codegen.rs). rust-analyzer also
generates its config documentation from the real `Config` type
(https://lobste.rs/s/lsytjv/self_modifying_code, matklad's comment).
Verdict. ADOPT for draft items 1 and 6. Generated tables live between `<!-- gen:begin NAME -->` /
`<!-- gen:end -->` markers inside the owner docs with the header naming the command; `gen --check`
fails on diff. Once code exists, config-key tables are generated from the config struct (the
rust-analyzer move), so the code and the manifest are compared rather than both being hand-written.

### P11. Compile snippets that live in docs
What it is. rustdoc runs doc examples as tests; `mdbook test` does the same for books; a build.rs that
emits `#[doc = include_str!("x.md")]` modules makes every fenced Rust block a doctest
(https://jackson.dev/post/testing-mdbook-rust-snippets/; https://github.com/budziq/rust-skeptic).
Evidence of the failure it prevents: "README's 5 active rust snippets are never compiled — all 5 fail
as doctests" (https://github.com/SlopMaster2/webhook-verify/issues/399), and path-filtered CI letting
doc-only PRs break included examples (https://github.com/mag1cfrog/delta-arrow-reader/issues/135).
Verdict. ADAPT, from the walking skeleton (R7) onward: port trait signatures shown in docs/architecture
are either included from the crate source or compiled as `no_run` doctests against the real crates.
Until code exists, skip. CI path filters must include docs/** for this job.

## 6. Architecture fitness functions and dependency rules (P14, P15)

What it is. "An architectural fitness function provides an objective integrity assessment of some
architectural characteristic(s)"; categories include atomic vs holistic, triggered vs continual, and
temporal (Building Evolutionary Architectures ch. 2,
https://www.oreilly.com/library/view/building-evolutionary-architectures/9781491986356/ch02.html).
cargo-deny `[bans] deny = [{ name = "x", wrappers = ["x-wrap"] }]` allows a banned crate only as a
direct dependency of named wrappers and reports `unmatched-wrapper` otherwise
(https://embarkstudios.github.io/cargo-deny/checks/bans/cfg.html;
https://embarkstudios.github.io/cargo-deny/checks/bans/diags.html). Projects with layer rules write a
`cargo xtask` over `cargo metadata` with an allowed-edge table
(https://github.com/Ma1achy/Principia/pull/16).
Evidence: the pitfalls are documented in public issue trackers: matching members by name instead of
path gives false violations (https://github.com/synnaxlabs/foundation/issues/834); host-only
`cargo tree` misses `cfg(target)` edges (https://github.com/AuDHSOS/AuDHSOS/issues/419); build-deps and
name-prefix matching escape the guard (https://github.com/getwyrd/wyrd/issues/868); out-of-workspace
path deps pass (https://github.com/AuDHSOS/AuDHSOS/issues/421).
Verdict. ADOPT, mapping onto draft item 6:
- Triggered, atomic (every PR): archlint, `gen --check`, `xtask check-dag`, `cargo deny check bans`.
- Temporal (weekly / at steward phases): the semantic audit of item 7 and the lychee online run.
- check-dag reads allowed edges from manifest.toml, matches by manifest path, uses `cargo metadata`
  without a host filter, and treats normal, build and dev edges separately (each pitfall above).
- cargo-deny `wrappers` encodes "only crate A may use external crate X" (e.g. one storage crate owning
  the SQLite binding), which is the code-side form of R2's single owner. Which crates those are is a
  manifest fact, not decided here.

## 7. Formal and semi-formal consistency checks (P16, P17)

What it is. Alloy: bounded relational model finding; "most inconsistent models have counterexamples
within small bounds" (small scope hypothesis; https://cacm.acm.org/research/alloy/). AWS reports
TLA+ and later the P language for protocol design (S3 strong consistency) and PObserve for checking
production logs against P specs, and names "formal specifications as test oracles" as perhaps the
most important benefit (Brooker and Desai, CACM 68(6) 2025, https://doi.org/10.1145/3729175;
Queue version https://queue.acm.org/detail.cfm?id=3712057).
Cost. Real: each model is a second artefact that must itself be kept consistent with the docs; the
benefit concentrates in concurrent/distributed protocols.
Verdict.
- P16 SKIP for document consistency. The neomail contradictions are naming, ownership and dependency
  facts, which relational queries over the manifest catch without a model checker. Revisit TLA+/P only
  if one protocol (for example a sync or approval state machine) becomes the source of repeated bugs;
  that is a code-phase decision.
- P17 ADOPT: treat manifest.toml plus the extract JSON as a facts database and write the cross-file
  rules as plain Python queries in archlint ("every redaction step's process is on the daemon side of
  every egress edge", "every crate that names a type depends on the crate that defines it", "every
  permission rule for an agent profile is defined once"). The extract already has the shape
  (`concerns` with defines/restates/references roles, `config_keys` with role, `cross_refs`). This is
  the lightweight-formal-methods idea (automatic, partial, cheap) without a new language.

## 8. Multi-agent LLM systems and shared state (P18-P22)

### P18. Shared context and explicit task contracts
Cognition (12 June 2025): "Share context, and share full agent traces, not just individual messages"
and "Actions carry implicit decisions, and conflicting decisions carry bad results"; their example is
parallel subagents building a Flappy Bird clone with a Mario-style background and a mismatched bird,
and their recommendation is a single-threaded linear agent where possible
(https://cognition.com/blog/dont-build-multi-agents).
Anthropic's research system (13 June 2025) saw subagents that "misinterpreted the task or performed
the exact same searches as other agents" and fixed it with task descriptions carrying "an objective,
an output format, guidance on the tools and sources to use, and clear task boundaries"; outputs go
to the filesystem rather than through the coordinator
(https://www.anthropic.com/engineering/multi-agent-research-system).
Fit. The neomail design was written by five parallel architects; most contradictions are exactly the
"implicit decision" kind (a name, an owner crate, a key spelling chosen independently).
Verdict. ADOPT, as draft item 9 already states, sharpened: parallel writers never decide shared facts.
The contract (manifest entities, glossary, owner map, decision IDs) is fixed and injected into every
writer prompt before fan-out; a writer that needs a new shared fact stops and requests it from the
contract owner; the writer's brief names its file (boundary), output format and the manifest slice it
may reference. Where a task cannot be split along owned files, use one writer (Cognition).

### P19. Verification at several levels
MAST analysed 1,642 traces over 7 multi-agent frameworks and found 14 failure modes in three groups:
system design, inter-agent misalignment and task verification, with framework failure rates of
41-87% (Cemri et al., https://arxiv.org/abs/2503.13657; https://sites.google.com/berkeley.edu/mast).
Secondary summaries put inter-agent misalignment near 37% and verification failures near 21%, and
read the paper as recommending per-agent, cross-output and end-to-end checks rather than one final
pass (https://thegrigorian.medium.com/why-do-multi-agent-llm-systems-fail-14dc34e0f3cb; percentages
differ between paper versions, treat as approximate).
Verdict. ADOPT: this is R5 / draft item 10. Map the levels: per-writer (archlint on the writer's file
before hand-off), cross-file (item 7 conflict detection + P17 queries), end-to-end (lens review against
docs/decisions.md and the owner's request).

### P20. Blackboard architectures
Recent papers put a shared blackboard in front of LLM agents so all roles read the same state
(Han and Zhang, https://arxiv.org/abs/2507.01701; https://arxiv.org/abs/2510.01285). None of the
papers found treats conflicting concurrent writes; they reach agreement by rounds until consensus
(search summary of the same papers).
Verdict. ADAPT. manifest.toml is neomail's blackboard: every agent reads it; only the contract owner
(or the decision-change workflow) writes it. That is single-writer ownership, which removes the
write-conflict problem the papers leave open.

### P21. Read-only cross-artifact analysis; principles re-injected every step (GitHub Spec Kit)
`/speckit.analyze` "performs a read-only cross-artifact consistency and quality analysis across
spec.md, plan.md, and tasks.md, reporting conflicts, gaps, and ambiguities", placed after tasks and
before implement; constitution violations are CRITICAL; the constitution is passed as context to every
phase (https://github.github.io/spec-kit/reference/agentic-sdd.html;
https://deepwiki.com/github/spec-kit/4.6-constitution-system). A reviewer reports it caught a spec'd
dry-run mode with no task (https://azanello.com/blog/github-spec-kit-review).
Verdict. ADOPT two parts: (a) the audit of item 7 is read-only and reports; fixing is a separate
step with one fixer per file (already in the draft); (b) the binding inputs (docs/decisions.md live
entries, glossary, owner map) are injected into every design and implementation agent's prompt, not
only into reviewers'.

### P22. How good are LLMs at finding contradictions?
- Requirements pairs from an electric-bus project: hybrid formal-logic + GPT-3 reached 60% recall;
  LLM-only 0% recall at 97% accuracy (https://www.researchgate.net/publication/380651776; via search
  summary, not fetched).
- RAG contradiction detection: Claude-3 Sonnet basic prompt precision 0.901 / recall 0.296; with
  chain-of-thought 0.951 / 0.566; "all models demonstrate high precision but lower recall"; human
  annotators agreed only 74% (https://arxiv.org/html/2504.00180v1).
- VeriSpec (Oct 2026) extracts structured rules, groups them by topic, then verifies with an LLM, and
  reports 38.5% precision on real specifications (https://arxiv.org/abs/2610.01847; search summary).
Implication. A "read all docs and list contradictions" prompt will miss most of them, and the misses are
invisible. The extraction-then-compare shape neomail already used (structured facts with roles, then
cross-file comparison) is the shape the hybrid results favour.
Verdict. ADAPT draft item 7: (1) keep structured extraction, push every comparable fact class into
deterministic comparison (P17), and reserve LLM judgement for the rule/permission/boundary residue;
(2) measure recall: before each full audit, plant a handful of known contradictions (a changed
threshold, a renamed key, a moved step) in a scratch copy and require the audit to find them; report
recall alongside the count. Without this, "zero contradictions found" cannot be distinguished from a
blind auditor. (3) refute-by-default verification stays: it protects precision, the seeded canaries
protect recall.

## 9. Ownership freshness and staging (P23, P24)

### P23. Freshness dates (Google g3doc)
Google attaches "freshness dates" with an owner and sends reminders "when the document hasn't been
touched in, for example, three months"; "Documents without owners become stale"
(Software Engineering at Google ch. 10, https://abseil.io/resources/swe-book/html/ch10.html).
Counter-evidence: bumping review dates becomes rubber-stamping; one audit PR bumped `last_verified` on
14 files with "no record of that check" (https://github.com/a-jay85/IBL5/pull/3108; search summary).
Verdict. SKIP time-based freshness; KEEP owners. The manifest already names an owner per concern (R2).
Staleness in neomail is event-driven (a decision changed, an owner section changed), which P1's suspect
links detect exactly; a calendar reminder adds noise for docs nothing touched.

### P24. Staging and atomic promotion
Regulated practice puts every item under configuration management before verification and controls
changes through a recorded change request (IEC 62304 cl. 5.1.11 and 8.2, consolidated text above;
DO-178C Table A-8 per the search summary). Local evidence: the single reviser "died mid-rewrite",
leaving half-applied revisions (owner's brief).
Verdict. ADAPT draft item 5: the staging copy is a dedicated GitButler branch per decision change or
multi-file rewrite, with checkpoint commits after each file; promotion is the lint-gated merge of that
branch. A crash leaves an unmerged branch, never a half-updated default branch. A separate staging
directory duplicates what branches already give and creates a second copy of every fact.

## 10. Concrete edits to the draft process

1. Item 1: add stable section anchors (P2) and make every § reference resolve through them.
2. Item 2: implement "value outside owner" and "unknown name/key" in archlint from the manifest
   glossary (not Vale, P12); add orphan detection: every manifest entity has one definer and at least
   one consumer (P3).
3. Item 3: decision records append-only, both-direction supersession links, status-conditioned
   citations, decisions as manifest rows validated in CI (P6, P7).
4. Item 4: compute the impact set from suspect links (P1) plus citations, then grep as a backstop;
   no section may stay suspect on the default branch.
5. Item 5: branch = staging, merge = atomic promotion, checkpoint commit per file (P24).
6. Item 6: add a ratchet baseline that can only shrink (P8); `cargo deny check bans` with wrappers and
   a path-matched, all-targets check-dag (P15); codegen-as-test markers with "do not edit" headers
   (P10); doctest/include of trait snippets from the walking skeleton on (P11).
7. Item 7: deterministic fact queries first (P17), LLM lenses for the residue; seeded canary
   contradictions to measure recall at each full audit (P22); weekly lychee online run (P13).
   Replace "counts tracked as a trend" with the ratchet of P8.
8. Item 8: once code exists, generate signature/config reference from code (P9/P10) so the docs keep
   rationale and contracts' semantics; ICRs cover semantic changes only.
9. Item 9: contract injected into every writer prompt; writer briefs name objective, output format,
   owned file and allowed manifest slice; a new shared fact goes to the contract owner (P18, P20, P21).
10. Item 10: three verification levels (per-writer lint, cross-file checks, end-to-end lens review)
    (P19).

Not adopted: dedicated RM tools (P5), Vale (P12), Alloy/TLA+/P for documents (P16), time-based
freshness (P23).
