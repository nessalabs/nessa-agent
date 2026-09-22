# 0014. Enforce hook policy only at boundaries Nessa controls

## Purpose

Give users one hook vocabulary across agent bindings, with an explicit answer
when a binding cannot enforce a requested policy. This decides the boundaries for
[#130](https://github.com/nessalabs/nessa-agent/issues/130); it does not claim the
hook runtime, importer, or policy stops are implemented.
The [pinned survey](../../design/agent_execution/hook-capabilities.md) records
provider evidence and gaps separately from this decision.

- **Date:** 2026-09-22
- **Status:** proposed

## Context

Native hook APIs are not the capabilities exposed through an ACP binding.
Observing a tool notification does not establish that Nessa can prevent its
execution. The current SDK has invocation callbacks, attributed permission
decisions, and separate local cancellation/provider settlement evidence, but no
configured pre-tool policy runtime. Claude's rich hook surface is the target
vocabulary, not a promise that every event or output works on every binding.

## Decision

The domain owns immutable verdicts and valid stop transitions; application-owned
ports evaluate configured policy and coordinate effects; provider adapters declare
facts and translate protocol effects. Extend the existing operation-capability
chain rather than introducing a second provider registry. Each feature reports
supported, unsupported, or unknown, with its enforcement scope. Unknown is not
permission to execute: unresolved requirements may be retained for negotiation,
but the application must verify them before activating a policy or dispatching
work that relies on it. Restoration revalidates requirements against the new
binding. Unsupported requirements refuse activation with a feature-specific
explanation; optional observational behavior requires an explicitly accepted
lesser contract. No silent skipped hooks.

Hooks run at submitted-input, invocation, permission-gated tool, and settlement
boundaries that Nessa actually owns. Token chunks are observations, never veto
points. A pre-tool guarantee applies only to calls whose actual execution is
gated by that binding's permission exchange. Native tools or already-running
calls outside that exchange are not covered. Denying an individual review does
not imply stopping an invocation or undoing a tool effect.

The verdict is `Continue`, `Deny`, `EndTurn`, or `CloseSession`. A denial includes
a bounded public explanation and a separate refusal outcome: continue, end this
turn, or close the session. Provider-readable explanations are sent only when the
binding can carry them; no steering message stands in for a permission answer.
Policy stops are local decisions, not invented provider terminal results. A
stop fences the affected work before awaiting audit or teardown, and retains the
first cause for each owner. End-turn support is advertised only when cleanup and
subsequent session reuse can be enforced; otherwise a policy requiring it is
refused, not silently promoted to session closure.

Enforcement is fail closed. Hook spawn, timeout, malformed output, panic, and
unsupported verdict failures cannot allow the guarded action. `on_error: deny`
is the default; `on_error: allow` is available only for explicitly advisory hooks,
reported as such and never accepted as an enforcement requirement. A policy stop
severs further execution rather than asking the model to cooperate. Best-effort
provider notification is separate evidence and cannot delay cleanup. The panel
always gets the local reason even when notification fails; resumption must not
claim the model saw a reason without delivery evidence.

Context is a borrowed, read-only view of observations already retained for the
current invocation, capped at 256 events and 64 KiB of payload per evaluation.
One application owner binds the context cut to the exact binding generation,
invocation and guarded review, using a causal watermark from the ordered adapter
stream. Queued observations must reach that prefix before evaluation; a lagging
snapshot is incomplete even below the window limits. Missing correlation or an
unavailable prefix fails closed without holding a lock needed to consume it.
The view states omitted-prefix and incomplete-text facts. Adjacent chunks of the
same message/channel form one logical text sequence, so matching crosses chunk
boundaries without treating them as separators. A boundary cutting a text
sequence is explicitly incomplete; absence in that window is not proof of absence
in the turn. Required complete context that does not fit fails closed. No new
history retention is authorized. Thinking is excluded by default; disclosure to
a hook requires explicit configuration by the verified policy administrator and
is recorded with the immutable rule revision. Hook input is sensitive, untrusted
model/provider data; it never supplies executable configuration or authority.

Executable hooks come only from gateway-owned configuration authorized by its
verified administrator. Native import produces a reviewable Nessa configuration
and a per-hook adopted/different/unsupported report; import neither executes nor
activates hooks. Commands use fixed executable/argument vectors, explicit working
directory and an allowlisted environment; model input travels as bounded data,
never shell interpolation. Execution has bounded input/output, elapsed time,
concurrency, and owned process cleanup through injected application ports. A
subprocess boundary alone is not a sandbox. Native hooks stay disabled; inability
to establish suppression refuses the affected binding's startup or restoration
before work, including when no Nessa hook is configured. This is a required future
contract, not a claim that every current adapter already enforces suppression.

Every consequential verdict retains session/invocation/tool or permission target,
before/after meaning, bounded reason, and the exact rule ID, immutable revision,
and verified configuration grant action. Reuse `ApprovalRuleReference`'s meaning:
the automatic executor is not the human grantor. Policy initiation is explicit,
not relabelled client/provider/runtime activity. Domain validation protects
cause/target relationships; application attribution accompanies it through audit,
snapshots, restoration, gateway and panel. Audit acknowledgement, local decision,
wire delivery, provider outcome, and physical cleanup remain separate facts.
Audit failure is visible and never prevents necessary cleanup. General logs carry
rule/tool identities and safe reasons, not raw inputs or thinking.

## Alternatives considered

- Re-enable native hooks: Nessa cannot provide one audit or authority boundary.
- Promise every Claude event on ACP: native availability is not protocol access;
  unsupported concepts need explicit semantics and evidence first.
- Treat denial as permission deferral: ACP's resolved refusal cannot become a
  later approval through steering. Remembered approvals and bounded decisions are
  [#141](https://github.com/nessalabs/nessa-agent/issues/141); human questions are
  [#146](https://github.com/nessalabs/nessa-agent/issues/146).
- Fail open for enforcement or merely tell the model to stop: neither preserves
  the policy boundary when the hook or model fails.

## Consequences

Parity is incremental and testable per event, output, binding, and delivery mode.
The cost is refusing some imported policies until their enforcement is proved.
The survey and capability matrix must distinguish source evidence, exercised
behavior, and unknowns. Implementation must cover caller loss, late provider
results, concurrent controls, restoration and audit-sink failure under the
[canonical review gate](../../../CODING_STANDARDS.md#local-code-review-gate).
Context tests pause observation consumption before permission arrival and cross
restoration, stale generations, and native steering; no allow precedes the exact
causal prefix. Include valid complete prefixes and oversized single events.
Suppression tests cover failure both with and without configured Nessa hooks.
A Nessa-owned BYOK harness and invented equivalents for provider-only concepts
remain outside this decision.
