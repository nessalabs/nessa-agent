# 676. The desktop follows commits

## Purpose

Both desktop windows learn that a conversation changed from a commit ping and
then read only what moved. A chat created in the other window shows up without
waiting for a remount. This supersedes the part of
[ADR 596](596-observe-every-owned-conversation.md) that left the panel with no
receiver: `conversation.observe` stays the way an incomplete list is finished
when the watch is not held.

- **Date:** 2026-10-09
- **Status:** accepted

## Context

The panel and the workspace each downloaded full views on a timer, and the
panel's list did not refresh until it was opened again. Paired devices already
follow `conversation.changed` and then read a head. That path admits only a
receiver binding. The panel session is a credential, not an enrolled device.
A connection may watch one conversation. The view the windows render is not
the raw record page.

## Decision

Composition pairs each unrevoked `surface:nessa-panel` credential once, as
`local-surface:<credential>`, owned by that same principal. It does not
regrant a revoked binding. `conversation.binding` returns that row for the
session's own credential, or `not_bound`. Watches and record reads still go
through `Admit passiveRead`, so a revoked credential loses the watch and the
next read.

Each window keeps one catalogue watch and one record watch on its main
connection, plus at most two extra connections, one record watch each. Two
windows then sit on the principal cap of eight. A conversation past that cap,
or whose record watch drops, is read when its catalogue row changes. The
catalogue watch dropping resumes the old poller. Membership is replaced on
the first catch-up and when the stream identity or access epoch changes, not
on a dropped watch.

The position is `recordsHead`. When it moves, the window calls
`conversation.read` once. Record pages are not folded in the client.

| Now | What happens | Next read |
| --- | --- | --- |
| Watch registered | Recheck the head. A ping during that recheck causes one more pull | No view when the head is unchanged |
| Head moved | One `conversation.read` | Stay on the watch |
| Scope or epoch changed | Replace catalogue membership and read watched conversations | Stay on the watch |
| Catalogue watch ended, or never registered | Resume the poller | No membership replace from the drop itself |
| Record watch ended or refused for capacity | That conversation is read when its catalogue row changes | The other watches stay |
| Access refused after a binding was held | Stop. Do not poll | A new epoch, if binding succeeds, replaces membership |
| Catalogue names a new conversation | The row is shown | No view unless this window has it open |

## Alternatives considered

Raising `recordTargets` lost because the published limit is the admission
rule, and two windows already fill the principal cap without raising it.

Folding `recordsPage` into a `ConversationView` in TypeScript lost because
the projection has one owner and the page has no live running flag, questions,
or permission choices.

A new "view since revision" method lost for the same reason: the view is a
bounded replacement, not a durable cursor, and the head plus one read already
says whether anything changed.

Regranting a revoked panel binding at startup lost because revocation would
silently undo itself the next time the gateway started.

## Consequences

An idle window does not list or re-read on a timer while the watch is held.
A stolen panel token can read the physical records of conversations that
token already could read as views. It still cannot name another receiver,
and a revoked credential still fails the next read. A chat that is running
only in another window does not show as running until this window reads it:
the catalogue payload has no running flag. The poller remains for a client
with no binding and for a watch that ends.
