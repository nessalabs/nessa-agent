# Read the live owner (#672, #868, #963)

Three checks were reading a copy taken too early. The mount test treated one
`act` as the whole frame queue. The lifetime verifier read follow counts off
the last render. Share returned focus to the menu item that had opened it,
which is gone once the menu unmounts. Each fix reads the thing that still
owns the fact, at the moment the check runs.

No stored data changes. Harness scope stays Claude and Codex.

## Mount frames (#672)

The Agents entry takes the current page on the second frame after it is
opened (`OverviewRow`). Mount may also have queued frames, including the
pane fill that schedules its next frame from the update the first frame
commits. Those updates land when `act` ends, so a loop inside one `act`
never sees the frame it just caused.

| Row | What is queued | What runs | Entry |
| --- | --- | --- | --- |
| M1 | Mount frames only | One frame per `act`, then the frame that update queued | Not the current page |
| M2 | Open has just been asked | No frame yet | Not marked |
| M3 | The open's first frame | That frame, and any mount frame still beside it | Still not marked |
| M4 | The open's second frame | That frame | Current page (`data-active="true"`, `aria-current="page"`) |

Invariants:

- The mark is the entry's own attributes after two frames of the open.
  `frames.pending() === 0` is not that contract.
- Each flushed frame gets its own `act`, which is `flushAnimationFrames`.
- The root is unmounted before the test scheduler is restored, including
  when an assertion fails.
- A frame still queued by something else is named by its stack on a failed
  mark. It does not fail the test by itself.

Alternatives that lose:

- A higher cap inside the same `act`. The frame caused by the last callback
  is queued when that `act` ends, after the loop has stopped looking.
- Asserting the global queue is empty. That fails the build on a pane-fill
  callback and says nothing about the entry.
- Waiting on the wall clock. The miss is the `act` boundary, not time.

## Lifetime counters (#868)

`follows`, `stops` and `closed` live in the fixture. A render copies them
onto attributes. A cleanup that runs after that render leaves the attributes
behind.

| Row | Cleanup | Rendered attributes | What the verifier reads |
| --- | --- | --- | --- |
| C1 | Follow started | May still show the previous copy | Live `follows` |
| C2 | Stop runs after the finish render | Still the old `stops` | Live `stops` |
| C3 | Original close records its target | May lag | Live `closed` |
| C4 | Finish render | Identity, phase, notices, overflow | Those, from the document |

Invariants:

- One owner, `createLifetimeCounters`, published as
  `window.__localTabLifetimes.snapshot`. The verifier calls that through
  `readLifetimeCounters` (`verification/desktop/scripts/lib/lifetime-snapshot.mjs`).
  The controlled test imports the same function. Its types are the sibling
  `lifetime-snapshot.d.mts`, the split `scripts/nessa-ui-paths.mjs` already uses.
- The snapshot is a copy. Reading it does not require another render.
- Identity, phase, notices and overflow stay on the document. The diagnostic
  counts do not.
- A missing or malformed owner fails the check.

Alternatives that lose:

- React state so the attributes update. The next cleanup after that render
  is stale again, and the counts are not product state.
- Re-reading the attributes after another paint. The verifier then depends
  on a render it does not own.
- Reaching into the module from the script with no published accessor. That
  is a second way to read the same fact.

## Share focus (#963)

Share is opened from a session's menu. The menu item is focused, then the
menu unmounts and the dialog opens. Radix recorded that menu item. On close
the item is gone, so focus falls through to `body`.

| Row | Close | Focus when the dialog is gone | Where focus ends |
| --- | --- | --- | --- |
| S1 | Escape, close button, or a press outside | `body`, and the session's row is still drawn | That row |
| S2 | The same | `body`, and the row is gone | `body` |
| S3 | The same | Already inside that session's row | Left where it is |
| S4 | Link a Device, which opens Settings | Settings, or whatever took focus | Left where it is |
| S5 | Dialog still open | Inside the dialog | Tab does not leave it |
| S6 | Escape or close | `body`, and the session is drawn twice, one copy inside an inert ancestor | The copy that can take focus |

Invariants:

- The return target is the session row named by the dialog, looked up when
  the dialog closes, not the menu item captured when it opened.
- A copy inside an inert ancestor is not that target. The folded sidebar
  keeps a pinned session's row, and that row is first in the document.
- A close that already moved focus somewhere else does not pull it back.
- Tab containment is the dialog's existing trap.

Alternatives that lose:

- The kit's opener restore. A context-menu item has no trigger the lookup
  can keep, and the item itself has unmounted.
- The menu's own hand-back. It runs when the menu closes, and the dialog
  takes focus after that. It does not run again when the dialog closes.
- Remembering the menu's element. The row may have been drawn again. The
  session id still names it.
- The first element the session id matches. In columns that is the sidebar
  copy, and a folded sidebar is inert, so focus stays on the page.

## Review brief

Reviewed tree: branch `cursor/live-owner-reads-263c`, base `main`
(`7b429791ec294ead4952ec86dde67d284d5aa9e9`). Head is this branch after the
implementation commit; review the committed tree, not a dirty worktree.

Intended behavior is the three tables above. Owned scope is the mount-frame
test, the lifetime fixture and its verifier, and Share's focus return.
Out of scope: product follow and close behavior, share grants, and issues
#965, #981, #1038, #1039, #1030, #1034, #345, #337, #746.

Standards: [CODING_STANDARDS.md](../../CODING_STANDARDS.md), gates 4, 5, 6,
13, 15, 16 and 17. The adversarial pass should try to falsify each
invariant: a frame queued by an effect after `act`, a counter changed with
no render, and Share closed after its menu item has been removed while
another surface already holds focus.
