# Permission targets — issue 797

The client owns capture of a permission's conversation, execution, identity and
offered choices from one validated view or its trusted local projection.
`permissionTargets` publishes frozen, opaque, process-local values. Answer and
cancel consume one value rather than independent IDs. Its weak registry rejects
copies or fabricated targets without retaining targets after callers drop them.
The panel keeps these values in deliberate control arguments, not persisted state.

This is a caller contract, not authorization. The gateway still resolves current
conversation access; SDK `ExecutionSession` and gateway `AppReviews` own pending
review decisions. A fabricated input view is not authenticated by the factory.
Wire DTOs remain raw IDs, and a saved/JSON-copied target must be replaced by a
target from a newly read view. Opening epochs and stale-review semantics remain
with their existing owners.

PR 796 has already landed turn `Submitter`/`Initiation` attribution. Permission
capture changes neither that contract nor snapshot schema, policy revision,
verified caller attribution, or audit ownership.

## Ordering and enforcement

| Row | Input or ordering | Required result | Enforcer |
| --- | --- | --- | --- |
| PT1 | Capture reviews in two conversations or executions | Each target retains its enclosing conversation and its own execution/review | `permissionTargets` |
| PT2 | Change source IDs/options after capture | Captured target and choice set remain unchanged | Frozen copied fields and private copied option set |
| PT3 | Substitute a field, spread/JSON copy, or fabricate a target | Refuse before client RPC; panel rejects before create/control | Issuance registry via `permissionFields` / `permissionTargetConversation` |
| PT4 | Route a captured target to another local tab's server conversation | No create or control for either conversation | Control thunk's conversation agreement check |
| PT5 | Capture then switch active tabs, or await preparation | Answer/cancel still sends the captured target | Control argument and client serialization |
| PT6 | Select another review's unoffered option | Refuse before client RPC | Captured published choice set |
| PT7 | Correct target and offered option/cancel | Existing wire IDs and request attribution are sent | Conversation API |
| PT8 | Review resolves or access changes after capture | Existing gateway refusal; no automatic replay | Gateway/SDK owners and existing client control-error contract |

The client and panel regression suites exercise PT1–PT7. PT8 stays with existing
SDK, gateway and control-error tests; targets confer no current-state authority.
Question answers, other execution controls and persistence correlation remain
follow-up slices of [#797](https://github.com/nessalabs/nessa-agent/issues/797).
