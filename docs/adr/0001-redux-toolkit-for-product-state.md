# 0001. Product state lives in Redux Toolkit

- **Date:** 2026-08-31
- **Status:** accepted

## Context

The shell will grow into a large product, and an agent will need to drive it
the same way a user does — send a draft, open a tab, stop a reply — without
going through React. The conversation strip is the first piece of that state.
Host concerns (frost, resize, the window size) are subscriptions to the
operating system, not product state.

The binding constraint is **named, serializable actions** a non-React caller
can dispatch. Local component state cannot offer that. A store that only
exposes hook setters cannot either.

## Decision

Use Redux Toolkit and react-redux for product state. The first slice is the
conversation strip. It is a projection: named actions call
`ConversationGateway` and store the strip. `store.dispatch` is the agent
entry point. See [0002](0002-conversation-vertical-and-gateway.md).

Host, DOM, and clocks stay in hooks: `use-host-panel`, `use-panel-frame`,
`use-edge-reveal`, `use-color-scheme`, `use-surface`, and
`ConversationClocks`. They subscribe to something outside the product and
must not become reducers.

`useEffect` is the right tool for those subscriptions. It does not belong in
a component that only renders (`app.tsx`).

## Alternatives considered

- **Zustand.** The 2026 default for a new React app, and what several large
  products use. It lost because the agent needs a `dispatch(action)` contract
  and DevTools replay, not a bag of imperative setters. Zustand can be shaped
  that way; Redux Toolkit already is.
- **Jotai / atoms.** Fine for derived UI. Poor as an agent façade: there is
  no one place to send "the user hit send".
- **Keep `useState`.** Cheap today, and the strip is still small. It cannot
  be dispatched into from outside the tree, so it would have to be replaced
  the moment an agent existed.
- **A four-layer DDD tree around this slice.** Deferred at the time. The
  conversation vertical later earned it — see 0002 — because a server will
  own the commands and the UI must already be a projection.

## Consequences

An agent can `dispatch(setDraft({ draft: "…" }))` then `dispatch(sendDraft())`
and the panel updates. Tests of the strip do the same, with no renderer.

What stays hard: deciding whether a new piece of state is product or host.
If it is a window, a pointer, a clock, or a CSS token, it is a hook. If an
agent should be able to ask for it by name, it is a slice.

Watch for the store importing the panel chrome, or conversation use cases
importing React or Redux — both are architecture-check failures. The slice
is already a projection of the gateway; a remote runtime replaces the local
adapter, not the chrome.
