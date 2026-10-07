# Test-only red checkpoint

Checkout: `/workspace/nessa-agent-owned-settlement`
Branch: `codex/625-646-649-owned-settlement`
Production/source base: `3e65d96e44e27c502a6225ec16e554fe18c4a152`
Test-only checkpoint: see `red-head.txt` (commit e3077fe2).
Only `crates/nessa-sdk/tests/application/agent_execution/subagents.rs` changed.
No production/docs changes, push or PR. Working tree clean after commit.

## Commands and actual results

All commands run from the checkout, using this exact executable/environment prefix:

```sh
/tmp/nessa-browser-libs/usr/bin/tini -s -- env CARGO_HOME=/tmp/nessa-cargo RUSTUP_HOME=/tmp/nessa-rustup PATH=/tmp/nessa-cargo/bin:$PATH CARGO_TARGET_DIR=/workspace/nessa-agent/target CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=2
```

Append these commands to that prefix:

```sh
cargo fmt --all
cargo test -p nessa-sdk --test application rejected_startup_without_owner_then_parent_close_settles -- --nocapture
cargo test -p nessa-sdk --test application released_child_with_rejected_coordinator_observation_is_not_closed -- --nocapture
cargo test -p nessa-sdk --test application rejected_startup_without_cleanup_returns_the_live_slot -- --nocapture
cargo test -p nessa-sdk --test application c5_released_process_with_failed_audit_is_not_an_audited_close -- --nocapture
```

Formatting passed. `git diff --check` passed.
`649-red.log`: exit 101, one test executed; runtime assertion at line 577, actual Err(Incomplete), required Ok(()).
`646-red.log`: exit 101, one test executed; runtime assertion at line 1121, actual Ok(()), required Err(Audit(Rejected)).
Both controls ran one test and passed (`649-control.log`, `646-control.log`).

The initial command added `--exact` after the bare 649 test name; it compiled but selected zero tests (407 filtered). `649-first.log` preserves this invalid selection. It is excluded from red or passing-test evidence. Corrected commands above ran the actual tests. No compile/fixture failure or fixture correction occurred.

## Self-review of premises

649 uses actual public coordinator admission and ScriptFactory Ready Rejected+None, with capacity 1 and real bound Released+Acknowledged root resources. Before the failing close result assertion, actual startup rejection, shared gate seal, first TerminalFailure/Runtime cause/initiator, prepares=1/submits=0, and empty factory child-resource list all pass. No synthetic physical child resource or report is supplied. Parent close is independently bounded and returns Incomplete, not a timeout. Closed parent/child and preserved first cause/counters are follow-on assertions and remain unexecuted while red. No assertion incorrectly requires the child to stay Closing immediately after the future implementation completes its failure transaction.

646 successfully prepares/submits a child whose actual resource report is Released+Acknowledged, with real root resources bound. The selective substitute accepts close intent and targets only exact child identity, some close operation and before=Closing; it deliberately does not assume after=Closing, since the old graph can already emit Closed. It captures one attempted record, holds independently through Notify, then rejects only that selected observation. During the gate, accepted Open->Closing intent, actual child close count=1 and unrelated-root successful admission through the single live slot all pass. Therefore rejection is independent coordinator evidence, not failed provider evidence, failed intent or retained capacity. After releasing the gate, physical count remains 1 and returned Ok is the decisive erroneous outcome. Closing-state assertions remain unexecuted while red.

Shared fixture extension defaults disabled, takes its one-shot selector before awaiting and holds no mutex across await. Existing generic fail_next behavior and successful record semantics remain intact; the two adjacent controls pass. Notify notify_one retains a permit if the waiter has not yet polled, avoiding a scheduling race. No sleeps or proposed settlement types are used.

This checkpoint proves the two current public outcomes only. It does not establish future debt representation, durable retry, independent child completion joins, restoration, panic containment, Ready-before-Drop ownership or store failure semantics. Those are deferred to implementation authorization and the landed #650 base update. No broad gates or independent review are claimed for an intentionally red test checkpoint.
