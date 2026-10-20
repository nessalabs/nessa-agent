# 136. A turn stopped by policy says so

## Purpose

When a configured rule or a permission-review hook stops a turn, the person
reading the turn sees why, and the audit keeps that local decision apart from
whatever the provider reports afterwards. This is the panel, log, and snapshot
slice of [0014](0014-nessa-owned-policy-hooks.md) for
[#136](https://github.com/nessalabs/nessa-agent/issues/136). It does not add a
hook runtime, an importer, or a product setting that turns a refusal into a
stop. The live gateway still installs a continue refusal.

- **Date:** 2026-10-20
- **Status:** accepted

## Context

A binding can be configured to end the turn or close the session when it
refuses a tool review. A permission-review hook can return the same two
outcomes. Both already record evidence. The configured stop's audit record
carries `ApprovalRuleReference` (rule id, revision, and who granted it). The
hook's withdrawal carries `ApprovalBasis::Hook` on the actor and the hook's
reason on the cancellation. Neither fact reaches the sentence the panel reads.

The panel reads execution events, not the audit sink. The declined-review
event kept the tool and the structural reason, and dropped the rule. After a
restart the snapshot restored that event, so the rule was gone from the turn
even when the audit still had it. A hook that ends the turn cancels the
review and finishes the turn without a notice, which is the quiet panel this
issue exists to stop.

A provider result can still arrive after the local stop. That reply is already
a separate late-result record. It must not become the turn's outcome and must
not replace the sentence.

## Decision

The declined-review event carries the same rule the audit record carries.
One check pairs them: a stop has a rule, a continue does not. The panel
sentence is read from that event. Audit success is not an input to the
sentence. The snapshot stores the rule id, revision, and grantor with the
event and restores them through the same constructors. A saved stop without
a rule, or a saved continue with one, is corrupt.

A hook stop is not relabelled as a rule. The cancellation already names the
hook. When its code is an end-turn or a close-session hook refusal, and its
review id is a notice id, the panel says the turn stopped or the session
closed, names the tool, and quotes the hook's reason.

The gateway log for a configured stop names the rule id, its revision, and
the tool. The hook stop log names the tool, the hook index or `runtime`, and
the reason, and it is written only after that stop is applied. An end-turn
that arrives after the provider has already settled, and a close that does
not confirm, are not logged as a stop. Neither log carries tool arguments or
the grantor.

The audit record for the decline says the turn was `running` before the
decision. After a continue it is still `running`. After an end-turn it is
`stopped`. After a close it is `closed`. The provider's later result stays a
separate late-result record.

## Ordering

The local stop is fenced before audit and before the provider is told. The
first matching row is what happens.

| What is observed | Result |
| --- | --- |
| A configured refusal stops the turn | The notice is published with the rule, then the turn is finished locally as cancelled. The audit record of that decision is attempted after the fence. |
| The provider's result arrives after that fence | The local outcome and the notice stay. The reply is a late provider result, a different record. |
| The audit sink fails or stalls | The turn reports the audit failure. The notice already published still names the rule and the tool. Cleanup still finishes. |
| The session snapshot is restored | The event's rule id, revision, and grantor are the ones that were saved. The panel sentence is computed again from that event. |
| A hook returns end-turn or close-session | The review is withdrawn as the hook. The notice quotes the hook's reason and names the tool. The audit record is the cancellation, with the hook as initiator. The hook stop log follows the applied stop: the turn cancel, or a confirmed session close. |
| A hook end-turn arrives after the provider has already settled | The session stays open and no hook stop is logged. The provider finished the turn on its own. |
| A hook returns continue, or an end-turn the binding cannot perform and so records as continue | No stop sentence. The turn keeps running. |
| A hook stop's review id is not a notice id | No notice is added, because a notice id that the client rejects would drop the whole view. The cancellation and its audit remain. |
| Provider notification of the stop fails | The local notice and the local outcome stay. The notification failure is its own log line. |

## Invariants

- A stopping decline and its audit record name one rule. A continue names
  none. `stopping_rule_matches` is the only check, used by the record and by
  the event.
- The panel sentence is a function of the event. It does not read the audit
  sink, and a later provider result does not rewrite it.
- The grantor is the person who configured the rule. The initiator of the
  stop is the rule, or the hook. Neither is stored as the person who was
  using the turn.
- General logs name the rule or the hook and the tool. They do not include
  tool arguments.

## Alternatives considered

- Leave the rule only on the audit record and have the panel read the audit.
  The panel's source is the event stream. An audit failure would then hide
  the reason the stop exists to show, and a restored snapshot would still
  have no rule.
- Encode the rule into the existing reason sentence and parse it back. The
  rule is an identity. A sentence is not a store, and two parsers would be a
  second owner.
- Add a second event type for "policy stop". `ReviewDeclined` is already the
  local refusal. A parallel event would disagree with it.
- Record a hook stop as `ApprovalBasis::Rule`. A hook has an index and a
  reason, not a configured revision and a grant. Relabelling it would name
  the wrong initiator.

## Review brief

Review the checkout on branch `cursor/policy-stop-says-so-454a` against
`main`. Intended behavior is the table above. Owned scope is the declined
review event, its snapshot, the panel sentence, the two log lines, and the
audit before/after fields. Out of scope: a product setting that selects a
stopping refusal (the gateway still installs continue), native hook import,
and phone notification of a waiting review (#989), which is why this change
does not close #130.

The live gateway does not install a stopping refusal, and no conversation
stress step can produce one without that setting. The projection tests are
the panel contract. Do not treat a missing stress step as covered by a unit
test of a different feature.

Inspect the pairing check, the snapshot decode rejects, the projection
sentence, and the worker publication order (notice before the audit await).
Adversarial cases: a late provider success, a stalled audit, a restored
event whose rule was stripped or attached to a continue, and a hook
continue that must not say the turn stopped.
