# 746. A busy discovery source does not end the watch

## Purpose

Two devices following one conversation share one discovery cache. A read that
finds the cache checked out, or that is still waiting for a worker, must not
spend its discovery budget on that wait. Otherwise the watch ends
`unavailable` / `source_preparing` while the source is still getting ready.
This amends the occupied row of
[315](315-bounded-terminal-discovery.md). The step size, the 200 ms budget, and
the client's four preparing attempts stay as they are.

- **Date:** 2026-11-02
- **Status:** accepted
- **Issue:** [#746](https://github.com/nessalabs/nessa-agent/issues/746)

## Context

On a loaded macOS runner the two-device live follow ended
`unavailable` with `source_preparing`. An occupied checkout returned in
microseconds, and the admitted read counted each of those returns toward its
128 steps, so it answered `source_preparing` four times before the owner
finished one step. Separately, the 200 ms clock started when the read was
queued, so a worker that had not started yet could already be out of budget.
Raising the step count, the attempt count, or the budget would hide either
case. A source that never becomes ready must still end.

## Decision

An occupied checkout is not a discovery step. The read waits until its own
stream's entry can be checked out again, or its existing work budget
elapses, then calls again. Another stream that is still checked out does
not hold that wait. The unbounded head and page scans use the same wait, so
they return when their stream's entry is back. The budget
starts when the worker is about to make its first discovery call, after it
has a thread and has looked up the stream. A real validation step still
counts, and four preparing attempts still end a source that stays unready.

## States

| State | Event | Next | What stays true |
| --- | --- | --- | --- |
| Admitted, cache free | one discovery call | `Preparing` or `Ready` | the call validated one step; progress is kept |
| Admitted, cache occupied | one discovery call | wait on the same read | no step was spent; nothing was validated |
| Waiting for this stream's entry | that entry can be checked out again | the next call on this read | another stream still checked out does not keep the wait |
| Waiting for the owner | the work budget elapses | `source_preparing` | the permit is released; the client retries as it does today |
| Queued for a worker | the thread has not started | the budget has not started | queue time is not discovery |
| Discovering | 200 ms of calls, or 128 validating calls | `source_preparing` if not ready | same bounds as 315 |
| Subscription ended `source_closed` | the change source closed | the follower subscribes again from its cursor | the desktop and the stress follower already do this; a bounded `follow` command still prints the end |

`an_occupied_discovery_cache_does_not_spend_the_read_steps` and
`time_waiting_for_a_read_worker_is_not_the_discovery_budget` are the rows for
the occupied wait and the clock. `occupied_answers_do_not_spend_discovery_steps`
is the step count with the cache replaced by the answer.
`a_head_for_one_stream_proceeds_while_another_stays_checked_out` and
`a_read_proceeds_when_its_stream_returns_while_another_stays_checked_out` are
the row where the other stream stays checked out.

## Alternatives considered

Counting occupied calls as steps, and starting the clock at queue time, is
what ended the watch. Waiting until every cache entry is idle would keep a
reader of one stream blocked while another stream is checked out, and the
unbounded head and page scans would wait for as long as that other stream
stayed busy. More preparing attempts, or a longer budget, would let
a stuck source run longer and would still lose to a tight loop of empty
answers. Treating `source_closed` as fatal is already rejected for the
desktop and the stress follower; this record does not change the bounded
`follow` command, which reports that end and stops. Resuming an agent turn
whose scheduling receipt and terminal result disagree is a different owner
([#1034](https://github.com/nessalabs/nessa-agent/issues/1034)) and is not
settled here.

## Consequences

A second device waits out the first device's step instead of ending. A cold
read that fits in 200 ms of work still finishes after it has waited for a
thread. A source that stays occupied for the whole budget still answers
`source_preparing`, and the client's four attempts still end it. The permit
is held through the queue as well as the budget.

## Review brief

Base is `main` at the merge of #1242. Head is this branch. The owned behavior
is the discovery wait and when the budget starts. Review the state table
against `operation.rs` and `terminal_discovery.rs`, and try an occupied cache
that is released only after the old step loop would have given up. The stress
follower's `source_closed` resume, the review-answer receipt, and the access
menu focus wait are already on `main` and are not this diff.
