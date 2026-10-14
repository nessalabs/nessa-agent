# Published clock snapshot browser verification

The public `ClockProvider`/`useNow` fixture drives an injected clock from 999 to
1000 between React's reads, waits for one interval publication, then unmounts
and verifies that each timer created by StrictMode setup is cleared.

Fixed source: `f6a697af9e94db9e2c753dde3046f56a3f4e8d5a`.

- Command: `node verification/desktop/scripts/clock-snapshots.mjs --out <result.json>`
- Chromium and WebKit held `published-tick-and-cleanup`; each rendered 1000,
  recorded one timer tick, and ended with 2 timer starts / 2 stops and no page
  or console errors. The normal dev-server console row only notes the known
  favicon 404.
- Result JSON: `clock-snapshots.json`.

This is the historical fixed run for the source-revert witness below. It remains
at the source revision that produced it; the later two-case verification is in
`clock-snapshots-current.json` and is attributed to its own source commit there.

## Source-revert witness

A detached worktree at the fixed source commit used a separately cloned
`@nessa-ui/react` checkout at `30900b8644ce9b1bb49681ed28067dad4537a70d` and
its own offline-installed dependencies. An asserted one-occurrence mutation
replaced `useSyncExternalStore(subscribe, store.getSnapshot)` with
`useSyncExternalStore(subscribe, () => Math.floor(clock.now() / interval))` in
`src/clock/ui/provider.tsx`. The fixed and mutated module SHA-256 values were
`2daedfda65ca4c2cfd9a2dad93953c7f00e3437df0d1b7426e68ece27709488e` and
`cdaf7ff5c7c939cbc160dcc5c2b3b5b341a9896e5400ff7ce082ef78d06be8da`.

The same browser script then exited 1 in both engines. Each console row
captured React's `The result of getSnapshot should be cached to avoid an
infinite loop` warning; each hook row also timed out waiting for the published
render. Result JSON and the short raw log are `source-revert.json` and
`source-revert.log`. The probe worktree was removed after capture.

## Current independent cases

Source: `f1c30633d0f3d29f7cae1d68c423277fd7ab13da`.

The current fixture keeps the two production failure modes distinct. CL1 injects
999 then 1000 on consecutive reads; replacing the cached revision with the old
time-derived snapshot fails in both engines with React's uncached-snapshot
warning. CL2 records a committed 999 baseline, advances time, and invokes the
active 1000 ms interval callback; removing listener notification fails before a
new rendered 1000 can commit. The same CL2 case unmounts the last reader and
checks StrictMode timer starts and stops match.

- Command: `node verification/desktop/scripts/clock-snapshots.mjs --mode dev --engine chromium,webkit --channel bundled --out clock-snapshots-current.json`.
- Current result: `clock-snapshots-current.json`, **4/4 held**. Both engines show CL2 changing from 999 / two renders to 1000 / four renders, one published tick, and complete cleanup.
- CL1 time-derived-snapshot mutation: `source-revert-time-derived.json` and `source-revert-time-derived.log`; **0/4 held**, with the React warning in Chromium and WebKit.
- CL2 missing-listener-notification mutation: `source-revert-no-notify.json` and `source-revert-no-notify.log`; **0/2 held**, both timing out before a post-tick render.

Both source-revert runs asserted one exact mutation site and restored the
provider file byte-for-byte before the positive current-head run.
