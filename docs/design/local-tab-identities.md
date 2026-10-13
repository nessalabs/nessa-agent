# Local tab lifetimes — issue 812

The local tab and the durable gateway conversation have different identities.
Restoring a saved gateway reference creates a new local tab lifetime. The
existing `takeConversationId` allocator owns those local names; restoration
advances the current LocalTabs counters rather than reconstructing names from
array position. The identity rule is owned by
[gate 12](../../CODING_STANDARDS.md#gates).

An admitted command may still finish against its captured remote conversation
after the tab disappears. Its old local publications then find no tab. This
preserves command semantics without adding another cancellation or publication
state machine. The active-tab follow effect already cleans up and starts again
when local identity changes.

| Row | Ordering | Expected local result | Evidence |
| --- | --- | --- | --- |
| LT1 | Restore while control preparation waits, then close succeeds | Captured remote conversation closes; replacement cancellation/draft/ready state is untouched | Store restored-lifetimes tests |
| LT2 | Restore while permission effect waits, then it fails | Failure returns to original operation; replacement has no old error or follow | Store restored-lifetimes tests |
| LT3 | Restore same remote conversation, start another control, finish old control | New control remains pending | Store restored-lifetimes tests |
| LT4 | Restore while send waits, then admission succeeds or fails | Replacement draft, error and read ownership are unchanged | Store restored-lifetimes tests |
| LT6 | Restore active A into B with readiness unchanged | Existing follow(A) stops; new follow(B) begins | Hook/browser lifetime verification |
| LT7 | Repeated or empty restoration followed by ordinary open/close | Fresh local names and preserved counters; saved remote selection/titles remain valid | Saved-tabs tests |

The allocation state is scoped to the current store lifetime. Saved snapshots
retain remote references and titles, not local names, pending commands or
permission authority. Remote execution, cleanup, authorization and initiation
remain with their existing owners.

File upload publications also need their local parent identity, independently
of tab allocation. That separate confirmed correlation defect is tracked in
[#814](https://github.com/nessalabs/nessa-agent/issues/814); fresh tab names alone
cannot protect a publisher that names only a file.
