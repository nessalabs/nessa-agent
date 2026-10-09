# 676. The desktop follows commits

## Purpose

Both desktop windows learn that a conversation changed from a payloadless
commit ping, then run the list or read they already have. A chat created in
the other window shows up without waiting for a remount. The poller stays for
any chat the ping does not cover.

- **Date:** 2026-10-09
- **Status:** proposed

## Context

The panel and the workspace each downloaded full views on a timer, and the
panel's list did not refresh until it was opened again. Paired devices already
follow `conversation.changed`, which carries only `{watchId}`, and then read
through their own receiver. The panel session is a credential, not an enrolled
device. A connection may watch one conversation and one catalogue. The view
the windows render is not a raw record page.

[ADR 596](596-observe-every-owned-conversation.md) left the desktop unpaired
for four reasons, stated under Alternatives:

1. The panel has no receiver binding.
2. The handshake does not publish one.
3. A catalogue payload has no running flag.
4. A second payload decoder would be a second owner.

596 took the other road for the index: `conversation.observe` is an ordinary
`conversation.write` command, the same grant as `conversation.list`, and it
returns `ConversationSummary` rows, including `running`.

## Decision

An owner session registers the existing change watches with no receiver. The
watch params keep `receiverId` and `accessEpoch` optional. Both present is
the paired-receiver path, admitted as before by passive-read. Both absent is
the owner path: `conversation.write`, the same grant `conversation.read`,
`conversation.list`, and `conversation.observe` already ask for, plus
ownership of the named conversation for a record watch. Exactly one of the
two fields is an invalid request. Registration, each notice, and the
periodic tick call that same admission. The ping stays `{watchId}`.

The desktop does not mint a receiver, and it does not call
`conversation.binding`. There is no `not_bound` error. Startup does not
write a pairing row for the panel credential. Revoking the credential is
what ends the session, which the socket already does.

Each window opens one connection used only for watches, separate from the
connection commands use. That connection holds the catalogue watch and at
most one record watch. The published per-connection limit stays one record
target. The follower does not read a view, a head, or a catalogue payload.
It signals two things:

- the list changed, and the window runs its existing list (`list()` /
  `listConversations`, including `conversation.observe` when the list is
  incomplete);
- chat X changed, and the window runs its existing read (`read(X, "held")`
  / `refreshConversation`), including the taken-out check, sequenced reads,
  and timeouts.

The catalogue watch replaces only the list timer. A record watch replaces
only that chat's fast poll, and only when nothing unsaved is pending: an
app review, a provider still starting, a turn still running or queued, or
a chat this window has not read yet, keeps the 250 ms poll. A running
turn's text is the live view, and a commit ping does not carry it. Every
other chat keeps the poll too. A watch
refusal or the follow connection closing resumes the timers. The next poll
round tries the watch again. It does not sign the session out.

| Now | What the window does |
| --- | --- |
| Catalogue ping | One existing list |
| Record ping for the watched chat | One existing read of that chat |
| Chat has no record watch | Its 250 ms poll stays |
| App review, provider still starting, or a turn still running or queued | That chat's poll stays, and it does not take the record slot |
| Catalogue watch ended, or never registered | The list timer returns. The next round tries the watch again |
| Record watch ended or refused for capacity | That chat polls. The catalogue watch stays |
| Access refused on the follow connection | Both timers return. The next round tries again |

## Why 596's four reasons still hold

1. Still holds. The panel has no receiver binding, and this decision does
   not manufacture one.
2. Still holds. The handshake does not publish one. The owner path sends
   no receiver fields.
3. Still holds for a catalogue payload: it has no `running` flag. The
   desktop does not read that payload. The list and `conversation.observe`
   do carry `running`, and a list ping uses those.
4. Still holds. The desktop does not decode a catalogue payload, so it
   does not become a second owner of that encoding.

## Alternatives considered

Folding `recordsPage` into a `ConversationView` in TypeScript lost because
the projection has one owner and the page has no live running flag, questions,
or permission choices.

A new "view since revision" method lost for the same reason: the view is a
bounded replacement, not a durable cursor, and one existing read already
returns it.

Manufacturing a panel receiver so the desktop could pass passive-read
admission lost because of the four reasons above. It would also let the
panel token read record pages and catalogue payloads, which a ping does
not need, and it would write a pairing row nobody's Linked Devices screen
manages.

Raising the published per-connection record-target limit is not part of
this decision. That limit bounds how many hint bytes one connection may
queue. It is not the principal cap. The principal cap counts watches, on
every connection, and phones share it. Raising the per-connection limit
would change how many sockets a window needs, not how many watches the
principal is allowed. This decision leaves the limit at one and keeps the
other chats on the poll.

A catalogue delta (`conversation.observe` since a revision) is a follow-up.
A list ping today asks for the list, which is at most 500 rows, and the
catalogue pings when a message is accepted or a reply settles, not on the
old one-second timer.

## Consequences

An idle window does not list on a timer while the catalogue watch is held.
Two idle windows hold two catalogue watches. A window following one settled
chat holds one more record watch. A turn that is still running or queued
does not take that slot. That is not "two windows sit on the cap of eight":
the cap is eight watches for the whole principal, shared with any phone, and
these windows do not fill it by themselves.

A catalogue ping costs one `conversation.list` (and `conversation.observe`
when that list is incomplete). A record ping costs one `conversation.read`
through the path the window already uses. Neither number is a
commit-to-screen latency. Owner-session watches do not admit `recordsPage`
or catalogue payloads.

Chats past the one record slot, chats whose provider is still starting,
chats with an app review open, and chats whose turn is still running or
queued keep the fast poll. Streaming text moves on that poll, because a
commit ping does not carry it.

Follow-ups, not this decision: a server-side catalogue delta; one follower
shared by both windows, or one owner-wide watch; more than one record
target on the owner connection; moving the follower into `@nessa/client`.
