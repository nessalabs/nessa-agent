# 238. The desktop workspace is a Redux projection of a workspace port, with pure pane layouts and shared components

## Purpose

Turn the desktop window's workspace spike into production frontend code:
organisation (sections → channels → sessions), chat panes that split and move,
two selectable layouts, Settings, and themeable icons. It extends
[0001](../done/0001-redux-toolkit-for-product-state.md)'s product state and
[0002](../done/0002-conversation-vertical-and-gateway.md)'s gateway seam to the
desktop window, so the backend can attach later by implementing one port.

- **Date:** 2026-09-27
- **Status:** proposed ([#238](https://github.com/nessalabs/nessa-agent/issues/238)); implemented, in review

## Context

The spike on `desktop-app` proved the experience, and people will choose between
its two layouts in Settings:

- _three columns_: sidebar, a channel's session list, chat;
- _sessions in the sidebar_: channels disclose their sessions inline.

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
  "close the focused pane" must be named actions (0001), not hook setters.
- **Calm means no dropped frames.** No frame over 50 ms on split, move, close,
  sidebar toggle, send or typing in a production build.

## Decision

The desktop window gets a `workspace` vertical in `src/desktop/workspace/`,
laid out feature-first with roles below, like `src/conversation/`.

- **`model/`** holds pure values and rules:
  - the organisation's types (`Section`, `Channel`, `SessionSummary`, the
    session's model from the SDK catalogue);
  - a **pane layout** (`pane-layout.ts`) of columns of stacked panes, one
    focused, with tested operations — split, move, swap, nudge, close, even
    out, focus — and the limits of four panes and three columns;
    `pane-sizing.ts` holds the rules that need measured pixels: placements,
    whether a pane still fits (300 × 220), and edge drags held to that;
  - session grouping ("Needs you", "Running", "Earlier"), search, and the
    new-session lifecycle, where a draft is never listed and is let go once no
    pane shows it.
- **`application/`** owns the **`WorkspaceSource`** port and the workspace's
  state, with each use case a pure function over it (`usecases/`).
- **`adapters/`** holds:
  - `store/`: the Redux slice, which only names actions over the use cases;
    `commands.ts`, the one list of commands; `effects.ts`, listener effects;
    `refusal.ts`, which tells the source's typed refusals from faults for
    both; `hooks.ts`, the store's typed hooks; and memoised selectors per pane
    and per sidebar row;
  - `in-memory/`: an **in-memory `WorkspaceSource`**, the only home of sample
    data and the scripted, streamed replies, on timers it owns and cancels;
  - `dom/`: what belongs to the page, not the product — FLIP motion, drag and
    drop, pointer resizing, keys, a first message's arrival, the clock's ticks.
- **`ui/`** holds each component once:
  - source list (both sidebars as variants of one component), session list,
    pane grid, pane, pane header, transcript, transcript heading, message, tool
    steps, approval card, pane home (the window's own `Home`, given the
    composer's props), drop target, quick switcher and empty states;
  - **two layout compositions** in `ui/layouts/` that only arrange those.

The port is narrow and speaks the workspace's own types: the organisation, a
session's transcript, **one stream of replacement updates** (a summary, a
transcript, a removal), send, approve and deny, and the three things the lists
let a person change: pin, archive, mark read. Refusals are a typed
`WorkspaceSourceError` with a reason; anything else a call throws is a fault,
logged as one. Every replacement carries the source's **revision** of it
(`model/revision.ts`), and the newer revision wins whichever channel brought it
and whenever it arrived, so an update that overtakes a read, a read answered
late, or an organisation read while updates already flow cannot undo anything.
A removal is remembered with its revision, so an older read cannot bring the
session back.

**Commands.** Everything a person or an agent does is dispatched from
`adapters/store/commands.ts`: plain actions where the state alone decides
(`openBeside`, `movePane`, `toggleSidebar`, …), thunks where a fresh id, the
time or the source is needed (`newSession`, `closePane`, `sendMessage`,
`approve`, `deny`, …), taking them from the store's thunk extra argument
following [dependency injection](../../design/dependency-injection.md). The
thunks live beside the slice rather than in `application/`, because the
architecture check keeps Redux out of `application/`, as it does for the
conversation vertical. What follows from a change whoever caused it — reading
a newly shown session's transcript, marking a shown one read — is a listener
effect, so a plain action from an agent gets it too. "Shown means read" has
that one owner: opening a session, the organisation's first session, and a
shown session the source marks unread again all reach it.

The desktop gets its own store and composition root: `src/desktop/store.ts`,
with `src/desktop/dependencies.ts` building its dependencies and
`src/desktop/main.tsx` wiring dependencies → store → provider → icons → window.
A pane subscribes only to its own session, so streaming into one pane renders
one pane; `ui/panes/pane-isolation.test.tsx` holds that.

**Layouts.** Settings › Workspace › Layout chooses _three columns_, _sessions
in the sidebar_, or _classic_. Both workspace layouts share the pane grid and
everything in a pane; they differ in the sidebar (channels beside a session
list, or channels that disclose sessions), the session list, and the quick
switcher, which only the second has. Where the spike's two copies of a pane
differed, one design was kept for both: the three-column pane chrome and grid
(which also gives the second layout split down and moving panes), the
second's richer transcript (step groups with diff counts, code, lists, the
live activity row and "Always Allow"), and the first's arrival motion.
**Classic stays**: it is the existing `ui/desktop-app.tsx` shell with a home and
a right panel, it shares `Home` and the composer rather than duplicating them,
and removing it is a product decision this work does not make.

Settings is a typed catalogue: categories → tabs → settings, with the search
index derived from it, in `src/desktop/settings/model/`, rendered generically
by `src/desktop/settings/ui/`. Preferences stay host-side adapters that notify
other readers in the same window — theme, icon family, workspace layout and
tint are each a `storedPreference` (`src/desktop/adapters/stored-preference.ts`).
They are not Redux state, because an agent does not dispatch "tint from
picture".

Icons resolve through a provider that mirrors nessa_ui's `NessaIconProvider`
contract. It has semantic roles, nested partial overrides, and the resolution
order component → nearest provider → parent → default. Families are data, and
a preference chooses one. It stays in `src/desktop/ui/icons/` — it is UI, and
moving it would be churn — until nessa_ui ships the contract, and is then
replaced by nessa_ui's, not kept beside it. The composer's access shield is
the one exception: nessa_ui's `ComposerAccessMode` has no icon slot, so its
masked outline stays in `styles.css` until nessa_ui adds one.

Motion animates transform and opacity only. Layout changes are applied at once
and played back with FLIP. The measurement is taken in React's commit phase,
just before the DOM changes (`FlipScope`, `getSnapshotBeforeUpdate`), so any
dispatch — a click, a key, an agent — animates, and it plays only when the
panes' arrangement changes, never while an edge is dragged. Blur and large
shadows pause while panes fly; a new pane's content fills in the frame after
its shell, in a transition. Durations and curves are `--desktop-*` tokens
defined once in `styles.css` and read by script and stylesheet alike; reduced
motion sets the durations to zero there, and script motion follows with no
check of its own.

### Commands in flight

What each command shows at once, and what each of the source's answers does
to it. One row, at least one test (`adapters/store/commands.test.ts`,
`application/usecases/sessions.test.ts`, `application/usecases/updates.test.ts`).

Nothing the person does is written into what the source said. A message waits
in an **outbox**, shown after the source's conversation, until a replacement
from the source includes it by id; an answer to an approval waits in `answers`
until the source's conversation no longer asks; a model chosen for the next
turn waits in `chosenModels` until the person chooses again — no summary
speaks for the person, whichever model it names. A new session is listed at
revision 0 — the window's own — until the source first speaks of it, and every
message sent to it meanwhile carries what it starts with, which the source
takes once (`OutgoingMessage.start`). It shows running while any message may
still begin it — one on its way, or one taken and not yet shown. A refused
message is sent again in its place and under its id, with the model chosen for
the next turn at that moment; the source takes an id once
(`OutgoingMessage.messageId`), so a refusal that was not one does not send it
twice. Pin and archive are the source's: its call resolves only once its
update has reached subscribers, and that update alone shows the pin or removes
the session — a removal has one path, whoever asked for it, and closes the
pane showing the session or starts the last over, leaving the view as it was.
A session the source has not spoken of — no summary from it yet; a
conversation that arrives first is not held, and is read again once the
summary comes while a pane shows it, or when one first does — can be neither
pinned nor archived. Discarding its last refused message takes a shown one
back to the new session's home it came from, under the same id, and lets an
unshown one go; either way the window forgets all it held of it, and a summary
from the source listed later under that id replaces the home, keeping the
model chosen there. A summary is listed by one rule whether the organisation
read or the stream brings it. A read that fails after a conversation arrived
another way changes nothing, and a read begun before its session was removed
is let go whenever it answers. Starting over, a home keeps the model chosen
for the session's next turn. A refusal of a pin or an archive leaves the
session as it was, and a refused read mark stays cleared here; each is logged;
anything a call throws but the source's typed refusal is logged as a fault.
The consequential calls — an answer to an approval, a pin, an archive — carry
who asked: the person at the window's controls or an agent dispatching the
same command. The source records each the moment it is asked, then the session
as it found it when it carried the call out, and what became of it — refused,
or taken with the revision it produced (`WorkspaceSource.approve`, `deny`,
`setPinned`, `archive`). A message carries who sent it too; the source records
it when, taken, it lets a waiting approval go — naming the message, the
approval and the sender. The in-memory source keeps that record (`audit()`).
Each of these commands, and sending a message, answers its caller with what
became of it — `sent`, `refused` when the source said no, `unknown` when no
answer came and it may have been done (`unavailable`), or `not-asked` when
there was nothing to ask, and for an answer `answering` when one is already on
its way. An answer is `not-asked` when no pane shows the session, or its
conversation no longer asks that approval: only an approval on screen is
answered, so an agent answers what it has opened. An answer on its way stays
while its pane shows another session. Each answer has its own token, so an
earlier answer's late refusal never sets aside the one on its way. Every call
to the source settles — an adapter rejects on a timeout of its own rather than
leave a read or a send hanging. The one mark the window clears itself is
"unread".

| Command | Shown at once | Source takes it | Source refuses (typed) | Source's update arrives |
| --- | --- | --- | --- | --- |
| `sendMessage` to a draft | listed at revision 0, titled by the message, running; the message in the outbox, "sending" | mark cleared | "Not sent. …", with Send Again and Discard; at rest once no message may begin it; Discard of the last takes a shown one back to a new session's home and lets an unshown one go | the conversation that includes the message retires it from the outbox |
| `sendMessage` to a session | the message in the outbox, "sending", with the model chosen for the next turn | mark cleared; an approval still waiting is let go, on the source's record with who sent the message | "Not sent. …", with Send Again (`resendMessage`) and Discard (`discardUnsent`); the session as the source last said | any conversation without it — a read of the history, a reply still streaming — leaves it where it is |
| `approve` / `deny` (with the initiator: `"person"` from the card, `"agent"` from an agent) | the approval's buttons at rest | the conversation that no longer asks, delivered first, lets the answer go; the source has recorded the decision and who made it | asks again, saying why; answerable again | a conversation no longer asking lets the answer go |
| an answer to an approval the window does not hold — moved on, or its session in no pane | nothing; the command returns `not-asked` | — | — | — |
| a second answer while the first is on its way | nothing; the command returns `answering` | — | — | — |
| `pinSession` | nothing; a session at revision 0 is not pinned (Pin is disabled) | its update, delivered first, shows the pin | left as it was | a newer summary replaces it |
| `archiveSession` | nothing; a session at revision 0 is not archived (Archive is disabled) | its removal, delivered first, takes it out of the lists with everything the source said up to it; its pane closes, or the last starts over | left where it is | a summary the removal outranks stays out, whenever it arrives |
| a session is shown | marked read, and the source told | nothing more | the mark stays cleared | marked unread again while shown: read again |
| a shown session's transcript is read | the heading | the conversation, unless a newer one arrived meanwhile | the pane says why, and is not read again until "Try Again" (`retryTranscript`); a failure no pane shows any more is not kept, so the session is read afresh when shown again | the newer of it and the read's answer is kept, in either order |
| the organisation is read | nothing yet | the workspace opens on its first session; summaries the stream already brought, if newer, are kept | the workspace says why, with "Try Again" | a removal that arrived first keeps the session out |

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
- **FLIP in a hook wrapped around each dispatch.** Every command would need
  the wrapper, and an agent's dispatch would not animate. Measuring in the
  commit phase needs neither.
- **Drag state in the store.** What the pointer carries lives only as long as
  the pointer holds it and no agent asks for it; a small store beside the tree
  lets a pane subscribe to "am I lifted?" without a Redux round trip.
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
- **FLIP motion needs care.** It measures before and after a commit, so it
  lives in `FlipScope`, reads what is on screen mid-flight, and keys on the
  panes' arrangement rather than their sizes.
- **A second icon API for a while**, until nessa_ui ships its provider.
- **The two layouts' panes look alike.** Choosing between the spike's two pane
  designs was the price of writing the pane once.

Watch for:

- **A component reading the whole workspace** (`useSelector((s) => s.workspace)`),
  which brings the whole-tree re-render back; `pnpm architecture` refuses a
  selector returning the whole slice anywhere under `src/desktop/`
  (`scripts/architecture/whole-workspace.mjs`, with its tests).
- **Rules drifting into `ui/`.**
- **Frame timing regressing** on the interactions named above.
