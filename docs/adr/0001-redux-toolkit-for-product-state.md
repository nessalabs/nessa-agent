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
conversation strip. Rules stay in `conversation.ts`; the slice is the adapter
that turns those rules into actions. `store.dispatch` is the agent entry
point.

Host, DOM, and clocks stay in hooks: `use-host-panel`, `use-panel-frame`,
`use-edge-reveal`, `use-color-scheme`, `use-surface`, and the per-conversation
`ReplyTimer`. They subscribe to something outside the product and must not
become reducers.

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
- **A four-layer DDD tree around this slice.** The repo's own rule: a module
  does not earn `domain/application/adapters` until a concept has an
  invariant, a second consumer, or its own storage. Conversation rules are
  already a pure module. That is enough.

## Consequences

An agent can `dispatch(setDraft({ draft: "…" }))` then `dispatch(sendDraft())`
and the panel updates. Tests of the strip do the same, with no renderer.

What stays hard: deciding whether a new piece of state is product or host.
If it is a window, a pointer, a clock, or a CSS token, it is a hook. If an
agent should be able to ask for it by name, it is a slice.

Watch for the store importing the panel chrome, or `conversation.ts`
importing React or Redux — both are architecture-check failures. Revisit this
record if the agent grows a runtime that owns the turn lifecycle itself; the
slice would then become a projection of that runtime, not the source of
truth.
