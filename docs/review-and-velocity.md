# Review, change size, and velocity

The structural rules only survive if the review loop enforces them. This is that
loop.

## The standard

Approve when the change **definitely improves the overall health of the system**,
even if it is not perfect. Perfect is not the bar; withheld approval in pursuit
of it costs more than the imperfection.

Order of attention, highest first:

1. Does it solve the right problem?
2. Is the design right — do the dependencies point inward, does the logic sit in
   the layer that owns it, is a boundary being crossed?
3. Is it correct?
4. Is it tested, and would the test have failed before?
5. Is it readable by someone who was not in the conversation?
6. Are the names the product's names?
7. Is it tidy?

Do not lead with 7. Leading with formatting on a change with a layering problem
wastes the author's revision and yours.

## Conventions

**`Nit:`** prefixes anything the author may ignore. Everything without the
prefix is expected to be addressed or argued with. This one convention removes
most of the friction from review, because it removes the guessing.

**Facts, then principles, then the author's preference.** Technical facts win.
Where a written principle applies, cite it. Where neither applies, it is the
author's call — "I'd have done it differently" is not a review comment.

**Explain the why.** A correction teaches nothing; a correction with its
principle means the comment is not needed next time.

**Disagreement resolves in conversation, not in comment threads.** Two rounds
without convergence means talk, not type.

## Change size

- Aim under ~200 changed lines. Review effectiveness falls off a cliff past
  that, and the reviewer starts skimming without admitting it.
- One reason per change. Refactor and behaviour change do not travel together —
  split them so each is reviewable, revertable, and bisectable on its own.
- Three small pull requests beat one large one even at identical total work,
  because the feedback arrives while acting on it is still cheap.
- Mechanical changes (a rename, a move) go in their own commit, clearly labelled,
  so the reviewer can skip the body and check the edges.

## Commits

- First line: imperative, lowercase, prefixed with the module, under ~50
  characters, no trailing period — `tray: reapply frame on every show`.
- Blank second line.
- Body wrapped, explaining *why*, and referencing the issue it closes.

The message answers the question a future reader has, which is never "what
changed" — the diff says that — but "why was this acceptable".

## Before large work

Anything expensive to reverse gets a page first: the problem, the proposed
change, the alternatives, the consequences accepted. Circulated before
implementation. This is the cheapest place to find out the design is wrong; the
next cheapest is review, at roughly ten times the price.

Once decided, it becomes an [ADR](adr/README.md).

## Velocity diagnostics

Run these periodically. They measure whether the structure is doing its job.

- **Files per routine change.** Rising means boundaries are drifting. Look at
  the last ten changes and ask, for each, how many files it *should* have
  touched.
- **Modules per feature.** A normal feature touching three or more contexts is a
  boundary in the wrong place, not a big feature.
- **Time from open to merge.** If it is growing, changes are too large or
  ownership is unclear.
- **The file everyone edits.** Whichever file appears in the most diffs
  regardless of subject is either a composition root (fine) or a god object
  (not).
- **Setup lines per test.** Growing setup is the earliest warning of coupling,
  and it shows up long before anything else does.

## Keeping the map current

`ARCHITECTURE.md` is updated in the same change that invalidates it, or it is
not updated at all. A stale map is worse than no map, because people trust it.
When a change adds a module, moves a boundary, or breaks an absence, the map
moves with it or the change is not done.
