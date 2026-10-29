# Independent retention fixture review — round 1

Result: **CLEAN: no blocker, major, minor or nit findings** in the scoped delta.

Checkout `/workspace/nessa-webhook-recovery`, intended base `e4d08234a5aa57e57f1f1ab7d475aa1c651fb995`, reviewed head `2cde0447bda02a474485cb47824f7c8a2cb67697`. The tree was clean and remained at that exact head throughout review. I made no shared source edits, commits or external writes. I read AGENTS.md, CODING_STANDARDS.md (including gates, local review loop, agreement and evidence), relevant architecture/codebase organization guidance, and all four changed files.

## What was checked

- `tests/webhooks/infrastructure/outbox.rs`: compared each changed test against the exact base. All original assertions and the real final operations remain. The terminal test still queues, admits and completes the actual1025th delivery; the reset still acts on1024 pending rows with a changed source; the stop still fails actual immutable publication after the audit directory is renamed, inspects committed state before recovery, then restores publication and verifies retirement. The two added reset assertions distinguish actual invalidation from the pre-existing1024 terminal rows.
- `outbox/retention_fixture.rs`: traced representative real public operations and generated prehistory against production `queue.rs`, `journal.rs`, `publication.rs`, `stored.rs`, `restoration.rs` and `schema.sql`. Generated states use the domain transitions and owning canonical encoders/audit builders. All new audit bytes are actually written, synced and closed; the directory is synced before the fixture transaction commits. No file/query/transaction guard survives the helper return to obstruct the Windows directory renames. Original capacity and restoration owners validate the fixture. No production durability or permission path changes.
- Oracle: actual two-terminal/two-pending public history is compared for both Sent and Refused against generated history. Fresh registration/conversation/delivery/audit identities alone are normalized; byte payloads, decoded state/scope, audit facts, publication flags, cursor position and durable projections remain compared. Checked why omitted SQL identity columns remain correlated by the canonical stored/cursor/restoration validation. The small oracle is not represented as1024 independent public acknowledgements or native crash recovery.
- Adversarial/adjacent paths: examined concrete missing-trim, omitted source-invalidation, wrong-epoch and valid-payload cases. The original control logs show that missing trim breaks all three actual full-boundary tests; missing invalidation reaches the newly added pending-count assertion; wrong epoch fails `WrongEpoch` before oracle comparison; valid payload substitution reaches and fails the full equivalence assertion (`sent=false`). Current fixture SHA256 is `068477c05a5832f73e99965de1ea8b2872f490174bf96ab3e0411b75f3b0cd22`, matching the byte-restored payload-control receipt. I inspected these original control receipts/source hooks; I did not repeat the author mutations or claim them as independent mutation executions. Independently ran the adjacent published/owed audit deletion and nonterminal history refusal tests.
- Evidence README/control JSONL: explicitly distinguish domain refusal from oracle failure, preserve original compile/Windows failures, label Linux timings and pending Windows qualification, and avoid claiming delivery/runtime completion. Tests-only changes do not invalidate the historical state-diagram source checkpoints. New helper remains in its owning test module; no runtime/API/dependency/workflow change occurs in this delta.

## Independent checks

With the supplied Rust toolchain/Cargo home:

- `cargo test --locked -p nessa-server --lib retention_is_bounded -- --nocapture`: **3 passed**,2.54s. Log `/tmp/nessa-896-retention-review-boundaries.log`.
- `cargo test --locked -p nessa-server --lib acknowledged_fixture_matches_real_public_history_and_durable_projections`: **1 passed**,0.07s. Log `/tmp/nessa-896-retention-review-oracle.log`.
- `cargo test --locked -p nessa-server --lib delivery_retirement_removes_only_its_published_audit_and_preserves_owed_rows`: **1 passed**. Log `/tmp/nessa-896-retention-review-owed.log`.
- `cargo test --locked -p nessa-server --lib deleting_nonterminal_metadata_does_not_erase_its_acknowledged_history`: **1 passed**. Log `/tmp/nessa-896-retention-review-nonterminal.log`.
- Seven-package CI selection: `cargo clippy --locked -p nessa-local-storage -p nessa-auth -p nessa-server -p nessa-protocol -p nessa-client-core -p nessa-device-client -p nessa-sdk --all-targets -- -D warnings`: **passed**,14.97s. Existing lease-protocol constant-evaluation warning is unchanged. Log `/tmp/nessa-896-retention-review-ci-selection-clippy.log`.
- `git diff e4d08234a HEAD --check`: **passed**. Final HEAD and clean worktree rechecked.
- Inspected, rather than duplicated, author's final206-webhook result (7.82s),108 architecture tests/scanner and restored oracle evidence.

## Limits

This review qualifies the scoped test setup/oracle delta on Linux. It does not qualify native Windows behavior, independently reproduce the earlier Windows cancellation failures, establish exact-head CI success, or complete #896. The existing file-handle repair remains native-CI pending. It does not mount command/startup/shutdown ownership, principal share resolution, source feed/sender composition or actual recipient E2E. No live registration, persistent real credential, production data, external recipient or public listener was used.
