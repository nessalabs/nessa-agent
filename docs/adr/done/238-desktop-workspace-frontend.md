# 238. The desktop workspace is a Redux projection of a workspace port, with pure pane layouts and shared components

## Purpose

Turn the desktop window's workspace spike into production frontend code:
the workspace's index (sections → channels → sessions), chat panes that split and move,
two selectable layouts, Settings, and themeable icons. It extends
[0001](../done/0001-redux-toolkit-for-product-state.md)'s product state and
[0002](../done/0002-conversation-vertical-and-gateway.md)'s gateway seam to the
desktop window, so the backend can attach later by implementing one port.

- **Date:** 2026-09-27
- **Status:** accepted ([#238](https://github.com/nessalabs/nessa-agent/issues/238)). Implemented: the
  workspace vertical, both layouts and Classic, Settings, the Agents overview,
  and the verification scripts that hold them; what is left is under
  _Remaining_.

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

Since [253](253-split-panes-component.md), the split panes — the pane
layout, its sizing, drops and the drag's phases (`model/`), the drag, FLIP
and Tab order (`adapters/dom/`), and the grid (`ui/`) — live in
`src/desktop/split-panes/`, which the workspace wraps through one
`SplitPanesSource`; the paths below that name them are that module's, and
the resize edge is the desktop's (`src/desktop/ui/resize-edge.tsx`). The
behaviour set down here is unchanged by the move.

- **`model/`** holds pure values and rules:
  - the **index**'s types (`model/workspace-index.ts`: `Section`, `Channel`,
    `SessionSummary`, the session's model from the SDK catalogue) — see
    _Naming the index_ below;
  - a **pane layout** (`pane-layout.ts`) of columns of stacked panes, one
    focused, with tested operations — split, move, swap, nudge, close, even
    out, focus — and the limits of four panes and three columns;
    `pane-sizing.ts` holds the rules that need measured pixels: placements,
    the **one fit rule** (`arrange`, `fitted`: every pane at least 300 × 220)
    and edge drags held to it; `drop.ts` holds where a drag aims and what a
    drop on a pane's zone does, the one outcome a drag previews and the
    command commits; `drag.ts`, what a press becomes, event by event;
  - why the source refused (`failure.ts`, a typed reason — the sentence a
    person reads is the UI's, `ui/failure-copy.ts`), and what the window
    keeps of what it does not show (`retention.ts`);
  - session grouping ("Needs you", "Running", "Earlier"), search, and the
    new-session lifecycle, where a draft is never listed and is let go once no
    pane shows it;
  - the Agents overview's rules (`model/overview/`): what it lists and in
    what order (`agents-glance.ts`), its filter (`filter.ts`), where the
    keyboard goes (`walk.ts`), a peek's story of the turn and a request read
    from a conversation (`peek.ts`, `request.ts`).
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
    (`measure.ts`), post-layout transcript scrolling, the clock's ticks;
  - `storage/`: the overview's filter kept between launches
    (`remembered-filter.ts`, `RememberedFilter`).
- **`ui/`** holds each component once:
  - source list (both sidebars as variants of one component), session list,
    pane grid, pane, pane header, transcript, transcript heading, message, tool
    steps, approval card (its command and answers, `approval-request.tsx`,
    arranged by the card's width and shared with the Agents overview's peek), pane home (the window's own `Home`, given the
    composer's props), quick switcher, empty states, and the Agents overview
    (`ui/overview/`: its layer, list, rows, peek and reply pill);
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
revision, so an older read cannot bring the session back. **A summary may
say what is going on in its session now** (`SessionSummary.now`, optional): one
line in the source's own words — "Running the reconnect tests after adding a
token bucket" — which the overview's peek shows as it is. It belongs to the
summary and follows its revisions: a newer summary without one says there is
nothing to say, and it may lag or lead the conversation, which has its own
counter. The window never writes one and shows nothing where the source says
nothing — a line of only whitespace says nothing either, the one rule for
which is the model's (`nowLine`), the line otherwise shown as it was sent;
the in-memory source writes one at each beat of its scripts and in
its sample data; the gateway's source (#248) sends none, since the gateway's
list says no such line, and the window shows none for it. An update at a
revision the source could not have sent is let go, and logged where it is
received (`followWorkspace`, `loadWorkspace`); the reducers stay pure.

**The stream may lose updates; the index is the resync.** The port does
not ask the source to replay what a dropped connection or a late listener
missed. A read of the index is instead the source's whole list when it
answered, and so authoritative: dispatched again (`loadWorkspace`), a session
it does not list — or lists in a channel it does not list — is taken out by
the same removal path the stream's removals take, at the revision held, and
the conversation of every session on screen is read again — a read of one
already on its way is set aside unless it was asked after the index was, since
one asked before may predate what was lost. **The source says when to
resync**: it sends `{ kind: "resync" }` on its stream when it reconnects or
finds a gap, and `followWorkspace` reads the index again; a source that cannot
tell never sends one, and the window then resyncs only when it opens or the
person asks (Try Again). The in-memory source loses nothing, and never sends one. The one exception is a
session the stream brought after that read was asked, which the read may
predate: it stays, until a later read says otherwise. Each read notes what
the stream brings from when it is asked (`reading`, by the name it was asked
under), so with two reads on their way, a session brought before the second
was asked is kept by the first and taken out by the second. **Answers are
applied oldest to newest**: a read whose later-asked read has already been
applied is outrun (`outrun`), and its answer is let go unread whenever it
comes — applied, an older index would take out a session the newer one
listed, leaving a tombstone at the revision held that a later index listing
it at that same revision could not undo. A session the source
has not spoken of (revision 0) is the window's own and stays. A pane is never
left showing a session that is not there: a removal closes the pane showing
it, and the last pane starts over as a new session's home in the same channel.

**The index is taken as far as it holds together** (`consistentIndex`, `model/workspace-index.ts`): one section, channel or session per id — the first listed — each channel under a section the index lists, each session in a channel it keeps; what contradicts the rest is left out and said by id where the index is received, never opened where the sidebar cannot reach it.

**What is kept of what is not shown** (`model/retention.ts`, as data): every
conversation a pane shows; those the open Agents overview shows, up to 24,
the chosen one first, then the waiting ones most recently active first; and
the eight most recently active of the rest; the newest 256 removals. Anything
let go is read again when a pane or the overview shows it; a read older than
256 removals is corrected by the next index read. A conversation's content
let go keeps its newest revision while its session is listed
(`conversationRevisions`), so a read answered after the content went, with
an older conversation, is not taken for new.

**Commands.** Everything a person or an agent does is dispatched from
`adapters/store/commands.ts`: plain actions where the state alone decides
(`openSession`, `focusPane`, `toggleSidebar`, …), thunks where a fresh id, the
time, the source or the page's measure of the panes' room is needed
(`newSession`, `openBeside`, `movePane`, `dropSession`, `nudgePane`,
`closePane`, `sendMessage`, `approve`, `deny`, …), taking them from the
store's thunk extra argument following
[dependency injection](../../design/dependency-injection.md):
`WorkspaceDependencies` carries `measure(): PaneRoom | undefined`,
composed in `src/desktop/dependencies.ts` from `adapters/dom/measure.ts`.
An agent drives the window as a person does, so `movePane` and `dropSession`
measure the room as they run, like every other command that places a pane.
The drag alone is given the room it read as its press began: it previews
with `dropOutcome` of the layout its source reads and commits with
`commitDrop`, through the split panes' source
(`adapters/store/split-panes-source.ts`), both in that room, and the commit
is not a command an agent is meant to dispatch. The
thunks live beside the slice rather than in `application/`, because the
architecture check keeps Redux out of `application/`, as it does for the
conversation vertical. What follows from a change whoever caused it — reading
a newly shown session's transcript, marking a shown one read — is a listener
effect, so a plain action from an agent gets it too. "Shown means read" has
that one owner: opening a session, the index's first session, and a
shown session the source marks unread again all reach it.

**One fit rule for every change of layout.** Split, open beside, drop, move
and the keyboard's nudge all go through `arrange` in `split-panes/model/pane-sizing.ts`:
the new layout is taken if every pane is readable (300 × 220) in the room the
page measures — the grid's laid-out size, never a flight's transformed box —
rebalancing shares where it must; else if it is once the sidebar folds; else
not at all. A named side is that side or nothing (Split Right never silently
splits down); "beside" with no side named — ⌘-click — is to the right, else
below, else in the focused pane's place. With nothing measured, nothing new is
placed: no room is not room. Menus and the switcher ask the same rule before
they offer anything (`canOpenBeside`, `canNudge`), so an item is
disabled, or says what it will do ("Open Here…", "Go to Pane"), rather than
promise what it will not. When the panes' room changes (a resize, a side column
opening), `fitPanes` holds them to the same minimum; the side columns fold
first (`model/window-fit.ts`, asking the same `columnsFit`); where even that
cannot fit them, each pane's composer takes a compact, one-line form rather
than be clipped.

**The night scene's layout work stays with its visible representation** (#616).
`ui/night-scene.tsx` takes its content height from ResizeObserver rather than
forcing another computed-height read. It remains hidden until measured. A static
conversation header renders only the rain rows inside its existing window clip
and one steam pattern, at their resting offsets in either motion mode. Animated
homes retain the full tiling copies. Browser comparisons of the original and
compiled fixed headers hold identical pixels in both engines, layouts and motion
modes; the static scene retains 30,998 rather than 108,814 characters. This bounds
redundant text, not every source of frame delay.

The home's shape watcher is registered in the layout commit before paint and
takes its initial CSS shape from the first resize observation. Initial appearance
plays no settling; a later observed shape change does. An empty composer supplies
one logical draft line to the existing page-mode decision without a synchronous
geometry query; placeholders and minimum field height do not own draft length.
Nonempty drafts, including whitespace, still use measured lines. These changes
remove eager reads but do not establish the whole desktop's frame budget.

**A new session's home takes its pane's shape** (`ui/panes/conversation.css`,
issue #285). In a pane at least 640px each way the home is the window's own:
the scene, "Working late?" and the composer's card. In a smaller pane — split
beside or below another, or in a short window — it takes a conversation's
shape: the header shrinks to a band across the top of the pane — the
picture or scene, a share of the pane's height, with its Customize control
(issue #320; until then the scene stepped aside, and with it the only place
the picture was chosen) — the greeting sits under it at the pane's left at a
conversation title's size, and the composer docks at the pane's foot as a
conversation's does, with the greeting turned off in Settings too. Every
pane's "…" menu also offers Choose Header Picture… and, with a picture, Use
Night Scene, through the same picker Customize uses. A home's picture, in
either shape, reaches up behind the pane's title row to the pane's top edge,
fading in there as a conversation's band does, except in the window's corner
with nothing beside it, where the title row is the window's controls' and the
picture stays below it. The greeting follows the hour of the window's clock
(`model/greeting.ts`): good morning, afternoon or evening, and "Working
late?" only from 10pm to 5am. The pane's own size decides, by the container query the pane
already is (`workspace-pane`), never the window's. It is one composer in both
shapes: the home holds it in the dock's box (`Home`'s `composerClassName`),
and the dock's rules, and a title's type, ask for a `workspace-docked`
container around them, which a conversation always is and a home only while
small, so the dock and the title are each written once. Nothing is
remounted, so the draft, the caret, the model chosen and a long draft's page
stay as they were across the threshold; the page still opens in either
shape. Sending the first message replaces the home directly when the session
is listed; there is no overlapping home/composer clone or geometry-driven
arrival animation. The existing workspace focus owner follows the removed
field and respects focus the person moved elsewhere. Crossing the threshold, as a pane is resized or
split, the greeting and composer settle into their new places
(`--desktop-slow`, opacity and transform only) under the panes' own flight;
with less motion they are simply there. Only a crossing plays it: the
stylesheet names each shape's settling in `--workspace-home-settle`, and the
pane marks its home `data-reshaped` when that name changes under it
(`adapters/dom/home-shape.ts`), so a home that appears, in either shape,
plays only its own entrance, as it did before. Focus on the scene's controls
(Customize, the picture's framing) when the scene steps aside goes to the
focused pane's composer, as focus that falls away from anything in a pane
does (`adapters/dom/focus.ts`; a widget's body in a widget's pane, ADR 326); a framing being adjusted waits, as it was,
for the scene to come back.

`responsive.mjs --only home-shape` measures, in Chromium and WebKit: the
docked composer's distance from the foot, inset and height against the
conversation's beside it, with the greeting on and off; the scene hidden and
the header band, its Customize control, and the greeting's place and type in
a small pane; the scene and the card in a large one; that a home appearing, large or small, plays no settling, and
that each crossing plays one, by opacity and transform alone, and none with
less motion; and, across each crossing, the same field, its draft, the model
chosen and an open page — and, where the crossing is a resize that moves
nothing else, the caret's focus and position. `focus.mjs`
(`focus-home-scene`) measures that focus on Customize stays there as the home
becomes small, and `responsive.mjs` (`header-picture`) that a pane's menu
chooses the picture, takes it back, and says why a file was refused.
`focus.mjs` measures the first-message handoff, including one composer per
pane and the reply caret; no separate arrival animation remains.

The first-message and transcript handoff has one focus owner and one scroll
owner (`focus.ts`, `transcript.tsx`):

| event | view and effect |
| --- | --- |
| first send pending | draft home remains while the source creates the session |
| source lists the session | home is replaced by the conversation; the focus adapter follows a lost field after its target paint |
| person took focus elsewhere | keep that focus rather than reclaiming it on mount |
| transcript or viewport layout changes while pinned | ResizeObserver pins the tail after layout, before paint |
| person scrolls away | resize does not change their scroll position |
| person returns to the tail | following layout changes keep them pinned |
| view removed | disconnect observers and reject their late callbacks |

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
edge-peek reveal in the classic shell, both workspace layouts and Settings;
the pointer or keyboard focus inside it keeps it shown. Its states, and what
a held pointer button does to them, are one table in
`src/desktop/model/edge-peek.ts` (`stepEdgePeek`); `useEdgePeek` only runs
its clock and sends it the page's events. **A press freezes it**: a reveal or
a hide on its way is cancelled as the button goes down, and the peek stays
exactly as the press found it — entering and leaving change nothing — until
the release, when where the pointer then is decides. A release the page
missed (the button let go outside the window) is taken as one at the next
enter or leave with no button held, or when the window loses focus, so a
lost release never leaves it frozen.

The modern workspace sidebar keeps its tint, rim and resting shadow without a
live backdrop blur. The ambient light is already softened; resampling the
changing pane region through the sidebar adds paint work to pane and overview
transitions. Its edge peek uses an opaque tinted fill so the underlying
transcript stays out of the revealed list.

Opening the overview paints the peek only once, and registers pending focus
before the following row paint so reads precede that frame's DOM writes.
Keyboard walking uses the rows currently drawn. A reply opened while the
list is arriving retains its expanded row when the source moves that session
to another group; the normal prefix continues growing one row per frame.
The browser's row-movement and Show All probes wait for the list's published
completion marker before manipulating the completed list.

**What fills the content region** is workspace state (`content`: the panes,
or the Agents overview), so an agent can move it too. The overview is part of
the workspace (`ui/overview/`, `model/overview/`): its selection and filter
are slice state (`selectInOverview`, `filterOverview`; a session not listed is
not selected, and while the open overview lists any, one is chosen — the first
it lists, on opening or once the chosen one is gone, `keepOverviewChoice` — so
the session its peek shows is the one the window reads for it), and the filter
is kept between launches by composition
(`RememberedFilter`, read once into the store's first state and written by an
effect on each change — storage never speaks for it while the window is
open). **The overview is always offered**: the sidebar's "Agents" entry, at
its top in both workspace layouts, and ⌘0 open it, with no preference to
turn on (`layouts.test.tsx`). It began as a preview behind a setting; that
setting is gone, and with it the sidebar's separate "Needs you" and "Running"
views, which the overview holds — the session list shows one channel's
sessions (`SessionView`). Previews are offered under Settings ›
Advanced › Experimental. The subagents preview is on offer there; the page
says it has nothing to try only while none is. The overview's groups are
shown only while they hold something, "Needs you" too — nothing waiting
leaves no empty section — and its one quiet line ("Nothing needs you") shows
only when it lists nothing at all (`overview.test.tsx`). **Each count in its
header shows its group alone** (see _Showing one group alone_ below): a toggle
button, pressed while its group is shown alone, in a group labelled "Show
only"; chosen again it shows every group. The counts are always what the
filter lets through, whichever group is shown and whichever session is looked
at — one the filter keeps out stays listed while looked at, and is never
counted — so another group is one click away on the same line, and the chosen
one stays in the line at nought ("0 finished") so it can be let go where it
was chosen. A group shown alone that lists nothing of its own says so in one
quiet line at the top of the list ("Nothing has finished"), even while the
session looked at, having left it, is still listed beneath under its new
heading (`quietOf`). A count the keyboard is on that goes from the line — let
go at nought, or emptied by the source — gives the keyboard to the list, on
its current row or at its top, never to the page. The line under
the list — "N more sessions outside this view · Show All" — counts only what
the filter keeps out *of the group shown*, and Show All chooses every session
at any time under any tag, so it always lists what it counted. Its header — the title, its counts and the filter — holds its place; only the list under it scrolls, fading out at its top edge. The entry and ⌘0 are a place like a channel:
choosing it again keeps it, and Escape leaves it whenever it is open —
wherever the keyboard is, even before the keyboard has landed on its row —
but for Escape in a menu or dialog over it, which is theirs, Escape in a
reply pill, which takes the keyboard back to the list, and, while one group
is shown alone, the first Escape, which shows every group again (the next
leaves); every action that goes
somewhere — a session, a channel, a new session, another pane,
or the focused pane asked for by name — goes back to the panes, even when it
finds the window already there; and one that changes the panes a person asked
to change — closing, moving, evening them out, resizing — goes back only when
it changed them, so no pane changes unseen beneath the overview and a move at
the edge leaves it where it is (`navigated`, applied by the slice). The
overview is drawn in a layer over the session list and the panes, which stay
laid out beneath it, unseen and out of reach: the room a pane command
measures is the panes' own whether the overview is open or not, and the
layer's width — whether the peek fits beside the list — is known before it
opens, so its first frame is laid out once. The keyboard lands on the
current row only after that row is painted. The overview makes its content
group inert; a folded list keeps its own inert state independently. Sidebar
current-page metadata follows this workspace's completed list paint, with
its subscription and pending frames owned by that workspace and root.
Left — by Escape, or by any command that brings the panes back,
one that changed nothing else included — it goes back to the focused pane's
composer (its body, for a pane showing a widget: ADR 326); an answer given in it — by key or click — moves it to the next
request. ⌘R puts the caret in the reply pill of the session the keyboard is
on: the peek it sits in is drawn at once for it (not a step behind, as the
list's walk draws it), so what is typed next lands in the pill. **The caret
stays with the session's pill**: sending a reply that moves its row to
another group (Needs you → Working), which draws its pill anew, gives the
caret to the new pill; the caret leaves the pill only when the person moves
it (Escape, a click, Tab). **One press answers one request**: a held key's repeats answer and
open nothing, in a row or on an answer's button, and a press made within
250ms of the press that answered — each timed as it was made — is not taken
(`takesAnswerKey`); a held
Return in a composer sends once. The sidebar
marks what is shown: Agents while the overview is, a channel or session only
while the panes are.

**What the overview shows is on screen** (`overviewShows`, `onScreen`, one
rule in `application/workspace-state.ts`): the session chosen in it, and each
session its filter lists waiting on the person. Its approvals are answered by
the same `approve` and `deny` as a pane's card, so there is one answer per
approval whichever surface gave it — a second, from either, is `answering` —
and refused stays apart from unknown. Its conversations are read and kept as
a pane's are (bounded, above), and a failed read is kept while it is shown. A
peek is not reading the session: only a pane marks it read. With a group
shown alone, what waits on the person is on screen only when that group is
Needs you (`inGroup`, in the same rule): a request the group leaves out is
neither read for the overview nor answered from it (`not-asked`).

**The peek tells the turn's story** (`model/overview/peek.ts`,
`ui/overview/session-peek.tsx`). Under its title: where the session stands
(waiting for you, working, finished) and, when the source says one, its line
of what is going on (`SessionSummary.now`). Then, top to bottom: the person's
latest message — the last message of theirs in the source's conversation —
then what the agent did and said since, in order, drawn by the
conversation's own `Message` (its steps, edits, code and words, as a pane
draws them), then what it is doing now (the live row), then the request in
full where one waits. Nothing is summarised by the window, but a peek is a
glance drawn afresh for each session the keyboard lands on, so it is
bounded: it draws the turn's latest parts (`peekParts`, 24 — steps,
paragraphs, code, lists; the oldest message drawn cut to its latest), and
where the turn holds more, one line under the person's message says so
("Earlier in this turn · Open"), opening the session, which shows it all.
Beside the list, a long turn scrolls within the peek with the reply pill
held at its foot; beneath a row it scrolls within its own bounded height,
taking the keyboard to do it (a stop of its own, so WebKit reaches it; its
arrows, Home and End scroll it rather than walk the list), its scrollbar
shown and its foot fading while there is more below.

### Showing one group alone

The overview's state holds the group shown alone (`OverviewState.group`:
`needsYou`, `working`, `finished`, `earlier`, or `null` for every group),
beside its filter and chosen session, and `showOverviewGroup` is the one
command that changes it; a count's toggle is the view dispatching it (its
group, or `null` when it is already shown). A session's group is where it
stands (`groupOf`: waiting; running; idle and unread; idle and seen), the one
rule the list, the counts, the footer's figure and `overviewShows` read: the
glance files each session once, by `groupOf`, and draws its lists and counts
from that one pass. The group is kept while
the window lives — closing and opening the overview keeps it, as it keeps the
chosen session — and **never between launches**: only the filter is a
standing choice worth remembering (`RememberedFilter`); a group is a
momentary narrowing, and a window opening on yesterday's "Finished" would
hide a request waiting on the person. One row, at least one test
(`adapters/store/commands.test.ts` › _the overview shows one group alone_,
`model/overview/agents-glance.test.ts` › _one group shown alone_,
`ui/overview/overview.test.tsx` › _the counts show one group alone_).

| State | Event | Next | What the person sees |
| --- | --- | --- | --- |
| every group | a count chosen (click, Space or Return on it) | that group | only that group listed; its count pressed; every count still in the line; if the chosen session is outside the group, the group's first is chosen (`keepOverviewChoice`), and nothing when it lists none |
| a group | its count chosen again, or `showOverviewGroup(null)` | every group | every group listed; the chosen session kept |
| a group | another count chosen | that group | as for the first choice |
| a group | the same group asked for again | unchanged | nothing changes (the same state) |
| a group | Escape, outside a menu, dialog or reply pill | every group | the overview stays open |
| every group | Escape | — | the overview is left (as before) |
| a group | the filter changes | the same group | its counts and list under the new filter |
| a group | the chosen session moves to another group while looked at | the same group | it stays listed, under its new heading, until the person moves on (`looking`), as under the filter |
| a group | its last session moves to another group | the same group | its count at nought, pressed; "Nothing …" at the top of the list (`quietOf`); that session — the one looked at, as a group's first is chosen — stays chosen and listed beneath the line under its new heading, until the person moves on (`looking`) |
| a group | its last session is removed | the same group | its count at nought, pressed; "Nothing …"; nothing chosen |
| any | the session looked at is one the filter keeps out | unchanged | it stays listed while looked at; no count counts it |
| a group at nought | its count let go (click, Space, Return, or Escape) while the keyboard is on it | every group | the count leaves the line; the keyboard goes to the list — its current row, or its top |
| any | a count the keyboard is on drops to nought, its group not shown alone | unchanged | the count leaves the line; the keyboard goes to the list — its current row, or its top |
| a group | the overview closes and opens again | the same group | as it was |
| any | a launch | every group | the filter as kept; no group |
| Needs you not shown | an answer asked for a waiting session not chosen | unchanged | `not-asked`; it is not read for the overview |
| Needs you | a request answered from its row | unchanged | it settles in place; held rows are listed only while the waiting are shown |

**Focus follows the focused pane** (`adapters/dom/focus.ts`, one mechanism):
whenever another pane takes focus — a split, ⌘N, ⌘W, ⌘1–4, ⇧⌘[ ⇧⌘], a pick in
⌘K, an agent's dispatch, or the panes coming back from under the Agents
overview — the caret lands in its composer; when what held the
caret went away (an answered approval), it lands there too. Focus in a dialog
or a menu, or in a list walked with the arrow keys, is left alone. Closing ⌘K
without a pick, or Settings, gives focus back to what opened it. [ADR
326](../todo/326-widgets.md) amends both halves of this rule: a pane showing a
widget takes the caret in its body rather than a composer, and while the
window shows a widget, focus that falls away lands in that widget's body.

Programmatic pending focus uses the shared `focusAfterPaint` controller; pane
focus resolves only the foreground widget or eligible pane body. The pane
caret waits for its target's first layout to paint before landing
(`adapters/dom/focus.ts`, `focus.test.tsx`):

| pending focus | next frame | result |
| --- | --- | --- |
| target absent, inert, or pane body waiting for layout | look again, up to 30 attempts | no geometry or focus read |
| target found | let this frame paint | retain the target identity |
| target painted, still current | focus with `preventScroll` | finish |
| target replaced | find the current target | wait for its paint |
| modal or inert target, or deliberate focus elsewhere | leave the pending landing | keep that focus |
| cancelled or owner removed | cancel the pending frame | leave focus alone |

Transcript heading visibility comes from its IntersectionObserver, including
its first report; mounting does not read rectangles to predict the observer.
The pane header stays visible until the observer confirms that the transcript
heading is in view (`transcript.test.tsx`), including a conversation opened at
its latest message.

**Drag and drop** is carried by the pointer (`split-panes/adapters/dom/drag.ts`), not the
browser's drag, and a pane's header carries the pane, never the window (no
drag region in it). What a press becomes is one pure state machine,
`split-panes/model/drag.ts` (`stepDrag`); the adapter sends it every event and draws the
phase it answers with. What the page made for a press or a drag is held in
the phase itself, so there is one answer to whether a drag is live. Its
rules, few on purpose (_Keep patching the drag_, below): **one aim point, the
pointer**; **nothing is read again while carrying — a change ends the drag
instead**; **only panes a person can see are targets — never a side
column**; **only a drop that was previewed commits**; **only the primary
button carries**; and **the zone settles at rest**.

| phase | event | next | what the window does |
| --- | --- | --- | --- |
| idle | press of the primary button on a pane's header or a session's row, not on a control inside it | pressed (no copy yet) | nothing in the press's own frame; in a task once it has painted, the copy is made unseen and the page read once — the grid, the room (`measure`), each pane's box and parts, the side columns drawn, and whether panes can be seen at all |
| idle | any other press: another button, a control inside a header | idle | the press is not the drag's: nothing is made or held for it |
| pressed (no copy yet) | the copy made (`ready`) | pressed (with its copy) | — |
| pressed (no copy yet) | the copy cannot be made: no panes laid out | idle | — |
| pressed (no copy yet) | move | pressed | nothing: a press becomes a drag only once its copy is made, so no event reads the page |
| pressed | a move of another pointer, or of its own under 4px (`liftDistance`) | pressed | — |
| pressed (with its copy) | move 4px or more | carrying (aim: none yet) | the copy is shown under the pointer at the carried pane's own size, and glides (`--desktop-base`, transform only) until its centre is under the pointer, where it stays; the zone waits for the next frame |
| pressed | release of its pointer | idle | a click; what was made goes |
| pressed | Escape; any other key but a lone modifier; another button (a move whose `buttons` is not the primary alone — a chorded button reaches the page as a move, never a press); `pointercancel`; the window's blur; a change (below) | idle | the press is let go: no later move can start a drag |
| carrying | move of its pointer | carrying | the copy moves with the pointer, one to one; the zone is decided from the pointer (below); the panes, the placeholder and the copy's shape change only when the zone does, a frame later (the preview): each pane takes the rect the drop would give it, and the copy the placeholder's size about the pointer — or, with nothing offered, its own (_What the copy and the panes are drawn at_, below) |
| carrying | a move of another pointer, or to where it already is | carrying | — (resting is not restarted) |
| carrying | no move for 150ms (`restAfter`, `still`) | carrying | the pointer's heading has aged out: the zone is decided again as at rest, and previewed a frame later |
| carrying | release of its pointer while the page shows a zone that offers something (`dropOutcome`) | dropping | the zone shown is committed — what the person saw, never one decided again at the release, so a heading that ages out as the button lifts cancels nothing — by `commitDrop`, in the room read as the press began, and the copy flies into its rect — already its shape, so it only moves, unless the release came mid-change; the click the release makes is swallowed |
| carrying | release of its pointer with no zone shown that offers something: before any preview was shown (a flick), off the grid, over a side column, the carried pane's own place, a side the room refuses, no pane in sight (under the overview, or under a widget in the window: [ADR 326](../todo/326-widgets.md)) | cancelling (home) | the copy flies home, taking its own size again, as the panes go back to theirs; the click is swallowed |
| carrying | release of another pointer | carrying | — (no click is swallowed for it) |
| carrying | Escape | cancelling (home) | as above; Escape goes no further |
| carrying | another button, `lostpointercapture`, `pointercancel`, the window's blur | cancelling (home) | as above; with the press let go, a later move starts nothing |
| carrying | a change: any key but Escape or a lone modifier (a command — ⌘W, ⌘0, ⌘B, ⌘,, ⌘K, an arrow — ends the drag before it runs); a resize; the panes, the content view or the side columns changing in the store (an agent's dispatch, a session removed); the carried session no longer listed; the window going inert under Settings, seen as it happens (a `MutationObserver` on `inert`). A layout switch unmounts the shell, and the drag with it, taking everything it drew | cancelling (at once) | the copy and the preview go at once, nothing read again, and the change plays as it would with no drag; a key that reaches the window while it is inert is never the drag's, so Settings keeps its Escape |
| dropping | its flight finishes before the deferred commit/preview release | dropping | retain the resource owner until both finish, including reduced motion (#616) |
| dropping | the drop's own change to the panes | dropping | (it is the drop) |
| cancelling (home) | a change of room/view before its return flight ends | cancelling (at once) | remove the retained copy and preview immediately, including a lost pointer followed by resize; keep the resource owner through the flight (#616) |
| dropping, cancelling | the owned copy's flight ends (`landed`, with its made resources) | idle | an obsolete flight's settlement leaves the current drag and its preview unchanged (#616) |
| any phase with retained resources | adapter unmounts | idle | cancel owned flights, remove their copy/preview, and fence their later callbacks before cleanup effects (#616) |
| dropping, cancelling | anything else, a press included | unchanged | a press while the copy still flies starts nothing |

What the copy and the panes are drawn at (`copyShape`, `split-panes/model/drag.ts`; the
drawing is `split-panes/adapters/dom/drag.ts`):

| while carrying | the copy | a pane the drop would move or resize |
| --- | --- | --- |
| no zone shown: before the first preview, off the grid, over a side column, its own place, a side the room refuses, under the overview | the carried pane's own size, its centre on the pointer | where and as big as it is |
| a zone shown that offers something | the placeholder's size — the slot it would land in — its centre still on the pointer, never the slot's place | the rect the drop gives it (`dropOutcome`, the same the drop commits), what it holds at its own size, as a pane that shape would place it — its transcript held to the top, centred across as it grows and held left as it shrinks, its composer to the foot, its header to the top left — and cut to the shape where smaller, its header past the window's controls where it would rest in the corner (`data-drag-corner`) and not until it has moved out from under them where it would leave |
| the zone changes, even mid-change | from the size it is drawn at now to the new one, `--desktop-base` on `--desktop-ease`, one way, never past it | from where it is drawn now, the same |
| released onto the zone shown | flies from the pointer into the placeholder's rect | lands where it was previewed: `FlipScope` lets the preview go and finds nothing to fly |
| cancelled home | flies home, back to its own size | back to its own place and size |
| less motion | changes size at once | takes the rect the drop gives it at once, never gliding, and is let go as the drop lays it out there (`FlipScope`, which flies nothing); the placeholder marks the slot. Left where it is, a swap would show only the placeholder, under the copy: nothing would say the other pane moves (#286) |

Neither the copy nor a pane is ever drawn stretched: each grows or shrinks
into its shape by a scale its content undoes at each of twelve steps (as a
flight does, `flip.tsx`), so at every step its words are drawn at their own
size, and at rest the box is exactly the rect it would take. The two differ
in what the content is laid out at, chosen by measurement:

- **The copy is laid out at the slot's size, once per zone change**, so its
  title reads as the pane will; at rest nothing in it is scaled. It is the
  pane's chrome in a box of its own (`contain: strict`). The conversation
  stays in the copy and is not painted: laying that body out on each shape
  change misses the frame budget.
- **A pane keeps its layout and is cut to its would-be shape**: its box
  takes the rect by transform (`overflow: hidden` clips it) and its content,
  scaled back, stays at the size it has, placed as a pane that shape would
  place it — its transcript held to the top, centred across where the pane
  grows, as a wider pane centres its column, and held left where it
  shrinks, so a line loses its end, never its start (centred both ways, a
  narrower pane cut its title's first words); its composer held to the
  foot, as a taller pane docks it (held to the top with the rest, it
  floated mid-pane), and what is held to the top cut where the composer
  begins, by as much as the pane is shorter, so it never runs under it; a
  new session's home held to the middle — a small one's docked composer
  with it, not to the foot; its header
  held to the top left, so it steps
  past the window's controls where it rests in the corner and nothing of it
  passes under them. Centring down as well was tried: a pane shorter than
  before then drew its conversation's heading into the titlebar row, under
  the controls (`copy-takes-slot-shape`'s safe-area sampling caught it).
  Laying a pane out at its would-be size instead was built and measured: a
  pane is a size container (`container: workspace-pane / size`), so a change
  of its size restyles and lays out everything it holds — traced at 20–42ms
  of layout per zone change at 4×, frames of 67–83ms, over the 50ms budget
  even with its conversation held at its width; turning the container off
  during a drag cut it to 26ms but would drop its container queries and
  jump at the drop.

A box's change of shape and its content's undoing of it start at one time
on the document's clock (`startTime`), so an engine never starts one a
frame before the other and draws the content stretched for that frame.

The other ways were weighed for the copy too: a uniform scale clipped to the
slot (`clip-path: inset()`) never distorts but draws a tall pane's words
three times their size in a wide slot, and a box cut to the slot with its
content at the carried shape shows empty space where the slot is bigger;
animating `width` and `height` lays the copy out every frame. The copy's
shape is `made.drawing.shape`, and a pane's the preview's own motion —
neither is read back.

The copy is drawn in a layer that begins below the titlebar row
(`.split-panes-layer`), so nothing carried is ever painted under the
window's controls, whatever it passes over. The folded sidebar revealed from
the window's edge (the peek) neither shows nor hides while a pointer button
is held — a press freezes it (_Side columns_, above) — so the side columns
the press read are the ones there until the release. A peek still sliding in
as the press reads it is taken as covering where it is sliding to, its
settled rect, never the part of the way it has come.

Where the pointer is decides the zone (`aimAt`, `split-panes/model/drop.ts`):

| the pointer | zone |
| --- | --- |
| anywhere, while the Agents overview (or Settings) covers the panes | none: no zone, no placeholder, a release changes nothing |
| over a side column, docked or revealed from the edge over the panes (`Targets.covered`) | none |
| off the grid, or out of the window | none |
| in a gutter between panes | the nearest pane's, the pointer held to its edge (`paneAt`) |
| over a pane | each side reaches a third of the way in, held to 90–300px and never past the middle (`edgeReach`); the middle is what the sides leave, and where two reaches meet the diagonal between them decides (`zoneAt`) |
| over a pane, heading mostly toward a side (the last tenth of a second, `pointerVelocity`) | that side reaches 1.4 times further, so a drag down a tall pane is "below" by two-thirds of the way |
| over a pane, heading plainly along one axis | the sides across it are reached only within 16px of their edge — unless the pointer is already in one — so a sideways drag near a tall narrow pane's top moves beside it, never above |
| over a pane, the pointer 24px or more (`travelled`) from where the drag was pressed | the sides the way it has come reaches as a heading does, 1.4 times further: a pane moved up is moved up, one moved sideways beside — at rest too |
| over a pane, still for 150ms | as at rest: the heading has aged out |
| over the zone it is already in, resting or settling (under 0.25px/ms, `settlingSpeed`) | its side keeps the 1.4 times reach a heading gives, so coming to rest, or nudging into place, never swings the panes back; sweeping, only a heading toward a side reaches further |
| over the zone it is already in | it holds until another wins by 12px, so it does not flicker at a boundary |
| near a zone a drop would change nothing on — the side of a pane the carried one already sits on — or one the room refuses (`refusedZones`, read as the press begins) | never that zone: the zone beside it is decided as though it were not there, so moving a pane from above another to its right edge offers "right" at once, not "above" |
| released | the zone shown then, as above: a release decides nothing again |
| over the carried pane's own place | none, every zone refused: let go there, it goes home |

The pointer is the aim because the copy's centre is under it: what the eye
tracks and what aims are one point, so the middle of a tall pane — Swap — is
where the copy's middle is. **The result is shown by the layout**: over a
zone, the real panes move to where the drop would put them, and a calm
placeholder marks exactly the rect the drop will take — from `dropOutcome`,
the same outcome the drop's command commits, so nothing jumps
(`panes.test.ts` holds preview == commit for every zone; every preview rect is
held inside the grid). A zone the fit rule refuses offers nothing; a session
already on screen offers "Go to Pane". What is carried is an opaque copy
of the pane's title and chrome, in a box of the shape it would land in
(`contain: strict`). Its conversation is in the copy and not painted.
A session from a list is drawn from what the window holds of it. Nothing of
the page is read after the press's frame: where a preview has drawn a pane is
known from the preview's own motion, and the preview and
the drop's command are given the room read as the press began (a resize or a
side column changing ends the drag, so it is still the room at the release),
so beginning, previewing, dropping and letting go only write. What is read is
read in a task after the press's frame has painted; a page written to in
between is laid out by that read, once. That is accepted, not avoided: in
Chrome at 4× CPU throttle, four panes, the task took a median 1.6ms (max
3.0ms) on a clean page and 3.0ms (max 4.4ms) after a streamed paragraph was
written into every pane in between, six runs each — well inside a frame. A
press on what can be carried, and the drag it becomes, select nothing
(`selectstart`) and leave nothing selected; while carrying, one element over the page holds the
grabbing hand. Only transforms move — but the copy, laid out once per zone
change at the slot's size, as above; the zone is announced in a polite live
region; with less motion, nothing glides: the copy and the panes take their shapes at once. `split-panes/model/drag.test.ts`
has a test for each row above, `split-panes/adapters/dom/drag.test.tsx` for
what the drag asks of any host, and `adapters/dom/split-panes-drag.test.tsx` for the page's
side of them (the click a release makes, a control's own press, the carried
session unlisted, Settings' Escape, nothing read after the press, the copy
laid out at its slot and the panes drawn at their would-be rects, never
stretched on the way);
`verification/desktop/scripts/drag.mjs` checks the rest in Chrome and WebKit,
frame by frame — the copy's centre stays on the pointer, no pane leaves the
grid or the window, panes move one way between zone changes, the zone settles
at rest, nothing is a target under the overview or over the revealed sidebar
(which stays revealed), a flick or a chord drops nothing, nothing carried is
painted under the controls, a resize or a command mid-drag leaves nothing
lifted, a lost pointer starts nothing again, the copy takes the slot's shape
about the pointer and its own with no zone, changing size one way
(`copy-takes-slot-shape`), and every pane is previewed at the rect it lands
at with no title drawn stretched (`preview-panes-take-shape`).

**What is typed and not sent** is product state: `composerText` in the
workspace slice, by session or new session, written by the composer (and by an
agent, `setComposerText`), so it survives a change of layout, Settings, or a
pane showing another session, and goes with its session. Sent by the person
(`sendMessage` with the initiator `"person"`), it goes — it is what was sent
(`messageSent`); a message an agent sends is its own, and what the person is
typing stays; nothing sent (`not-asked`), it stays.

**The model a composer shows** is the one its next message is sent with:
`modelForNextTurn`, selected by the pane (`selectNextModel`). The composer
holds no model of its own — it is given one and told of a pick, as it is given
its text — so a model an agent chooses (`chooseModel`), or a newer summary
from the source that moves the session, is what the picker shows while it
stays mounted, and what `sendMessage` sends
(`ui/panes/conversation.test.tsx`). The classic shell's home, which has no
session, holds its own. A model the catalogue does not have shows as none
chosen, never as another (`ui/composer.test.tsx`).

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
the edge reveal, and fitting the side columns (`layouts.test.tsx` presses
every key of the shared map in both layouts and holds that each does the
same, but for ⌥⌘S and ⌘F where one has no session list). Where the spike's two copies of a pane
differed, one design was kept for both: the three-column pane chrome and grid
(which also gives the second layout split down and moving panes), the
second's richer transcript (step groups with diff counts, code, lists, the
live activity row and "Always Allow"), and the first-message handoff.
**Classic stays**: it is the existing `ui/desktop-app.tsx` shell with a home and
a right panel, it shares `Home` and the composer rather than duplicating them,
and removing it is a product decision this work does not make.

Settings is a typed catalogue: categories → tabs → settings, with the search
index derived from it, in `src/desktop/settings/model/`, rendered generically
by `src/desktop/settings/ui/` (`src/desktop/settings/index.ts` is its map).
A category's page shows its tabs when it has several, or one named other than
the category (`showsTabs`): Advanced, just before About, holds one tab,
Experimental, the home of previews. The subagents preview is offered there
as a switch; the page shows a calm empty state and no control only while
none is on offer. Search finds the tab as "advanced",
"experimental", "labs" or "preview" (`settings-catalogue.test.ts`,
`settings-view.test.tsx`).
Preferences stay host-side adapters that notify other readers in the same
window — theme, icon family, workspace layout, tint, the picture in
conversations (a still sliver of the header atop each conversation pane,
`HeaderSliver`), greeting, motion
(System/Full/Reduced, carried on the root as `data-motion`, which every
duration token and script motion reads), drifting light, ⌘-click opens
beside, running sessions first — each a `storedPreference`
(`src/desktop/adapters/stored-preference.ts`). They are not Redux state,
because an agent does not dispatch "tint from picture". "Show session list"
is the workspace's own column choice, the same as ⌥⌘S. It and "Keep running
sessions at the top" apply only where there is a session list — three
columns — so the catalogue says which layout a setting applies in
(`layout`), and in any other its control is disabled and its row says
"Three columns only" (`settings-view.test.tsx`, per layout). Settings is modal: the
window under it is inert while it is open, and focus goes back to what opened
it; its sidebar folds for room below a page's 420px, as the side columns do,
refitted in the render that sees the width; its room changes at once and the
page slides into place by transform (`slideFrom`), its inline title held
still at its place meanwhile (`holdStill`), whatever row it was in.

Every surface's titlebar — classic, both layouts, Settings — carries the
sidebar toggle, then Back and Forward (`src/desktop/ui/history-buttons.tsx`),
in one place whether the sidebar is drawn, revealed or hidden. The window has
no history yet: the two rest, and their props are the one seam history will
attach to; ⌘[ and ⌘] are reserved for them. Controls name themselves in one
tooltip for the whole window, in the window's own glass
(`src/desktop/adapters/use-window-tooltips.ts`).

**The titlebar's safe area.** Titlebar content starts at the safe area;
nothing is painted under the window's controls — no text, code, picture,
drawing or decorative layer's image. `--desktop-titlebar-safe-start`
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
hidden while its pane flies and changes size. A pane's header is its titlebar
row, and what follows it begins below the titlebar's height wherever the pane
is, so a scrolled transcript or a narrow pane's heading never passes under
the controls; a conversation's picture band begins after them in the corner
pane, and waits out of sight while panes travel. A frame-sampled check over
every transition that moves titlebar content (⌘B, ⌥⌘S, the edge reveal, a
resize that folds, a split, a new session, Settings, a change of layout), a
scrolled transcript, and widths down to 560, in Chrome and WebKit at 1440 ×
900 and 1000 × 700, finds nothing painted under the controls in any frame:
text, `pre`, svg, images, canvas, and any element's background image but the
window's own backdrop.

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

### The thinking control

The composer's thinking effort ([#287](https://github.com/nessalabs/nessa-agent/issues/287))
is the window's own control, `ui/thinking-control.tsx`, not nessa_ui's
`ModelThinkingControl`. That component's popover is fixed — a label with a
chevron over its own gradient slider and large knob — and exposes no place
for a level's description, swaps its label rather than cross-fading it, and
offers nothing to restyle its slider by; nessa_ui is out of scope here. What
it offered that still fits is kept: its `ModelFastMode` is the Fast toggle.

- **One owner of the levels.** The SDK catalogue: each model's effort levels
  in its provider's names and order, and whether it has Fast mode
  ([ADR 302](../todo/302-catalogue-reasoning-options.md), which replaced the
  Ultra and Fast tables this record first kept here). `thinkingLevelsFor` in
  `model/composer-options.ts` offers exactly the levels a model's entry lists,
  worded for the slider, and marks a level listed past `max` as Ultra,
  `utmost`. No catalogue model lists one today, so Ultra is offered nowhere yet.
  A choice carried to a model without it shows as the highest level it offers
  below it (`offeredLevelIndex`), and a level chosen there is recorded as
  chosen. Every rule
  of the slider — which level a key picks, where the pointer is and which level
  that snaps to — is `model/thinking-effort.ts`'s, by position, naming no
  level; where the popover sits is the tooltip's rule (`placeTooltip`, aligned
  to the chip's trailing edge), kept on the chip every frame while it is open,
  so a column folding or a pane opening beside carries it along.
- **Effort reads as rising.** A slider: a thin track whose fill is the theme's
  own light (`--desktop-light-*`, which a header picture tints), faint at its
  start and full at Max, and a small knob whose halo grows with it. Above it,
  the level's name and one line about it cross-fade, rising as the level rises
  and falling as it falls — only for a change made while it is open; it opens
  at rest. A model with Ultra ends its track with Ultra's own
  segment after a hairline gap, holding a trace of the theme's two lights;
  reaching it is the one moment — the segment glows, the lights bloom in the
  popover's corner, and a light runs once along the fill. A model without it
  ends at Max.
- **Fast is speed, not effort**: a pill of its own beside the heading, a fill
  while on, its bolt filled and leaning forward, two short streaks running
  back past it once as it turns on; never on the track.
- **Nothing moves the composer.** The chip is the same size whatever the level;
  wherever Fast is offered its bolt is laid out, on or off, so turning Fast on
  moves nothing either. The words hold one line in one cell and the slider its
  size, so the popover does not change size.
- **Keyboard, pointer and assistive technology.** The chip opens a non-modal
  dialog with the keyboard on the knob, a `slider` whose value is the level's
  name and whose description is its line; the arrows and Page Up and Down
  step, Home and End go to the ends. Pressed anywhere along the track, the
  knob comes to the pointer and follows it, the level following the nearest;
  let go, it settles on that level. Escape, Tab past either end, or a press
  elsewhere closes it, the first two onto its chip.
- **Motion** is transform and opacity on the window's tokens — the popover
  rises with `desktop-pop`; the fill is revealed by a clip sliding in while the
  light inside it slides back by as much, so the light stays fixed to the
  track; knob and fill glide together and settle — and while dragged the knob
  follows the pointer with no easing. With less motion nothing in it animates.
  The popover is drawn inside the surface its chip sits in, as the window's
  tooltip is, so it takes that surface's tokens, theme and reduced motion.

`responsive.mjs --only thinking-control` holds it in Chromium and WebKit: every
frame of every change and of a drag, the composer, its controls and the
popover in place; the held knob on the pointer and, let go, on its level; the
popover on its chip as ⌘B and ⌥⌘S move it; only Ultra marked apart; each
model it names offering exactly its catalogue levels and Fast, and a level
carried to a model without it shown as that model's nearest and chosen there; only
transform and opacity animated; and with the system's reduced motion, nothing
(`verification/desktop/CHECKLIST.md` › _Composer and approval card_).
`perf-budget.mjs` holds its walk from least to most and its drag to the frame budget
(`thinking-walk`, `thinking-drag`).

### Interaction and visual rules

The person's standing design choices for the window, held in review and
checked by hand (`verification/desktop/CHECKLIST.md` › _Menus and tooltips_
links here):

- **A selection is a fill.** No ring or border on a selected row or a
  highlighted menu item, in either theme.
- **Menus lead with icons only when every item has one.**
- **Shortcuts are right-aligned** in menus and tooltips, and named only where
  they work (`ui/layouts/shortcuts.ts` labels them).
- **A key is written as the platform writes it**: ⌃⌥⇧⌘ on a Mac, and
  Ctrl+Alt+Shift elsewhere — Option is ⌥ on a Mac and Alt everywhere else.
  One owner decides it for the whole window, the workspace, the classic shell
  and Settings alike: `src/desktop/model/keyboard.ts` matches and writes a
  chord, and `src/desktop/adapters/platform.ts` reads which platform it is,
  once. A surface states its chords as data and asks there, so a label
  cannot name a key its binding does not take (`model/keyboard.test.ts`,
  and each surface's own labels in `settings-view.test.tsx` and
  `adapters/dom/shortcuts.test.ts`).
- **A pane's actions are "…" then "×"**, shown on hover.
- **A submenu opens beside its item**, fully visible, never clipped by an
  ancestor, and takes the pointer.
- **One tooltip for the whole window**, in the window's own glass
  (`use-window-tooltips.ts`).
- **What moves is carried, not chased.** A dragged pane's copy is centred on
  the pointer, the one point that aims; nothing moves one way and then the
  other within a transition; only transform and opacity animate.

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
same command. The in-memory source records each the moment it is asked, then
the session as it found it when it carried the call out, and what became of
it — refused, or taken with the revision it produced (`WorkspaceSource.approve`,
`deny`, `setPinned`, `archive`). The gateway's source records nothing itself:
what it sends, the gateway records as this window's caller, and what it
refuses without sending is on no record (#248). A message carries who sent it too; the source records
it when, taken, it lets a waiting approval go — naming the message, the
approval and the sender. The in-memory source keeps that record (`audit()`),
and writes it before any subscriber hears of the change: one shown the
approval gone finds its `let-go` on record already.
Each of these commands, and sending a message, answers its caller with what
became of it — `sent`, `refused` when the source said no, `unknown` for
`unavailable` (which `model/failure.ts` defines), or `not-asked` when
there was nothing to ask, and for an answer `answering` when one is already on
its way. An answer is `not-asked` when neither a pane nor the open overview
shows the session (`onScreen`), or its conversation no longer asks that
approval: only an approval on screen is answered, so an agent answers what it
has opened. An answer on its way stays
while its pane shows another session. Each answer has its own token, so an
earlier answer's late refusal never sets aside the one on its way. Every call
to the source settles — an adapter rejects on a timeout of its own rather than
leave a read or a send hanging. The one mark the window clears itself is
"unread".

| Command | Shown at once | Source takes it | Source refuses (typed) | Source's update arrives |
| --- | --- | --- | --- | --- |
| `sendMessage` to a draft | listed at revision 0, titled by the message, running; the message in the outbox, "sending" | mark cleared | "Not sent. …", with Send Again and Discard; at rest once no message may begin it; Discard of the last takes a shown one back to a new session's home and lets an unshown one go | the conversation that includes the message retires it from the outbox |
| `sendMessage` to a session | the message in the outbox, "sending", with the model chosen for the next turn | mark cleared; an approval still waiting is let go, on the source's record with who sent the message | "Not sent. …", with Send Again (`resendMessage`) and Discard (`discardUnsent`); the session as the source last said | any conversation without it — a read of the history, a reply still streaming — leaves it where it is |
| `approve` / `deny` (with the initiator: `"person"` from the card, `"agent"` from an agent) | the approval's buttons at rest | the conversation that no longer asks lets the answer go — delivered before the call resolves where the source can say it by then (the in-memory source), after it otherwise (the gateway's, on its next read); the source records the decision, and who made it where it can tell (the gateway's records this window's caller, not person or agent) | asks again, saying why; answerable again | a conversation no longer asking lets the answer go |
| an answer to an approval the window does not hold — moved on, or its session in no pane | nothing; the command returns `not-asked` | — | — | — |
| `approve` / `deny` for a session the open overview shows (waiting, or chosen), in no pane | the row's and the peek's answers at rest; answered in the overview, the row settles in place (the view's own hold on the row) | as for a pane: the conversation that no longer asks, when it arrives, lets the answer go; answered in the overview, the row says what became of it, then leaves; answered from a pane or by an agent, the row leaves as the session moves on, with no settle | asks again, saying why, in the row and its peek; `unknown` (`unavailable`) the same, as a pane's card does | the session moves to Working; a row answered in the overview leaves once its settle has played |
| `approve` / `deny` for a session neither a pane nor the open overview shows (closed, or its filter leaves it out) | nothing; `not-asked` | — | — | — |
| the overview closes while an answer is on its way | nothing | the answer is kept, as for a pane showing another session | its reason is kept for when it is shown again | as above |
| `sendMessage` from the peek's reply pill while an approval waits | the message in the outbox, "sending" | the approval is let go, on the source's record with who sent it | "Not sent" under the pill; Send Again in the pane | as for a pane |
| a second answer while the first is on its way | nothing; the command returns `answering` | — | — | — |
| `pinSession` | nothing; a session at revision 0 is not pinned (Pin is disabled) | its update, delivered first, shows the pin | left as it was | a newer summary replaces it |
| `archiveSession` | nothing; a session at revision 0 is not archived (Archive is disabled) | its removal, delivered first, takes it out of the lists with everything the source said up to it; its pane closes, or the last starts over | left where it is | a summary the removal outranks stays out, whenever it arrives |
| a session is shown | marked read, and the source told | nothing more | the mark stays cleared | marked unread again while shown: read again |
| a shown session's transcript is read | the heading | the conversation, unless a newer one arrived meanwhile | the pane says why, and is not read again until "Try Again" (`retryTranscript`); a failure no pane shows any more is not kept, so the session is read afresh when shown again | the newer of it and the read's answer is kept, in either order |
| the index is read | nothing yet | the workspace opens on its first session; summaries the stream already brought, if newer, are kept | the workspace says why, with "Try Again" | a removal that arrived first keeps the session out |
| the index is read again (the resync) | nothing | a session it does not list, or lists in a channel it does not list, is taken out at the revision held — its pane closes, or the last starts over; every conversation on screen is read again, a read asked before that read of the index set aside and its answer let go — each answer paired with the read that asked it (`read`, on `indexRequested`, `indexLoaded`, `indexFailed`), in whatever order they arrive | an open workspace stays open, as it was | a summary the stream brought after that read was asked stays — noted per read, so a read asked after it is the newer word; the answer of a read outrun by a later-asked read already applied is let go unread |
| the source sends `resync` (it reconnected, or found a gap) | nothing | the index is read again, as above | as above | as above |
| a shown session's transcript read is answered with one the window cannot use (another session's, revision 0) | — | a fault, logged; the pane says why, with "Try Again" | — | — |
| an update at a revision the source could not have sent | — | let go, logged where received — by its kind, its session and the revision, never its content (a transcript holds the person's words and commands) | — | — |

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
  the pointer holds it and no agent asks for it; its phase is a pure value
  (`split-panes/model/drag.ts`) the drag adapter holds, asking the store only when the
  zone changes.
- **Keep patching the drag.** Three review rounds each found new cases in it
  (a blended aim point the eye did not follow, a zone that did not settle, a
  lost pointer that restarted the drag, previews re-read after the room
  changed); the redesign above removes behaviour instead — one aim point, no
  re-reading while carrying — and writes the machine down first.
- **The browser's own drag and drop.** Its drag image cannot be full size or
  move, so the carried pane could not become the window it will be; a pointer
  drag can.
- **Restyle nessa_ui's `ModelThinkingControl` from the stylesheet**, as the
  model picker and access mode are. Its slider could be quietened, but its
  popover has no room for a level's description and no way to cross-fade its
  label, so the control that reads as effort rising could not be reached
  without changing nessa_ui (_The thinking control_).
- **Separate stops rather than a slider.** Tried first (four bars filling one
  after another); a continuous track with a knob to drag reads as one scale of
  effort and was preferred, so the slider was kept and given the window's look.
- **Ultra in the SDK catalogue.** The right long-term home, but changing the
  catalogue was the SDK's decision, not this control's. It was made in
  [ADR 302](../todo/302-catalogue-reasoning-options.md), which records each
  provider's own levels and leaves the mapping onto the slider here.
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

Remaining — the one list of what this record leaves open; the
[index](../README.md) summarises it. Each is its own issue:

- **nessa_ui's icon contract, and an icon slot on its access mode**
  ([nessa_ui#101](https://github.com/nessalabs/nessa_ui/issues/101)). The icon
  provider in `src/desktop/ui/icons/` mirrors `NessaIconProvider` until
  nessa_ui ships it, and is then replaced by it, not kept beside it; the
  composer's access shield stays a masked outline in `styles.css` until
  nessa_ui's `ComposerAccessMode` has an icon slot.
- **The in-memory source records a same-tick message and answer out of
  order** ([#249](https://github.com/nessalabs/nessa-agent/issues/249)). A message that lets an approval go is recorded when the source
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
- **Watch: the drop's commit frame under load**
  ([#250](https://github.com/nessalabs/nessa-agent/issues/250)). A drop's `pointerup`
  commits the move, and `FlipScope` reads where the panes landed in that same
  task, so the new arrangement is laid out there (forced layout) rather than
  in the frame's own layout step; it is the same layout either way, which is
  why it is watched rather than moved. The frame after the press's `prepare`
  task is the other one near the line. On a quiet machine (load 3.0–3.7) at
  4× throttle, four runs each, the drop's longest frame was 33 ms headless
  and 35 ms headed (median 33–34), the cancel's 33–35 ms: inside the budget.
  Under heavy load (a fifth review's headed runs, other browsers running)
  the same frames reached 67–118 ms, the `pointerup` task 55–106 ms of it
  forced layout 41–84 ms. If a quiet run ever crosses 50 ms, the next step
  is reading the landed rects from what the drop already knows (the outcome's
  boxes) instead of from the page.
