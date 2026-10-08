# PR #671 frontend CI failure diagnosis

Frozen checkout: `/workspace/nessa-agent-peer-reply-capacity`, HEAD `8f083e81e2c323c818cb51612c45edcef42c61b6`. No repository source edits, Cargo commands, browser verification, or GitHub mutations were performed for this diagnosis.

## Conclusion

The observed CI failure is a pre-existing frontend mount/animation-queue timing failure, unrelated to issue #665's Rust MCP admission change. The failure is at `layouts.test.tsx:289`, **before the Agents entry is clicked**, on the assumption that mounting has no remaining animation-frame callbacks. It does not show a failure of the Agents entry's two-frame publication behavior. The exact owner of the single callback left queued in CI cannot be determined from CI's assertion output and was not reproduced locally; do not claim that owner has been proven.

`git diff --stat d59d9272ed7afdb512c13263c8b9620ebd591db0 -- src package.json pnpm-lock.yaml vitest.config.ts .vendor` is empty. Therefore the affected frontend, test harness, dependency declarations, lockfile, Vitest configuration, and vendored dependency revision are unchanged from the intended base. Previous passing CI runs of these same frontend inputs were supplied in the root task. Local repro runs also passed.

## Source evidence

- `src/desktop/workspace/ui/layouts/layouts.test.tsx:281-289`: installs controlled frames, mounts `SessionsInSidebar`, then runs at most 12 synchronous `frames.runFrame()` calls **inside one async `act`**, checking pending count inside that act. Only after the act completes does it assert zero pending callbacks.
- `src/desktop/workspace/testing.ts:279-314`: `controlledAnimationFrames` queues callbacks globally on `window.requestAnimationFrame`. `runFrame` executes only the callbacks queued when that invocation begins. `pending` counts every owner, not only Agents callbacks.
- `src/desktop/workspace/ui/panes/pane.tsx:65-70`: mounting a pane queues a frame that calls `startTransition(() => setFilled(true))`. The resulting conversation/home descendants and their effects arrive after React commits that transition. An empty queue inside the outer act does not establish that mount work cannot subsequently schedule callbacks when act flushes commits/effects.
- `src/desktop/workspace/testing.ts:318-330`: the existing `flushAnimationFrames` helper already runs **each frame inside its own awaited act**, precisely the boundary missing from this mount loop.
- `src/desktop/workspace/ui/source-list/overview-row.tsx:14-35`: the actual entry sets `marked` on its second post-open callback and cancels callbacks on teardown. Both unchanged local runs verified the existing test's post-click assertions.
- `src/desktop/workspace/adapters/dom/focus.ts:80-110` and vendored `packages/react/src/lib/size-observer.ts:194-211` show other legitimate frame owners in a mounted workspace. Neither has been identified as the CI survivor.

The source supports a mount-flush/act-boundary race as a plausible mechanism. It does **not** prove whether the CI callback came from React's mount effects, a descendant measurement observer, a focus retry, or a preceding test's residual work.

## Targeted verification

Environment: Node `v24.19.0`, pnpm `11.19.0`, Vitest `3.2.7`; existing node_modules. No broad frontend suite rerun.

1. `pnpm exec vitest run --config vitest.config.ts src/desktop/workspace/ui/layouts/layouts.test.tsx -t 'waits two frames, then takes the current page'`: **1 passed, 31 skipped** (171 ms tests, 4.61 s total).
2. `pnpm exec vitest run --config vitest.config.ts src/desktop/workspace/ui/layouts/layouts.test.tsx`: **32 passed** (4.225 s tests, 8.39 s total).
3. Same exact test using an external in-memory transform that records callback allocation stacks: **1 passed, 31 skipped** (170 ms tests). Initial queue: two callbacks, both from `pane.tsx:68`; then queue zero inside and after the act. Trace: `frontend-ci-frame-trace.log`.
4. Same file using that instrumentation: **32 passed** (4.086 s tests). Same initial owners and queue behavior. Trace: `frontend-ci-file-frame-trace.log`.

Instrumentation lives outside the checkout in `frontend-ci-instrument.config.mjs`. It transforms only the test helper in memory to retain allocation stacks and log pending callbacks; it does not modify source files or product behavior. Instrumentation itself can affect timing, so its passing outcome is not proof of absence under CI load.

## Narrow remedy, separate from issue #665

Change the mount-drain loop to await one `act` per frame (or use the existing `flushAnimationFrames` helper with a sufficient explicit bound), checking queue state only after each act has flushed React commits and effects. Keep the zero-queue assertion if full mount quiescence is the test's intended setup requirement. Keep the exact first/second post-click assertions. This is preferable to merely increasing the current 12 iterations inside one act, which leaves the commit boundary unchanged.

Also put root unmount in a `finally` preceding `frames.restore()` so a setup/assertion failure cannot leave a mounted root after restoring global scheduling. This is a cleanup defect in the existing test regardless of the original callback owner.

A focused rerun of the failed CI job is justified on unchanged frozen source. If it fails again, retain callback-owner tracing in a separate frontend investigation and verify the proposed per-frame-act remedy under the failing load; do not fold an unproven frontend change into #665.
