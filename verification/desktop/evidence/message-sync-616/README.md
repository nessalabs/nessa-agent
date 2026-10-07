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

## Adverse-condition investigation — performance gate remains open

The synchronized source at `5c5bb197` was measured in a production build,
Chromium bundled, headed, 4× CDP CPU throttling, three fresh pages per
interaction and layout. CPU calibration held (ratio 3.1); the known 120 ms
frame was detected and attributed. `affected-performance-failed.json` and
its log retain all six failing rows. Streaming maxima were 117 ms in both
layouts; drag/drop maxima were 266/232 ms and cancellation 201/167 ms.
These are failures against the unchanged 50 ms budget, not passing evidence.

Five empty-page controls under 4× throttling calibrated successfully and
had no frame over 18 ms. This does not eliminate contention during application
work. Long Animation Frame attribution and `drag-diagnostic.cpuprofile`
identify expensive forced layout in the drop commit, specifically the first
final-geometry read in `FlipScope.play`. Source maps from an otherwise identical
production bundle mapped the callback to `drag.ts:1210` and the read to
`flip.tsx:124`. Profiling adds overhead; this profile is diagnostic only.

Temporary browser overrides tested removal of night-scene content, scene
layout containment, and non-inheriting pane geometry properties. Their JSON
files retain results; none established a complete fix, and no override was
applied to application source. Removal had one passing streaming sample but
still failed drag. Timing differences between separate runs do not prove
causation. Calibration sequence samples show timing variability without
establishing a JIT defect. The shared calibrator and acceptance threshold
remain unchanged. Production message delivery and the full performance gate
must be reverified after a supported fix.

Two supported scene improvements were then implemented: the ResizeObserver's
content height replaces synchronous computed-height reads, and a static
header keeps only its clipped rain window and one steam pattern. Animated
scenes retain their full tiling copies. The static reduced-motion rule keeps
that window at its resting offset. Both regression rules fail when reverted
individually and pass when restored. The same-page prototype comparison is
pixel-identical in Chromium/WebKit, both layouts and both motion modes, with
ambient drift held and the pointer outside hover controls.

The compiled headless run still failed all six frame rows; its complete raw
result is `bounded-scene-performance-failed.json`. It must not be replaced by
a claim based on the passing streaming diagnostic. Matched empty controls at
1600×1000, Retina scale 2 and 4× CPU all stayed below 18 ms in both modes.
Headed application idle controls also stayed below 18 ms. Extra compositor
promotion worsened headless idle behavior and was rejected.

`preview-reordering-rejected.json` alternates the compiled fixed scene with an
experimental preview-clock reordering, three rounds. The unchanged source
passed all three headed drop/cancel pairs (drop max 49.3 ms, cancel max 33.8 ms);
the experimental source failed drops at 99.5 and 50.3 ms. It was reverted, not
included in the change. Passing pairs do not erase the earlier failing runs or
establish behavior under arbitrary host contention.

## Controlled load and additional layout reads

`delivery-controlled-pressure.json` uses the development-mode parent runner
with two separately owned, continuously busy Node CPU workers. The message-sync
child reports its own **production** fixture. Chromium calibration held at a
4× requested rate (loop ratio 4.99) and attributed the known 120 ms frame.
All 15 delivery checks pass across both engines/layouts, three fresh-page runs.
Chromium active maxima are 487.4/485.9 ms and held maxima 529/527 ms;
frame maxima are 18.7 ms. WebKit is separately unthrottled: active maxima
276/506 ms, held maxima 274/239 ms and frame maxima 22/21 ms.
The worker ledger retains the declared workload and teardown outcome.

The full sample-renderer interaction sweep under the same additional worker
count holds **9/35**, with failures retained in
`full-interaction-pressure-failed.json`. This is distinct from gateway delivery
and the already-open #588 sample-renderer investigation. The budget remains
50 ms unrounded; this result is not passing aggregate evidence.

The invalidation trace identifies two eager layout reads on a new empty home:
Composer's draft-line measurement, then home-shape's initial computed-style
query. Removing only the empty-draft read shifts the large flush to home-shape
(103.5 ms), rather than establishing a frame-budget fix. Composer now supplies
the known empty-draft line count to the existing page-mode owner; nonempty and
whitespace drafts are still measured. Home-shape establishes its initial
baseline in the first resize observation, registered in the React layout commit
before paint; subsequent shape changes still settle. Regression tests fail on
each original source, including reverting the composition to a passive effect.
Both engines/layouts pass the real home-shape check after these changes.

The scene-omission build is diagnostic only. It preserves the scene's boxes,
omits decoration, and alternates with the unchanged source under two busy
workers. It is not shipped and does not establish the only source of layout
cost; all source was exactly restored. All comparisons and unresolved failures
remain part of the investigation.
