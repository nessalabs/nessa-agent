# 0002. Conversation is a vertical; the server will own the rules

- **Date:** 2026-08-31
- **Status:** accepted
- **Updated:** 2026-09-03 — removed the local reply stand-in and phase clock;
  send/stop are no-ops until chat RPCs exist.

## Context

The panel will grow a real agent runtime and a gateway. Product rules — when
a send is legal, how a reply lands, that the strip never empties — must not
live in React components. The frontend should mostly call for a change and
paint UI state. [0001](0001-redux-toolkit-for-product-state.md) put the strip
in Redux so an agent can dispatch it; it left the rules in one module and
deferred a layered tree.

The second consumer is now planned: a server (or host-side runtime) that owns
those operations. The UI must already be shaped as a projection so that swap
does not rewrite the chrome.

## Decision

The conversation feature is a vertical slice:

```
src/conversation/
  model/                 contract the panel and the future server share
                         (strip = conversations + activeId; no id mill)
  application/local-strip.ts
                         UI-session counters; dies with the local adapter
  application/usecases/  one file per command
  application/ports.ts   ConversationGateway
  adapters/gateway/      local UI-session adapter; later a remote adapter
  adapters/store/        Redux projection — apply gateway result, nothing else
  ui/                    presentational; no product rules
```

Reducers do not contain rules. They call `ConversationGateway` and store the
strip they get back. `localConversationGateway` runs the use cases
in-process. A remote adapter replaces that object; the panel still only
dispatches and paints.

Use cases split into two kinds:

- **Server-owned (no-ops until chat RPCs):** `sendDraft`, `stopGenerating`.
- **UI session:** `setDraft`, `setActive`, `openConversation`,
  `closeConversation` — the composer and the open tab, until those persist.

Host subscriptions (frost, resize, compositor flush) stay in adapters, not
in the store and not in `app.tsx`.

## Consequences

`src/conversation/application/usecases/` is the catalog for this feature.
Each use case is independently testable. The transcript cannot import gateway
internals or the host seam.

When chat RPCs exist, the local adapter goes away and the use cases become
`await gateway.sendDraft(…)`. `LocalStrip` goes with it. The store grows an
async boundary (`createAsyncThunk` or a listener); the UI does not change
shape. Other modules keep importing the conversation barrel, not the files
under it.
