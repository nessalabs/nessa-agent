# Independent outbox retention-cost review

## Result and frozen scope

**Clean scoped review: blocker 0, major 0, minor 0, nit 0.** This is an independent review of the cleanup correction, not approval of all of PR #1101 or completion of #896.

- Checkout: `/workspace/nessa-webhook-retention`, branch `codex/896-outbox-retention-cost`.
- Reviewed candidate: `a917b479cab25b81db1652e8a9bb46f8b6f55d3a`.
- Implementation source: `611b5a87ae3e0256f50e72b16bc4a1e4cc5e4d49`.
- Intended base: `0e1f44def810c8b91439000f52986869cb0cdfc2`.
- Checkout was clean before review. Candidate changes after the implementation source are documentation/evidence only. No reviewer writes to checkout, Git state, primary Cargo artifacts, or implementation evidence occurred.

Inspected the complete implementation diff: `outbox/journal.rs`, `outbox/schema.sql`, `tests/webhooks/infrastructure/outbox/cleanup.rs`, its test module declaration, and `docs/design/webhook-delivery-implementation.md`. Also inspected changed state/evidence documentation and relevant owning `store`, `publication`, `restoration`, `capacity`, `source_reset`, schema validation, domain/application contracts and adjacent tests. Read repository review/gates/organization/audit instructions, architecture and DI guidance and relevant codebase ownership map.

## Agreement and lifecycle checks

The related facts have these owners:

| Relationship | Enforcer and check |
| --- | --- |
| Per-registration terminal rank versus protected pending work | `journal::TRIM_TERMINALS` selects its cutoff from the same registration and the same terminal-only partial index as candidate deletion. Descending implicit rowid ordering preserves the original window-query ordering. Offset `RETAINED_LIMIT - 1` plus strict `< cutoff` preserves exactly the newest1024 rank positions. Queued/sending/retry rows do not participate. |
| Rank versus owed audit | All terminal rows count toward rank, including owed rows. The outer `NOT EXISTS published=0` guard independently protects every older owed delivery. `capacity::check` remains the owner of refusing unbounded retained/pending state before commit. |
| Delivery ownership versus audit retirement | New `delivery_audit_retire` trigger retires only that deleted terminal delivery's published rows. It does not erase owed or nonterminal history. Published registration projections have their own indexed cleanup. No blanket delivery-history scan remains. |
| Audit retirement versus reset fact references | Existing membership cascade and audit-release triggers decrement the same immutable fact's references transactionally. Final-reference deletion occurs only when the retained cursor/member/audit owners have released it. Independent rollback/release probe confirms the new trigger composes with these owners. |
| Commit, durable file and exact acknowledgement | Existing `commit`, `finish`, `publication::publish`, `SqlOutbox::run/open` ordering is unchanged: owed batch correlation is checked before publication; files are synced/published and directory synced before exact one-row acknowledgement; restart validates schema and retained relationships. A durable file with a lost SQL acknowledgement remains replayable. |
| Stopped registration versus unrelated active registration | Existing retirement facts retain original target/actor/stop and drained-audit identity. Independent real-owner lost-ack/reopen probe retires one registration while another keeps its pending delivery and becomes due after recovery. |

Concrete contradiction probes include removing the registration correlation, deleting owed deliveries, removing audit retirement, and miscounting the boundary. These fail rather than merely producing valid individual records. Positive counterparts preserve mixed-state histories, original retirement actor, pending work in another registration, and transactional reset ownership. New SQL metadata fixtures deliberately do not claim domain-valid authenticated history or successful restoration. Actual-owner canonical tests cover those boundaries separately.

No policy, authorization, principal/grant admission, signature, destination, retry, deadline, network effect or flush owner changes appear in this diff. The original registration-auth correction and unbuilt W24 composition remain outside this review; their findings are neither reopened nor certified here.

## Independently executed evidence

Artifacts below are relative to `/tmp/nessa-retention-independent-review`.

- Independently compiled actual current Auth source into this review directory using `auth-resumed-args.json` and immutable dependencies under `/tmp/nessa-retention-dependency-snapshot`. Independently compiled copied actual current domain/application/outbox modules using `args-resumed.json`. `provenance.json` confirms all35 copied webhook source files match the checkout. The small private driver substitutes only the unchanged data-shaped `Target`; this is not full gateway composition or Cargo package-selection qualification. All reviewer-produced binaries/logs/copies stay in this directory.
- `baseline.txt`: **90 passed,0 failed**, including the real SQLite/filesystem full1024-boundary retention tests, publication failure/exact-ack/restart, source-reset retention/mutation, retirement and restored-history tests. Synthetic keys/paths only; no network recipient.
- `probes.rs` and `probes.txt`: initial **3 passed,0 failed**. Independent deterministic differential scenarios compare old and new cleanup over four interleaved registrations, mixed queued/sending/retry/terminal phases, published and owed audit, and registration audit. The final strengthened fixture uses2000 mixed rows per registration at the production1024 threshold, in addition to small-threshold scenarios. A separate SQL probe proves owed protection, final-reference release and rollback restoration with foreign keys enabled. A real-owner async fixture combines two registrations, credential stop, lost drained acknowledgement and reopen, preserving the other delivery and original retirement actor.
- `mutations.json`: **five independent mutation controls caught, each restored and rerun successfully**: off-by-one production offset (unchanged full-boundary test fails), cross-registration cutoff (differential probe fails), removed owed guard (differential probe fails), removed retirement effect (reference-release probe fails), removed materialization (existing VM-work test fails at67862 steps for128 terminal rows). Assertions verify every mutation changed the copied bytes and every restoration returned the original bytes; compilation always used those actual copies and the frozen dependencies. Individual compile/test/hunk files preserve the outcomes. The first differential controls used the smaller initial fixture; the final fixture is stronger.
- `source-proof.json`: independently confirms original full boundary function hashes are identical to base (`1d316c32…713` terminal and `b531e2d1…78f` stop), application port/bounds are byte-identical, later candidate content is docs-only, and checkout remains clean.
- `git diff --check` passed for base through candidate. State checkpoint names the actual implementation source; changed docs accurately retain the uncomposed-feature and qualification caveats. Visually inspected both committed rendered diagrams. They preserve the lifecycle described above.

The final restored suite result is recorded in `final-93.txt`; its exact outcome is appended below after completion.

## Supporting evidence and remaining gates

Inspected the implementer's retained source checkpoint, cost-profile methodology, control scripts/results, canonical test summaries, docs validation/render evidence and preserved initial archive-link failure. Those are supporting supplied evidence, not reviewer re-executions of the canonical120 server webhooks,212+55 Auth tests, seven-package Clippy, architecture check, profiler/strace or135-page docs validator. The archived failed link is not a pass. Work reduction and unchanged flush counts support a scoped SQL cost correction; they do not prove a Windows deadlock or a verified Windows fix.

Bundled rusqlite0.40 supplies SQLite; forced partial indices and MATERIALIZED are exercised by this Linux dependency snapshot. Windows, minimum supported Rust execution, the repository CI feature/package-selection matrix and assembled-stack integration were not executed here. Existing schema equality intentionally rejects noncurrent private schemas; no migration compatibility was added to this unmounted API.

Pinned Chromium/WebKit, scripted gateway browser evidence, stress/chaos and exact-head CI remain applicable external gates. System Chromium documentation renders do not qualify them. This review does not waive any gate or authorize merge. No publication, CI retry/dispatch, browser download, live registration, persistent credential, real recipient, public listener or security/network/account change was attempted. No tool action was denied or left uncertain during this review. The separately recorded historical denials remain untouched.

The correction is suitable for integration into the existing scoped stack, subject to combined-tree checks and parent-owned qualification/merge decisions. No independent implementation finding remains open in this round.

## Final restoration check

`final-93.txt`: **test result: ok. 93 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 17.95s** This run independently rebuilt the restored actual-source harness and includes the strengthened four-registration production-limit differential scenarios.
