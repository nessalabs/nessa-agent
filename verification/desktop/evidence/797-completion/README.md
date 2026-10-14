# #797 assembled completion evidence

Tested assembly: `4fb33cd5e114a707c563edf0ab91d5c2147b32cd`, including main
`e40733a2a6b74c2e102c1092a1408b8f059ff99b` and all thirteen tracked slices.
This is combined-source evidence, not a claim that every leaf was independently
rerun at this assembly SHA. Leaf review and regression evidence remains on each PR.
The runtime build and browser assets belong to this checkout. Private UI revision:
`30900b8644ce9b1bb49681ed28067dad4537a70d`; fonts resolve beneath this checkout.

- Frontend: 4,502 tests in 340 files; TypeScript, ESLint, formatting and architecture guard passed.
- Architecture and verification helper tests: 289/289 at preceding assembly `482b7432d`.
- Focus: 84/84 in Chromium/WebKit, both layouts, at `482b7432d`; source unchanged by the later main merge for focus/clock/keyboard owners. An initial attempt was stopped because font links resolved into another checkout and Vite refused them. Only the replay with owned fonts counts.
- Actual panel attachment scenarios: 36/36 in Chromium/WebKit at `4fb33cd5e`, including same-local-ID provider swaps, folder drop ownership, pathless drops and picker fallback.
- Signed-out gateway/MCP: verdict **pass**, MCP apps 16/16, gateway window 13/13, scripted scenarios 8/8, both engines. The exact emitted summary is [pr-summary.md](pr-summary.md).
- Production message delivery: 14/14 (calibration, twelve fresh-page runs and aggregate); three runs per engine/layout. Chromium uses 4x CPU throttling, WebKit is unthrottled. Max frame **48.5/50 ms**, active delivery **59.1/600 ms**, held delivery **34.7/600 ms**, zero over-budget interaction frames. Per-run medians/maxima and aggregates are in [delivery-statistics.json](delivery-statistics.json). No browser or build workload owned by this task ran concurrently with this measurement.

The three integration fixture migrations switch obsolete `send` observers to the
prepared `submit` API and move the restored-lifetime clock import to `src/clock`.
They belong in #823 and #859 after their upstream callers land. They add no product
state or compatibility aliases. Fresh assembled review checked those changes and
the merge with current main. One minor explanation defect introduced by main #843
is separately recorded in #860; it does not alter identity/authority behavior.

Commands used direct Node binaries rather than package-manager exec:

```text
node node_modules/vitest/vitest.mjs run --config vitest.config.ts
node node_modules/typescript/bin/tsc --noEmit
node verification/desktop/scripts/attachments-races.mjs
CARGO_TARGET_DIR=/tmp/nessa-797-combined-cargo-target MCP_LIVE_NESSA=/tmp/nessa-797-combined-cargo-target/debug/nessa node verification/desktop/scripts/scripted-e2e.mjs --channel bundled --evidence /tmp/nessa-final-4fb-scripted
NESSA_STAGE=ci VITE_NESSA_STAGE=ci node node_modules/vite/bin/vite.js build --config verification/desktop/fixtures/message-sync/vite.config.ts --outDir /tmp/nessa-final-4fb-message-dist
node verification/desktop/scripts/message-sync.mjs --mode prod --only delivery --engine chromium,webkit --channel bundled --url http://127.0.0.1:1506/desktop.html --out /tmp/nessa-final-4fb-message-delivery.json
```

The screenshot's Fastify 500 remains unattributed. Passing these checks does not
identify its server or claim to fix that separate symptom. The original 154 ms
frame's precise cause is also unmeasured; these results establish the current
production budgets and the separately reproduced clock snapshot fix.
