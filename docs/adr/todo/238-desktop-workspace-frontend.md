# 238. The desktop workspace is a Redux projection of a workspace port, with pure pane layouts and shared components

## Purpose

Turn the desktop window's workspace spike into production frontend code:
organisation (sections → channels → sessions), chat panes that split and move,
two selectable layouts, Settings, and themeable icons. It extends
[0001](../done/0001-redux-toolkit-for-product-state.md)'s product state and
[0002](../done/0002-conversation-vertical-and-gateway.md)'s gateway seam to the
desktop window, so the backend can attach later by implementing one port.

- **Date:** 2026-09-27
- **Status:** proposed ([#238](https://github.com/nessalabs/nessa-agent/issues/238))

## Context

The spike on `desktop-app` proved the experience, and people will choose between
its two layouts in Settings:

- *three columns*: sidebar, a channel's session list, chat;
- *sessions in the sidebar*: channels disclose their sessions inline.

Its code cannot ship:

- each layout is one ~2,500-line file;
- the transcript, approval card, pane header, drop targets and send animation
  are written twice;
- mock data and mock agent replies live inside the views;
- all state sits in one component, so a keystroke or a streamed word re-renders
  every pane (70–150 ms per keypress in development).

The binding constraints:

- **Backend later, without a rewrite.** Nothing is wired to the gateway now, but
  the views must already be a projection, so attaching it replaces an adapter
  and touches no component.
- **Agents drive the window too.** "Open this session beside that one" and
  "close the focused pane" must be named, serializable actions (0001), not
  hook setters.
- **Calm means no dropped frames.** No frame over 50 ms on split, move, close,
  sidebar toggle, send or typing in a production build.

## Decision

The desktop window gets a `workspace` vertical in `src/desktop/workspace/`,
laid out feature-first with roles below, like `src/conversation/`.

- **`model/`** holds pure values and rules:
  - the organisation's types (`Section`, `Channel`, `SessionSummary`);
  - a **pane layout** of columns of stacked rows, with tested operations:
    `split`, `move`, `swap`, `close`, `resize`, `focus`, and the limits of four
    panes and minimum readable sizes;
  - session grouping ("Needs you", "Running", "Earlier") and the new-session
    lifecycle, where a draft never sent is dropped.
- **`application/`** owns the **`WorkspaceSource`** port: the organisation,
  a session's transcript, send, approve and deny. Its thunks are the named
  commands, and they take the port as the store's thunk extra argument,
  following [dependency injection](../../design/dependency-injection.md).
- **`adapters/`** holds:
  - the Redux slice, with memoised selectors per pane and per sidebar row;
  - an **in-memory `WorkspaceSource`**, the only home of sample data and the
    scripted replies;
  - DOM helpers (FLIP motion, drag and drop). These are hooks, not state.
- **`ui/`** holds each component once:
  - source list, session list, pane grid, pane, pane header, transcript,
    transcript heading, approval card, pane home and drop target;
  - **two layout compositions** that only arrange those components.

The desktop gets its own store and composition root: `src/desktop/store.ts`,
with `src/desktop/main.tsx` building its dependencies. A pane subscribes only to
its own session, so streaming into one pane renders one pane.

Settings is a typed catalogue: categories → tabs → settings, with the
search index derived from it. It is rendered generically in
`src/desktop/settings/`. Preferences stay host-side adapters that notify other
readers in the same window, as the theme preference does. They are not Redux
state, because an agent does not dispatch "tint from picture".

Icons resolve through a provider that mirrors nessa_ui's `NessaIconProvider`
contract. It has semantic roles, nested partial overrides, and the resolution
order component → nearest provider → parent → default. Families are data, and
a preference chooses one. It lives in `src/desktop/icons/` until nessa_ui ships
the contract, and is then replaced by nessa_ui's, not kept beside it.

Motion animates transform and opacity only. Layout changes are applied at once
and played back with FLIP. Blur and large shadows pause while something moves.

## Alternatives considered

- **Keep each layout as its own component tree.** This is how the spike got
  here: every fix landed twice, and the two copies drifted within a day.
- **Local `useState` / `useReducer` in the workspace root.** Cheap, but an
  agent cannot dispatch into it (0001), and it re-renders the whole tree. Split
  contexts fix the rendering, but add a second state model beside Redux.
- **Zustand or atoms for pane layout only.** Fast subscriptions, but it splits
  product state across two stores. Redux selectors with `useSelector` and
  per-pane memoisation reach the same render cost.
- **A tiling library (react-mosaic, golden-layout, dockview).** Each brings its
  own chrome, drag model and CSS we would fight to reach this look, for a layout
  capped at four panes. The layout rules are a small pure module we can test.
- **Build the icon provider in nessa_ui now.** It is the right home, but
  nessa_ui is out of scope for this work. Mirroring the contract keeps the swap
  mechanical.

## Consequences

Easier:

- attaching the gateway is one adapter implementing `WorkspaceSource`;
- an agent or a test can drive the window with `dispatch`;
- each component exists once, so a design change lands in both layouts;
- pane layout rules are unit-tested without a renderer.

Harder, and accepted:

- **More files.** Keeping the vertical's boundaries takes discipline, and a
  component that grows a rule must move that rule to `model/`.
- **FLIP motion needs care.** It measures before and after a state change, so
  the measurement lives in a hook around dispatch rather than in reducers.
- **A second icon API for a while**, until nessa_ui ships its provider.

Watch for:

- **A component reading the whole workspace** (`useSelector((s) => s.workspace)`),
  which brings the whole-tree re-render back.
- **Rules drifting into `ui/`.**
- **Frame timing regressing** on the interactions named above.
