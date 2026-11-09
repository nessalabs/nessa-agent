# Nessa Agent sampled docs re-review, round 2

Reviewed the changed backup-restore and native-submissions pages plus client navigation on `codex/1263-docs-reader-quality`. No edits. This is a repeat review by the source-aware reviewer from round 1, not a fresh novice study. The task answers below were constructed from the revised docs only; subsequent source checks use the already identified production boundaries. No TLC or implementation tests were rerun, and this pass does not advance source checkpoints or checker receipts.

## Repeat reader tasks

### Restore: verification passed, but the live destination already exists

Path: gateway backup/restore owner → “What acceptance checks.” The page explicitly says verification and activation are separate, and acceptance requires an empty live destination. A passing bundle cannot authorize overwriting that destination. No guessing is needed; the example directly addresses this case.

### Restore: acceptance failed after consuming readiness; may I reuse that stage?

Path: restore → “What acceptance checks” → “If acceptance fails.” Answer: no. The page says spending occurs before promotion, and “a later failure requires a fresh stage to retry, even if the rename failed and the old folder remains.” The chart now puts Spent before promotion and has an inspection outcome. A new stage also does not bypass the empty-destination requirement.

### Restore: does an error prove the gateway's live folder is unchanged?

Path: restore → example → “If acceptance fails.” Answer: no. Rename may have happened before sync confirmation or final audit failure; Retained also does not imply an empty destination. The reader is told to inspect stage and live effects, with no invented universal recovery command. The chart labels durability-unconfirmed as already live and distinguishes inspection from rollback.

### Native submissions: cancel the second request while the first handoff is running

Path: client index → “Queue a native product submission” → example/table. Answer: a queued request is removed by identity and does not execute if cancellation wins before the handoff begins. Cancelling an already started handoff cannot prove the gateway did nothing. The next handoff need not wait for the first ticket's reply.

### Native submissions: close, then cancel a selected body that has not started

Path: native submissions → “Cancellation and orderly close.” Answer: close alone leaves the selected attempt alone; cancellation before start can suppress it and reports Closed if the lane is already closed. The table and diagram now agree. Once the body starts, its own result survives cancellation/close.

### Native submissions: is the three-request model the actual queue capacity, and were device tests run?

Path: “Queue limits and other operations” → “Source and executed evidence” → “Bounded rule model.” Answer: no configured Swift capacity/wait timeout, and Swift waiting is outside the Rust request deadline. Two focused production-Swift probes ran in the prior verification pass using generated-session stand-ins for worker tests. Real UniFFI, live gateway and phone qualification were excluded. This prose edit did not rerun those probes or TLC. The earlier contradictory broad regression disclaimer is removed.

## Source fidelity and round-1 closure

- **Restore spent-before-promotion:** resolved. Matches `accept_quarantine`'s stored spent record before live intent and `layout.promote`, and its documented fresh-stage retry requirement.
- **Post-publication errors:** resolved. Final audit failure and Retained effects are explained without asserting successful rollback. The diagram does not try to turn every error into one precise OS outcome.
- **Selected native cancellation after close:** resolved. Table/chart follow `ProductSubmitLane.cancel`, which chooses Closed when already closed and uses `settleIfWaiting` to suppress only unstarted work. Started-body ownership is preserved.
- **Evidence contradiction:** resolved. Earlier executed probes and currently unrun checks are separated explicitly.
- **Unexplained terms:** materially improved. Quarantine/live destination, journal, spent, handoff, ticket, drain group and capture have local definitions. Required attestations remain a technical detail rather than an operator recipe; the page appropriately declines a universal recovery command.
- **Navigation:** client index now exposes the native submission owner under Browse this area and states the mixed TypeScript/Swift coverage. It retains the important gateway/SDK post-acceptance ownership boundary.
- **Model claim scope:** restore explicitly says the unchanged model abstracts acceptance/retirement rather than enumerating the revised diagram's individual promotion/audit/retirement failures. The prose does not represent this edit as a fresh checker qualification.

## Verdict

Pass for the sampled technical and operational-reader scope. No unresolved material finding from round 1 remains in these revised pages. This verdict covers two owners and their immediate navigation, not all 173 atlas pages, implementation correctness, or a fresh independent-reader test. Historical source revisions and model receipts should remain unchanged as requested.
