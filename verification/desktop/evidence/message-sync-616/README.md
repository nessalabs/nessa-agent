# Transcript polling follow-up (#616)

Verified source head `5c5bb19753730ed58410ddc0b3229d3e25c5ad34`, clean tree, which merges main `5a045d9f2251e155e54949eafc7d9b241357d7fe`. Linux x86_64, Intel Xeon, 4 cores, 15 GiB RAM. Playwright 1.63.0 bundled Chromium and WebKit, headed.
Chromium calibrated 4× CPU, ratio 4.07 (plain 46 ms, throttled 186 ms). A known 120 ms frame measured 116.6 ms and was attributed. WebKit unthrottled.
The summary stays idle while the retained turn runs with text after send.

Production `message-sync --only delivery`, three fresh-page runs per engine and layout (`message-sync-delivery.json`):

| Engine / layout | Active median / max ms | Held median / max ms | Frame max / median run-max ms | Frames over 50 ms |
| --- | --- | --- | --- | --- |
| Chromium / columns | 388.4 / 491.4 | 346.7 / 451.2 | 66.7 / 50.1 | 5 |
| Chromium / sidebar | 349.5 / 508 | 272.9 / 472.9 | 83.4 / 83.3 | 17 |
| WebKit / columns | 990 / 1040 | 996 / 1094 | delivery returned first |  |
| WebKit / sidebar | 1018 / 1148 | 1034 / 1198 | delivery returned first |  |

Commands:

```sh
node verification/desktop/scripts/message-sync.mjs --mode prod --channel bundled --headed --runs 3 --only delivery --shots /tmp/616-shots-2 --out /tmp/616-delivery-2.json
node verification/desktop/scripts/message-sync.mjs --mode prod --channel bundled --engine chromium,webkit --layout columns --runs 1 --only delivery --headed --out /tmp/616-reverted-2.json
pnpm exec vitest run --config vitest.config.ts src/desktop/workspace/adapters/gateway/gateway-source.test.ts
pnpm test
pnpm test:e2e:scripted -- --channel bundled --evidence /tmp/616-scripted-head
```

- Delivery on the fixed source: 4/15 held. Every Chromium sample stayed within 600 ms. Columns run 2 also stayed within 50 ms. The other Chromium runs missed the frame bound by 50.1–83.4 ms, with empty LoAF scripts and at most 1 ms of style and layout. Every WebKit sample missed 600 ms. WebKit read cycles on this host were about one second, and the text still replaced in the DOM. WebKit idle shots are absent because those runs returned on the delivery bound.
- Source-only revert removes the retained running/queued check. Fixed sha256 `bc45bca08530fd6c420a0b428251e59f1b49d462fb37f27e4799bf93d2ec6700`. Reverted sha256 `9e994878d6e7fb7afe8d6831d2a84dbf9aa18e4e3a07e14c762a16bfeac53c1f`, matching current main. Both engines then fail: ten active and five held samples null per engine. Exact fixed bytes restored, sha256 matches, and the four F13 cases pass.
- Four F13 injected-clock cases fail on the reverted source and pass restored: running/queued, with an idle summary and with no summary. Reverted gateway-source file: 4 failed, 151 passed. Fixed file: 155/155. Full frontend vitest on this head: 285 files, 3,793/3,793.
- `production.json` is the earlier Mac run against base `86cbce188`. This head's delivery document is `message-sync-delivery.json`.
- Scripted gateway checks on this head, two runs. The first (`scripted/`) failed Chromium `gateway-window` console with `requestfailed: http://127.0.0.1:34877/mcp-resources net::ERR_ABORTED` (13/14); mcp-apps-gateway was 16/16 and scripted-scenarios 8/8. The repeat (`scripted-repeat/`) failed WebKit `mcp-apps-gateway` at `deny` with `frame.evaluate: Frame was detached`; gateway-window was 13/13 and scripted-scenarios 8/8. That detach is the #617 symptom. Cause remains unestablished.
- `scripted-initial/` is the earlier capture from source head `e10fe67e` against base `86cbce188`. It records the Chromium MCP denial `frame.evaluate: Frame was detached` tracked in #617. It is not a run of this head.

The measured boundary is controlled ready text to DOM replacement plus two animation-frame opportunities. Native compositor paint, provider startup, and production gateway payload/CPU cost are unmeasured. Historical #532 evidence and limitations remain in its original directory and report.


## Retained UI resources

At the user's request, the additional fixes remain on PR #618. The separate
#617/#620 reports were closed as consolidated, without claiming the original
intermittent symptoms' causes were established.

- Same-widget prefix changes and distinct repeated references retain their views.
  The real iframe check is functional verification in dev (the sandbox listener
  is absent from an ordinary production preview); production delivery remains
  separately calibrated. Four engine/layout combinations pass, plus console.
- A returning drag copy is removed in the resize event's turn in both engines
  and layouts: 5/5 held. The suspended flight is an actual Web Animation paused
  by the browser driver. Old source leaves one copy in both engines.
- Reverting only widget key retention replaces its proxy/app frame and loses
  document state on added/removed prefixes in Chromium and WebKit. Exact fixed
  source restoration is asserted in the probe driver.
- Source keeps its made resources through drop/cancellation, correlates terminal
  events, joins instant flights with deferred preview release, guards obsolete
  callbacks before effects, and releases retained resources on unmount. Regression
  tests cover stale settlement and both unmount phases.
- Full frontend vitest on this head, after the widget and drag commits: 285 files, 3,793 tests passed.
- The baseline production sweep was interrupted to investigate its failures,
  and is recorded as partial, not passing. Its missing-zone/chord symptoms and
  the one MCP detach remain causally unestablished. Eleven repeated pre-fix MCP
  core runs and isolated pre-fix drag checks passed; the reproducible ownership
  defects above have their own failing-before / passing-after guards.
