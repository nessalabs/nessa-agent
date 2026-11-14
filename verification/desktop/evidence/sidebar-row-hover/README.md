# Sidebar row hover (#1325)

Source fix: `ac0e8a86b140b1577552a4a5de596e158444c6b2`, based on
`edf3e028bf48748147ad2a0a35d028e2fc7cfca1`.

The kit places a row's actions beside its control. Pointing at that sibling
keeps the frame hovered, but stops hovering the control. The frame now supplies
the control's hover fill. Selected/pressed precedence and action targets stay
with their existing rules. The selector has the same specificity `(0,2,0)`.

Chromium before / after (1440 × 900, dev build, cropped to the same row):

![Before](before-chromium.png)
![After](after-chromium.png)

WebKit before / after:

![Before](before-webkit.png)
![After](after-webkit.png)

The existing measured regression was run without changing its assertions or
bounds, in Chromium and WebKit, columns and sidebar:

```sh
node verification/desktop/scripts/shared-controls.mjs --only rows \
  --engine chromium,webkit --layout columns,sidebar --channel bundled
```

Fixed: `shared-controls: 4/4 held`. Reverting only the selector: `2/4 held`;
both engines' columns fail with `kit row under its glyph: its fill is
rgba(0, 0, 0, 0)`. Restoring: `shared-controls: 4/4 held`. Both sidebar cases
also hold throughout, but their sample has no glyph row. The same pinned frame retains pointer membership and
its action hit target in both engine screenshots. The rows check also measures
selected/unread/name state, count/glyph hover, action visibility and hits.

Documentation review with the nessa-docs skill: no state page changes or
checkpoint advances. This selector changes a transient paint predicate; it
changes no stored state, command, admission, lifecycle or flow described by
`docs/state`. The existing shared-controls checklist already owns this exact
hover/action contract. No unrelated workspace page was certified.

Independent source review found no implementation findings. Browser execution
supports only the focused claim above. Full desktop qualification is not green:
smoke reports `28/32 held` because its existing Advanced settings explanation
assertion fails in all four profiles. This change must remain draft until
applicable browser and other repository gates pass. Native WKWebView/manual
interaction, the final full functional matrix and the scripted gateway flow
have not been rerun for this isolated repair.

Production performance was measured on this source in headless bundled Chromium,
three runs per interaction/layout, 4× CPU throttle. Calibration ratio 4.65 held;
the attributed 120 ms synthetic frame measured 211.5 ms. The interaction budget
failed all 33 rows (`perf-budget: 1/34 held`, including calibration). For the
sidebar toggle specifically:

| Layout | Maximum (ms) | Median longest frame (ms) | Frames over 50 ms |
| --- | ---: | ---: | ---: |
| columns | 139.6 | 121.7 | 13 |
| sidebar | 243.6 | 151.1 | 19 |

These are failed contracts, not a performance improvement claim. The retained
raw result includes each slow frame and available LoAF attribution; 455 of
1,369 recorded slow frames have no LoAF attribution. Observed scripts include
MessagePort callbacks, timers and keyboard/pointer handlers, alongside layout.
This does not establish that the hover selector caused the slow frames. A
suitable-host/control comparison remains needed before any performance verdict.
Pressed, coarse-pointer and overview-glass precedence were reviewed in source;
the focused browser receipts do not directly prove those states.

The full shared-controls suite subsequently reports `30/32 held`: both WebKit
segmented-control profiles time out at the unchanged 2000 ms bound. All four
rows profiles pass in that run. Identity, badges, rims, empty states, keys and
list measurements also pass. These adjacent failures remain qualification
blockers; their assertions and deadlines were preserved.


A subsequent headed run on the existing Linux VM used an Xorg dummy display
with no other tests or builds running. Chromium still uses ANGLE SwiftShader
(software rendering); this is not a hardware Mac or native WKWebView receipt.
All 33 interaction rows failed the unchanged 50 ms budget. Calibration held
at 4.02× and attributed the 120 ms synthetic frame at 122.3 ms.
[Host/GPU facts](gpu-capability.json) and [measurement summary](quiet-host-summary.json)
retain the workload and numbers.

Five alternating main-CSS/fixed-CSS pairs on nine affected interactions also
failed the absolute budget. Columns sidebar-toggle initially labelled fixed
minus main +61.7 ms (95% interval [3.9, 111.8]). Reversing source order labelled
main minus fixed +154.6 ms ([10.1, 333.4]), the opposite source-normalized sign.
Neither result is discarded. This does not establish a repeatable selector
regression or a safe performance verdict; runtime/source-order variation remains
unresolved. No paid host was provisioned.

The existing authentication workflow now has a manual, default-off
`desktop-performance` input. It measures production Chromium headed on the
existing standard `macos-latest` matrix leg, before Rust compilation. It retains
the original 4× throttle and 50 ms gate, five runs, both layouts, host/GPU facts
and full JSON in the job log. It adds no job, artifact or cache writer. Standard
GitHub-hosted runners for this public repository are free; larger runners are
not used. Actual Mac execution and quietness remain to be verified.

The focused CI contract tests pass 56/56 and actionlint 1.7.12 passes. Removing
the manual/Mac-only guard makes its regression fail; restoring it passes. An
independent review found no changes needed in the two CI files. State-model
checkpoints are unchanged: this opt-in measurement changes no application state
or lifecycle, and the earlier transient-hover review remains the same scope.
