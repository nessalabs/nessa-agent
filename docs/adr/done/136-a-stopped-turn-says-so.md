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
sentence. A configured close says the session is closing. That event requests
the close, and this projection does not see whether cleanup confirmed it, so
the sentence does not say the session has closed. The snapshot stores the rule id, revision, and grantor with the
event and restores them through the same constructors. A saved stop without
a rule, or a saved continue with one, is corrupt.

A hook stop is not relabelled as a rule. The cancellation already names the
hook. When its code is an end-turn or a close-session hook refusal, and its
review id is a notice id, the panel says the turn stopped or the session
closed, names the tool, and quotes the hook's reason. That notice is a hook
notice. A declined-review notice is a review notice. A deadline that settled
its review is a deadline notice. `ReviewDeclineId` and `PermissionId` are
different identities, and admission checks each within its own type, so the
same decimal may name a decline, a permission, and that permission's deadline
in one turn. The wire carries which decision the notice is. No sentence
replaces another.

The gateway log for a configured stop names the rule id, its revision, and
the tool. The hook stop log names the tool, the hook index or `runtime`, and
the reason, and it is written only after that stop is applied. An end-turn
that arrives after the provider has already settled, and a close that does
not confirm, are not logged as a stop. Neither log carries tool arguments or
the grantor.

The audit record for the decline says the turn was `running` before the
decision. After a continue it is still `running`. After an end-turn it is
`stopped`, because that execution has already been finished locally. After a
close-session it is `closing`: the record is the requested close, and cleanup
can still be unconfirmed. The confirmed session transition is the separate
`session_closed` record, whose after is `closed`. The provider's later result
stays a separate late-result record.

## Ordering

The local stop is fenced before audit and before the provider is told. The
first matching row is what happens.

| What is observed | Result |
| --- | --- |
| A configured refusal stops the turn | The notice is published with the rule, then the turn is finished locally as cancelled. An end-turn says the turn stopped. A close says the session is closing. The audit record of that decision is attempted after the fence. |
| The provider's result arrives after that fence | The local outcome and the notice stay. The reply is a late provider result, a different record. |
| The audit sink fails or stalls | The turn reports the audit failure. The notice already published still names the rule and the tool. Cleanup still finishes. |
| The session snapshot is restored | The event's rule id, revision, and grantor are the ones that were saved. The panel sentence is computed again from that event. |
| A hook returns end-turn or close-session | The stop is applied first. The review is then withdrawn as that stop, and that cancellation is the notice: the tool and the hook's reason. The hook stop log is the same applied stop. |
| A hook end-turn arrives after the provider has already settled | The refusal is recorded as continue. The panel does not say the turn stopped, and no hook stop is logged. |
| A hook end-turn cancellation is followed by a finish that is not a cancel | The sentence stays off the turn. It is shown when that execution is cancelled. |
| A hook close-session | The withdrawal is recorded while the session can still record it, then the session is closed. The panel sentence is that cancellation. |
| A hook returns continue, or an end-turn the binding cannot perform and so records as continue | No stop sentence. The turn keeps running. |
| A hook stop's review id is not a notice id | No notice is added, because a notice id that the client rejects would drop the whole view. The cancellation and its audit remain. |
| A hook stop's permission id equals a declined-review id in the same turn | Both notices stay. They are different decisions. The hook sentence is not dropped, and a later review update does not rewrite the hook sentence. Admission accepts the history. |
| A deadline settlement's permission id equals a declined-review id in the same turn | Both notices stay. The deadline sentence is a deadline notice. A later review update does not rewrite it. |
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
- A review notice, a hook notice, and a deadline notice are different
  notices. The same decimal may name each of them. The notice kind is what
  tells them apart. Admission rejects a repeated permission id, and a
  repeated decline id. It does not reject a permission id that equals a
  decline id.

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
- Keep permission ids and decline ids in one sequence so a deadline notice
  cannot repeat a decline's id. That made the independent notice kinds
  unreachable for any adapter or restored history that assigned the same
  decimal, and it rejected a history the panel can show. The deadline's own
  notice kind is what keeps the sentences apart.
- Record a close-session decline as `after: closed`. The decline is written
  when the close is requested. The session can still be unconfirmed. The
  confirmed transition already has its own `session_closed` record.

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
