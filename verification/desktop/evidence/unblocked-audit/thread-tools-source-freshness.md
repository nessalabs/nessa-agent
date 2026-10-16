# Agent tool source facts at admission

Implementation base: 77ae675a52ca6d4b97e844a5dafe71f845025e2b, isolated branch
codex/thread-tools-source-freshness. The initial metadata read remains an early
existence diagnostic, preserving the existing source capacity and cancellation
seams. After awaited live lookup and snapshot capture, source metadata is read
again directly before synchronous Cedar evaluation. No new policy comparison,
global lock, or deletion-cleanup serialization is introduced. Actor, source
execution, and lease remain those captured in the one snapshot; no later turn is
selected to explain this call. The design orderings were updated before code.

Regression: source_admission_reloads_metadata_after_waiting_for_live_evidence.
It captures the initial metadata reply using the existing repository gate, holds
the real service conversation map to prevent live lookup, then commits the real
service deletion fence. The Agent remains running and its original execution
is still active. Releasing the map lets admission gather live evidence, but the
new last metadata read must present the tombstone to Cedar and return NotFound.
A target service.create is executed only if admission succeeded; the test checks
no target record or provider dispatch after refusal. The denied audit preserves
source, target, original execution/lease, delivery call, raw logical request,
scoped command, actor, surface, initiation, and default-deny reason/policy ids.

Neighbor cases in that same deterministic test: unchanged source admits and
creates its target; a source cancelled after initial lookup retains its existing
allowed-decision audit and starts no target; a late metadata error returns the
original typed Metadata error without inventing a policy decision or dispatch.
Existing tests retain missing-source, pre-cancelled-source, audit failure, and
caller-loss coverage.

Coordinator verification on assembled source 302967fe: the four-case regression
and all nine `agent_tool` tests pass, including the actual MCP relay path. A fresh
independent review reports no actionable findings. Normal product generation and
all 67 protocol tests, formatting, type checking, and the strict seven-package
Clippy selection pass.

Completed mutation: restored the early load's `let record =` binding and removed
only the late reload before Cedar, asserting the changed hunk. The unchanged
regression failed at the fenced case with `Ok(())` after target creation, rather
than the required NotFound (exit 101). Restored committed source with fresh mtime
and byte equality; the regression passed (exit 0), and the worktree was clean.
An earlier interrupted mutation was restored and is not claimed as proof.
