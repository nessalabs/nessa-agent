# 1034. A captured stop is finished after a restart

## Purpose

A turn a person stopped, or a session close, can be saved before its
scheduling edge. After a gateway restart the turn is still unfinished.
Recovery records that edge and does not start another turn. This settles
[#1034](https://github.com/nessalabs/nessa-agent/issues/1034). A confirmed
stop receipt is still finished by the gateway Stop owner (#981). A decline
that lands before a successor is admitted is still the only decision (#1214).

- **Date:** 2026-11-02
- **Status:** accepted

## Context

Chaos seed 2 runs 182 actions and two restarts. A watch that had been
resumed was still unfinished when the 240-second settlement deadline passed.
The saved stream was complete and current. The turn had no terminal result.

The stop can be durable in two places. The command receipt can be settled
as cancelled while the turn history is still running; the gateway Stop owner
declines that turn and does not resume it. Separately, the invocation can
already hold the local stop (a person's stop, or a session close) while its
scheduling history is still queued or running. That turn was left as it was.
It was not owed, so nothing recorded the missing edge, and nothing was
allowed to continue it either. The event stream therefore never said the
turn was cancelled.

## Decision

The session history owns the decision. A queued or running turn with no
result, nothing running it in this process, and a captured person's stop or
session close is unfinished until that stop's scheduling edge is saved.
`resume_turn` and `decline_turn` record the edge and admit no successor.
A host shutdown or a retryable failure is still continued. A decline that
wins before successor admission still admits nothing.

## Alternatives considered

Hiding the row from the settlement check loses because the turn is still
unfinished. Extending the 240-second deadline, or running fewer actions at
once, loses because the turn never ends. Resuming it loses because a
person's stop is final. Recording `NotResumed` after the captured stop loses
because that edge may follow only a stop that left the work owed.

## Consequences

The same execution gains one cancelled scheduling edge and no successor.
A second restart finds nothing owed. Intentional suspension and a restart
that did not capture a person's stop still resume, with the same derived
successor identity. What we would watch for: a captured stop whose cause is
neither a person's stop nor a session close, left unfinished because this
rule does not name it.

## States

| What is saved | What recovery does |
| --- | --- |
| Queued or running, no result, no captured stop, nothing live | Owed. Resume records Interrupted and admits one successor. |
| The same, and a settled cancelled or withdrawn stop receipt | The gateway Stop owner declines it. No successor. |
| Queued or running, captured person's stop or session close, scheduling not yet terminal | Owed until that edge is recorded. Resume and decline record it and admit no successor. |
| Scheduling already cancelled by that stop | Not owed. Not resumed. |
| Suspended, or interrupted by a restart, with no person's stop | Owed. Resume continues it. |
| Decline recorded before successor admission | No successor. The turn is not owed. |
