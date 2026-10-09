# 596. The desktop index observes every owned conversation

## Purpose

The desktop index can show every summary the caller owns, past the newest-first
list of 500. The catalogue already walks a caller's rows in creation order.
This decision uses that walk from the desktop without pairing the panel as a
linked reader. The order of the walk is the table in
[UI workspace load](../../design/ui-workspace-load.md#how-the-desktop-observes-every-owned-summary).

- **Date:** 2026-10-06
- **Status:** accepted. The local panel surface now has a receiver binding for commit watches ([676](676-desktop-follows-commits.md)). This walk remains how an incomplete list is finished when that watch is not held.

## Context

`conversation.list` returns at most 500 rows and has no cursor. `complete` is
false when rows were left out. A newest-first cursor would move while a
summary's `updated_at` changes, so a page could skip or repeat. The catalogue
pass is creation order and finite: a captured head, then descriptors after a
cursor. That pass was built for a paired receiver. The panel credential is not
paired, and a catalogue payload has no running flag.

## Decision

`conversation.observe` is an ordinary `conversation.write` command, the same
grant as `conversation.list`. The server pages the caller's catalogue and
returns `ConversationSummary` rows, including `running`, at most one catalogue
page at a time, plus `complete` and an optional cursor. The desktop index
still asks `conversation.list` first. When that list is incomplete, it walks
`conversation.observe` inside the same call budget until the pass finishes.
A finished pass is the membership. A page that cannot resume does not remove
sessions it left out. `conversation.list` stays the bounded newest-first view.
Neither call opens a provider. `conversation.read` still takes a live slot.

## Alternatives considered

Raising `MAX_LISTED_CONVERSATIONS` and treating one response as complete lost
because a larger single response is still a bound, and the issue's pass
condition is the stored count.

Calling `conversation.catalogueManifest` from the desktop lost because the
panel has no receiver binding, the handshake does not publish one, the payload
has no running flag, and a second payload decoder would be a second owner.

A newest-first cursor on `conversation.list` lost because `updated_at` changes
during the walk. [ADR 196](196-conversation-metadata-database.md) had already
deferred page tokens for that reason.

## Consequences

The index can observe more stored summaries than one list holds. The panel's
message list still uses `conversation.list`. A 10,000-row browser run is still
a separate measurement. A pass that does not finish leaves omitted sessions in
place until a later pass finishes.
