# 238. The desktop workspace is a Redux projection of a workspace port, with pure pane layouts and shared components

## Purpose

Turn the desktop window's workspace spike into production frontend code:
the workspace's index (sections → channels → sessions), chat panes that split and move,
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
  sidebar and list toggles, send, typing, streaming, a pane dragged across
  zones, or opening and answering in the Agents overview, in a production
  build at 4× CPU throttling.

## Decision

The desktop window gets a `workspace` vertical in `src/desktop/workspace/`,
laid out feature-first with roles below, like `src/conversation/`.

- **`model/`** holds pure values and rules:
  - the **index**'s types (`model/workspace-index.ts`: `Section`, `Channel`,
    `SessionSummary`, the session's model from the SDK catalogue) — see
    _Naming the index_ below;
  - a **pane layout** (`pane-layout.ts`) of columns of stacked panes, one
    focused, with tested operations — split, move, swap, nudge, close, even
    out, focus — and the limits of four panes and three columns;
    `pane-sizing.ts` holds the rules that need measured pixels: placements,
    the **one fit rule** (`arrange`, `fitted`: every pane at least 300 × 220)
    and edge drags held to it; `drop.ts` holds what a drop on a pane's zone
    does, the one outcome a drag previews and the command commits;
  - why the source refused (`failure.ts`, a typed reason — the sentence a
    person reads is the UI's, `ui/failure-copy.ts`), and what the window
    keeps of what it does not show (`retention.ts`);
  - session grouping ("Needs you", "Running", "Earlier"), search, and the
    new-session lifecycle, where a draft is never listed and is let go once no
    pane shows it.
- **`application/`** owns the **`WorkspaceSource`** port and the workspace's
  state, with each use case a pure function over it (`usecases/`).
- **`adapters/`** holds:
  - `store/`: the Redux slice, which only names actions over the use cases;
    `commands.ts`, the one list of commands; `effects.ts`, listener effects;
    `hooks.ts`, the store's typed hooks; and memoised selectors per pane
    and per sidebar row;
  - `in-memory/`: an **in-memory `WorkspaceSource`**, the only home of sample
    data and the scripted, streamed replies, on timers it owns and cancels;
  - `dom/`: what belongs to the page, not the product — FLIP motion, drag and
    drop (`drag.ts`), pointer resizing, keys and Tab order, focus following the
    focused pane (`focus.ts`), the page's measure of the panes' room
    (`measure.ts`), a first message's arrival, the clock's ticks.
- **`ui/`** holds each component once:
  - source list (both sidebars as variants of one component), session list,
    pane grid, pane, pane header, transcript, transcript heading, message, tool
    steps, approval card (its command and answers, `approval-request.tsx`,
    arranged by the card's width and shared with the Agents overview's peek), pane home (the window's own `Home`, given the
    composer's props), quick switcher and empty states;
  - **one workspace shell** (`ui/layouts/workspace-shell.tsx`) and two
    layouts that are only a `SidebarRegion` each — see _Layouts_.

The port is narrow and speaks the workspace's own types: the index, a
session's transcript, **one stream of replacement updates** (a summary, a
transcript, a removal), send, approve and deny, and the three things the lists
let a person change: pin, archive, mark read. Refusals are a typed
`WorkspaceSourceError` with a reason; anything else a call throws is a fault,
logged as one (`failureReason`, the one place that tells them apart). State
holds the reason, never a sentence. Every replacement carries the source's
**revision** of it (`model/revision.ts`), and the newer revision wins whichever
channel brought it and whenever it arrived, so an update that overtakes a read,
a read answered late, or an index read while updates already flow cannot
undo anything. A session has two counters — its summary's and its
conversation's — never compared with each other; **a removal is counted with
the summary** (it is the summary's next revision), and is remembered with that
revision, so an older read cannot bring the session back. An update at a
revision the source could not have sent is let go, and logged where it is
received (`followWorkspace`, `loadWorkspace`); the reducers stay pure.

**The stream may lose updates; the index is the resync.** The port does
not ask the source to replay what a dropped connection or a late listener
missed. A read of the index is instead the source's whole list when it
answered, and so authoritative: dispatched again (`loadWorkspace`), a session
it does not list — or lists in a channel it does not list — is taken out by
the same removal path the stream's removals take, at the revision held, and
the conversation of every shown session is read again. The one exception is a
session the stream brought while the read was on its way, which the read may
predate: it stays, until a later read says otherwise. A session the source
has not spoken of (revision 0) is the window's own and stays. A pane is never
left showing a session that is not there: a removal closes the pane showing
it, and the last pane starts over as a new session's home in the same channel.

**What is kept of what is not shown** (`model/retention.ts`, as data): every
conversation a pane shows, and the eight most recently active of the rest; the
newest 256 removals. Anything let go is read again when a pane shows it; a read
older than 256 removals is corrected by the next index read.

**Commands.** Everything a person or an agent does is dispatched from
`adapters/store/commands.ts`: plain actions where the state alone decides
(`openSession`, `focusPane`, `toggleSidebar`, …), thunks where a fresh id, the
time, the source or the page's measure of the panes' room is needed
(`newSession`, `openBeside`, `movePane`, `nudgePane`, `closePane`,
`sendMessage`, `approve`, `deny`, …), taking them from the store's thunk extra
argument following [dependency injection](../../design/dependency-injection.md):
`WorkspaceDependencies` carries `measure(): WorkspaceRoom | undefined`,
composed in `src/desktop/dependencies.ts` from `adapters/dom/measure.ts`. The
thunks live beside the slice rather than in `application/`, because the
architecture check keeps Redux out of `application/`, as it does for the
conversation vertical. What follows from a change whoever caused it — reading
a newly shown session's transcript, marking a shown one read — is a listener
effect, so a plain action from an agent gets it too. "Shown means read" has
that one owner: opening a session, the index's first session, and a
shown session the source marks unread again all reach it.

**One fit rule for every change of layout.** Split, open beside, drop, move
and the keyboard's nudge all go through `arrange` in `model/pane-sizing.ts`:
the new layout is taken if every pane is readable (300 × 220) in the room the
page measures — the grid's laid-out size, never a flight's transformed box —
rebalancing shares where it must; else if it is once the sidebar folds; else
not at all. A named side is that side or nothing (Split Right never silently
splits down); "beside" with no side named — ⌘-click — is to the right, else
below, else in the focused pane's place. With nothing measured, nothing new is
placed: no room is not room. Menus and the switcher ask the same rule before
they offer anything (`canOpenBeside`, `canNudge`, `previewDrop`), so an item is
disabled, or says what it will do ("Open Here…", "Go to Pane"), rather than
promise what it will not. When the panes' room changes (a resize, a side column
opening), `fitPanes` holds them to the same minimum; the side columns fold
first (`model/window-fit.ts`, asking the same `columnsFit`); where even that
cannot fit them, each pane's composer takes a compact, one-line form rather
than be clipped.

**Side columns: chosen, or folded for room** (`src/desktop/model/side-column.ts`,
shared by the workspace's sidebar and session list and by Settings' sidebar):

| column | event | next |
| --- | --- | --- |
| any | the person shows it | open, not folded (drawn at once, at any width) |
| any | the person hides it | closed, not folded |
| open | no room (window width, pane columns, a split needing it) | open, folded |
| open, folded | room again | drawn |
| closed | room or none | closed |

A folded sidebar can be revealed from the window's left edge by the one
edge-peek reveal (`useEdgePeek`) in the classic shell, both workspace layouts
and Settings; the pointer or keyboard focus inside it keeps it shown.

**What fills the content region** is workspace state (`content`: the panes,
or the Agents overview while that experiment is on), so an agent can move it
too. The sidebar's Agents entry is a place like a channel: choosing it again
keeps it; and every action that goes somewhere — a session, a channel, a
status view, a new session, another pane — goes back to the panes by one rule
(`navigated`, applied by the slice to each). The sidebar marks what is shown:
Agents while the overview is, a channel or session only while the panes are.

**Focus follows the focused pane** (`adapters/dom/focus.ts`, one mechanism):
whenever another pane takes focus — a split, ⌘N, ⌘W, ⌘1–4, ⇧⌘[ ⇧⌘], a pick in
⌘K, an agent's dispatch — the caret lands in its composer; when what held the
caret went away (an answered approval), it lands there too. Focus in a dialog
or a menu, or in a list walked with the arrow keys, is left alone. Closing ⌘K
without a pick, or Settings, gives focus back to what opened it.

**Drag and drop** is carried by the pointer (`adapters/dom/drag.ts`), not the
browser's drag, and a pane's header carries the pane, never the window (no
drag region in it). What is carried is a translucent copy of the pane itself,
cloned once when the drag begins — its conversation where it was scrolled
to, its composer and chips; a session from a list is drawn from what the
window holds of it. **While dragging, the copy belongs to the pointer**: it
keeps its size and grab offset and follows one to one, with no pull toward a
target. **The zone is the pointer's, in pixels** (`zoneAt`, `model/drop.ts`): each
side is the triangle between the pane's diagonals and the middle a size in
pixels (`centreInset`, 48–120px), so a tall narrow pane's top and bottom are
short; the sides within reach are weighed by where the pointer has headed
over the last tenth of a second (`pointerVelocity`), so a sideways sweep
across a top corner reaches the side; and the zone it is in holds until
another wins by 12px, so it does not flicker at a boundary. **The result is
shown by the layout**: over a zone, the real panes
move to where the drop would put them, only when the zone changes, and a
calm placeholder marks exactly the rect the drop will take — from
`dropOutcome`, the same outcome the drop's command commits, so nothing jumps
(`panes.test.ts` holds preview == commit for every zone; every preview rect
is held inside the grid). **On the drop, it snaps**: the copy flies from the
pointer into the placeholder's rect and hands over to the real pane. A zone
the fit rule refuses offers nothing; a session already on screen offers "Go
to Pane". Escape, or a drop elsewhere, flies the copy home as the panes go
back. Only transforms move; the zone is announced in a polite live region;
with less motion, nothing but the copy moves. Chrome and WebKit are checked
frame by frame: the copy's corner is the pointer less the grab offset, no
pane leaves the grid, and panes move one way between zone changes.

**What is typed and not sent** is product state: `composerText` in the
workspace slice, by session or new session, written by the composer (and by an
agent, `setComposerText`), so it survives a change of layout, Settings, or a
pane showing another session, and goes with its session.

The desktop gets its own store and composition root: `src/desktop/store.ts`,
with `src/desktop/dependencies.ts` building its dependencies and
`src/desktop/main.tsx` wiring dependencies → store → provider → icons → window.
A pane subscribes only to its own session, so streaming into one pane renders
one pane; `ui/panes/pane-isolation.test.tsx` holds that.

**Layouts.** Settings › Workspace › Layout chooses _three columns_, _sessions
in the sidebar_, or _classic_. **The only difference between the two
workspace layouts is how the sidebar region is composed**: which sidebar
(channels to choose from, or channels that disclose their sessions) and
whether a session list stands beside it — a `SidebarRegion`, which is all a
layout file holds. Everything else is the workspace shell's, once: the quick
switcher (⌘K, and ⌘\ to pick what opens beside, in both), one keyboard map
(`workspaceShortcuts`; ⌘F searches the list where there is one), the pane
grid and all pane behaviour, focus, drag and drop, the titlebar's controls,
the edge reveal, and fitting the side columns (`layouts.test.tsx` holds that
both layouts answer the same keys with the same parts). Where the spike's two copies of a pane
differed, one design was kept for both: the three-column pane chrome and grid
(which also gives the second layout split down and moving panes), the
second's richer transcript (step groups with diff counts, code, lists, the
live activity row and "Always Allow"), and the first's arrival motion.
**Classic stays**: it is the existing `ui/desktop-app.tsx` shell with a home and
a right panel, it shares `Home` and the composer rather than duplicating them,
and removing it is a product decision this work does not make.

Settings is a typed catalogue: categories → tabs → settings, with the search
index derived from it, in `src/desktop/settings/model/`, rendered generically
by `src/desktop/settings/ui/` (`src/desktop/settings/index.ts` is its map).
Preferences stay host-side adapters that notify other readers in the same
window — theme, icon family, workspace layout, tint, the picture in
conversations (a still sliver of the header atop each conversation pane,
`HeaderSliver`), greeting, motion
(System/Full/Reduced, carried on the root as `data-motion`, which every
duration token and script motion reads), drifting light, ⌘-click opens
beside, running sessions first — each a `storedPreference`
(`src/desktop/adapters/stored-preference.ts`). They are not Redux state,
because an agent does not dispatch "tint from picture". "Show session list"
is the workspace's own column choice, the same as ⌥⌘S. Settings is modal: the
window under it is inert while it is open, and focus goes back to what opened
it; its sidebar folds for room below a page's 420px, as the side columns do.

Every surface's titlebar — classic, both layouts, Settings — carries the
sidebar toggle, then Back and Forward (`src/desktop/ui/history-buttons.tsx`),
in one place whether the sidebar is drawn, revealed or hidden. The window has
no history yet: the two rest, and their props are the one seam history will
attach to; ⌘[ and ⌘] are reserved for them. Controls name themselves in one
tooltip for the whole window, in the window's own glass
(`src/desktop/adapters/use-window-tooltips.ts`).

**The titlebar's safe area.** Titlebar content starts at the safe area;
nothing draws under the window's controls. `--desktop-titlebar-safe-start`
is defined once, from the host's inset for its traffic lights and the width
of our control cluster (sidebar toggle, Back, Forward, and the session list's
toggle or New Session), and every titlebar-row element derives its start from
it (`styles.test.ts` holds the rules to it). Every column's head — the
session list's in both layouts, the sidebar's, Settings' page — is one
component (`src/desktop/ui/column-header.tsx`): the titlebar row, holding the
column's own action at its far end, and the column's title **inline in that
row, after the controls where they stand over the column, when it fits there
with room to breathe; on a row of its own below when it does not**
(`titlePlacement`, `src/desktop/model/column-title.ts`, decided from the
laid-out widths before the frame paints, with a 12px hold so a dragged edge
does not flip it). Where an inline title starts is the column's stylesheet's,
at the safe area wherever the controls stand over the column. When the title
changes rows it fades in at its new place and what is beneath it glides by
the row, by transform and opacity; if its column is sliding at that moment,
an inline title holds still at its place (`holdStill`) rather than ride the
slide from where the column began. A column's titlebar-row action hides at
once as the column folds and shows once it has slid in; a pane's header is
hidden while its pane flies and changes size. A frame-sampled check over
every transition that moves titlebar
content (⌘B, ⌥⌘S, the edge reveal, a resize that folds, a split, Settings,
a change of layout), in Chrome and WebKit at 1440 × 900 and 1000 × 700, finds
nothing that reads or takes a press under the controls in any frame.

**A column's edge dragged past its narrowest folds it** (`draggedEdge`, in
`src/desktop/model/side-column.ts`): it resists at its minimum, folds once
dragged 40px past it, and opens again when dragged 40px back out from where
it folded (the window's left edge, for the sidebar) — the person's own
choice, as a toggle is, moving as a toggle does.

**Settings pending the backend.** What lives outside the window is shown,
marked `pending` in the catalogue, its control disabled and its row saying
"Not available yet": open at login, show in menu bar, open to, the three
notifications, update checks and channel, keep finished sessions, default
model, thinking and fast mode, providers, agents, Nessa account, GitHub, MCP
servers, default access, remember approvals, crash reports, session history.
No control may look as if it works when it does not (`settings-view.test.tsx`).

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
motion — chosen in Settings, or the system's — sets the durations to zero
there under the root's `data-motion`, and script motion follows with no
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
model chosen there. A summary is listed by one rule whether the index
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
| the index is read | nothing yet | the workspace opens on its first session; summaries the stream already brought, if newer, are kept | the workspace says why, with "Try Again" | a removal that arrived first keeps the session out |
| the index is read again (the resync) | nothing | a session it does not list, or lists in a channel it does not list, is taken out at the revision held — its pane closes, or the last starts over; every shown conversation is read again | an open workspace stays open, as it was | a summary the stream brought while the read was on its way stays |
| a shown session's transcript read is answered with one the window cannot use (another session's, revision 0) | — | a fault, logged; the pane says why, with "Try Again" | — | — |
| an update at a revision the source could not have sent | — | let go, logged where received | — | — |

### Naming the index

The workspace's sections → channels → sessions was first "organisation",
which collides with the wire's tenant `organization`. "Directory" collides
too: a session runs in a working directory (`ConversationRuntime.workspace`,
"Working directory" in the conversation details), and the panel handles
dropped directories. "Listing" collides with the backend's
`ConversationListing` port. **WorkspaceIndex** names what the read is — everything
the source holds, at a glance — and is used nowhere else; the port method is
`index()`.

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
  the pointer holds it and no agent asks for it; it stays in the drag adapter,
  which asks the store only when the zone changes.
- **The browser's own drag and drop.** Its drag image cannot be full size or
  move, so the carried pane could not become the window it will be; a pointer
  drag can.
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

Remaining:

- **The in-memory source records a same-tick message and answer out of
  order.** A message that lets an approval go is recorded when the source
  carries it out, while an answer is on record the moment it is asked, so an
  answer given in the same tick as the message that overtakes it is listed
  first. Reproduction, against `inMemorySource` with no timers:
  `send({ sessionId: "signing", messageId: "m1", … })` then, before either
  settles, `approve("signing", "signing-import", "once", "person")`; the
  answer is refused `not-waiting`, and `audit()` reads
  `[allow-once refused not-waiting, let-go m1 taken]` — the refusal before the
  message whose effect caused it. Each entry is right on its own; the order
  across them is not causal. The fix belongs to the audit's ordering (record a
  message when it is asked too, or order by when each was carried out) and is
  owed before a durable audit adapter copies this one's shape.

