# Tool disclosure scroll

A tool run in a conversation opens downward from the header that was clicked.
The reader stays on that header. A reply that is still growing keeps following
the end when the reader is already there.

Measured on the sample conversation "Split panes for the chat workspace"
(`split-panes`), Chromium, the reader at the end (`distance` 0). Opening
"Ran 3 tools" grew the transcript 86px. `scrollTop` grew 86px and the header's
viewport top moved from 235 to 149. The same growth with `overflow-anchor:
none` and the resize follow blocked left `scrollTop` unchanged and the header
where it was. Blocking only the resize follow, or only anchoring, each still
moved the header by the full growth. Scrolled away from the end, neither
moved it. WebKit is checked by `verification/desktop/scripts/tool-disclosure.mjs`.

The waterfall and the untimed list are the same `ToolTrace`. A call with no
`startedAtMs` has no bar, and a run with none drops the time column
(`spanOf` in the kit's `tool-trace.tsx`). The sample run used for the
evidence omitted those times, so it drew the untimed list. The hold listens
for that trace's header, call row, and folded-earlier row. The sample run
now has times, and the check requires the axis and the bars.

The resize follow is the `ResizeObserver` in
`src/desktop/workspace/ui/transcript/transcript.tsx`: while the reader is
within 24px of the end it sets `scrollTop` to `scrollHeight`. The browser's
scroll anchoring does the same when the scroller is already at its end.
A subagent transcript follows on a new line (`use-stick-to-bottom.ts`) and
has the same anchoring, so it uses the same hold.

## States

`following` means the reader is at the end (within the scroller's threshold).
`reading` means they are not. `free` means no tool disclosure is in motion.
`held` means one is: a tool line, a call row, or the folded-earlier row.

| Follow | Hold | Content grows or shrinks | What scroll does |
| --- | --- | --- | --- |
| following | free | resize | Move to the end |
| reading | free | resize | Leave `scrollTop` as it is. Anchoring may keep a paragraph still when something above it grows |
| following | held | resize | Leave `scrollTop`. Anchoring is off |
| reading | held | resize | Leave `scrollTop`. Anchoring is off |
| either | held | the disclosure's motion ends | Recompute follow from the distance. Do not move |
| either | free | the reader scrolls | Recompute follow from the distance |

A disclosure press enters `held` before that control's own handler. The hold
lasts until the trace's animations have finished and at least
`--nessa-motion-duration-normal` has passed, then one frame, and never longer
than a second. Another press restarts that wait. The hold ends without a move,
so an open that pushed the end away leaves the reader `reading`. Closing at
the end can move the header down only by the browser's clamp.

## Invariants

1. A resize while `held` does not change `scrollTop`. `followTranscriptEnd`
   in `tool-disclosure.ts`, `tool-disclosure.test.tsx`, `transcript.test.tsx`.
2. While `held`, the scroller's `overflow-anchor` is `none`.
   `transcript.css`, `tool-disclosure.mjs`.
3. The pressed control is a tool line, a call row, or the folded-earlier row,
   and the press is seen on the way down, before the control opens.
   `tool-disclosure.test.tsx`.
4. When the hold ends, follow is the distance then, and `scrollTop` is not
   written. `transcript.test.tsx`.
5. A later resize while `following` and `free` moves to the end.
   `transcript.test.tsx`, `use-stick-to-bottom.test.tsx`, `tool-disclosure.mjs`.
6. The clicked header's viewport top is unchanged after it opens and after it
   closes, at the end and away from it. `tool-disclosure.mjs`. Closing while
   already at the end can move the header only by the browser's clamp, when
   the new end is above the current `scrollTop`. The check uses a transcript
   with room, so the header stays.

## Alternatives

Leaving anchoring on and only skipping the resize follow still moves the
header at the end. Turning anchoring off for good would let a growth above
the paragraph being read (an image, a trace still running) shift that
paragraph. Scrolling the header into view after the open puts it at an edge
instead of where it was clicked. Correcting `scrollTop` from the header's
rectangle each frame forces layout on the animation; the hold means there is
nothing to correct.

## What this does not change

Opening a running trace, and folding it when the turn ends, are not presses.
They stay on the resize follow. The conversation stress check already opens
a settled run and one call; it asserts the header stays.
