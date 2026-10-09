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

Composition pairs the desktop's own panel credential once, as
`local-surface:<credential>`, owned by that same principal. The credential is
the one issued as `SurfaceProvision` for principal `surface:nessa-panel`.
A paired phone is issued to that same principal with `DevicePairing`, and is
not bound here. A revoked binding is not regranted. A journal error is logged
and the gateway still starts; the windows poll until a later start can bind.
`conversation.binding` returns that row for the session's own credential, or
`not_bound`. Watches and record reads still go through `Admit passiveRead`.
That admission is the whole passive-read surface: record head and page, and
catalogue head, manifest, and resolve. The desktop calls head, watch, manifest,
resolve, and `conversation.read`. It does not call `recordsPage`.

Watches are registered on a connection of their own. A watch refusal closes
that connection and the window resumes the poller. It does not sign the
session out. Each window holds one catalogue watch. A record watch is held
only while a conversation runs, waits, or has an app call unanswered, at most
three (one beside the catalogue watch, two on further connections). An idle
window holds the catalogue watch and no record watch. Two idle windows then
hold two of the principal's eight watch slots. A conversation without a record
watch is read when its catalogue row changes. The catalogue watch dropping
resumes the old poller. The next poll round tries the watch again, including
after a binding refusal. Membership on the first catch-up, and on a scope or
epoch change, comes from `conversation.list` (and `conversation.observe` when
that list is not the whole catalogue). A later ping resolves only the entries
that moved.

The position is `recordsHead`. When it moves, the window calls
`conversation.read` once. Record pages are not folded in the client. While a
conversation runs, waits, or has an app call, the window also reads it on the
fast timer: running, approvals, questions, and app calls are joined onto the
view and are not themselves commits.

| Now | What happens | Next read |
| --- | --- | --- |
| Watch registered | Recheck the head. A ping during that recheck causes one more pull | No view when the head is unchanged |
| Head moved | One `conversation.read` | Stay on the watch |
| Scope or epoch changed | Replace catalogue membership from the list and read watched conversations | Stay on the watch |
| Catalogue watch ended, or never registered | Resume the poller | No membership replace from the drop itself |
| Record watch ended or refused for capacity | Retry the watch. If it still fails, read that conversation when its catalogue row changes | The other watches stay |
| Access refused after a binding was held | Resume the poller. The next round tries the watch again | Views stay allowed |
| Catalogue names a new conversation | The row is shown, newest update first | No view unless this window has it open |
| Conversation running, waiting, or calling an app | Fast poll of that conversation, and a record watch while a slot is free | The list stays on the catalogue watch |

## Alternatives considered

Raising `recordTargets` lost because the published limit is the admission
rule. Idle windows release record watches so a phone still has slots under
the principal cap of eight.

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
A conversation that is running, waiting, or calling an app is still read on
the fast timer. A stolen panel token can read the physical records of
conversations that token already could read as views, including record pages
and catalogue payloads, because passive-read admission is not narrowed to
head and watch. It still cannot name another receiver, and a revoked
credential still fails the next passive read. The view read stays available,
so the window polls. A chat that is running only in another window does not
show as running until this window reads it: the catalogue payload has no
running flag. The poller remains for a client with no binding and for a
watch that ends.
