# App audit #914

The `6fd34dcc4/` packet records recovery/setup, signed-out gateway and native
clipboard verification before the later layout refinement. Its screenshots
show those controls, not a claim of final motion verification. Native clipboard
proof uses unchanged host/clipboard source from `5593efef3` in both local window
types; it does not test a signed public release or the user's installed app.

The `667a7a5e0/` packet records the layout trial committed in that change.
All runtime `src/` hashes match the measured dirty-tree source manifest. The
subsequent changes only remove a test non-null assertion and use the existing
shared body selector in the same browser check. Placement passes9/9 across
Chromium/WebKit, two layouts and sizes; restoring inheritance fails, restoring
the fix passes2/2. Scoped style/resize tests passed27/27.

The full calibrated4× production budget is **FAILED,32/35 rows held**.
Drag-drop now holds at max40.4/47ms (columns/sidebar). One cancellation frame
is53.7ms with no script/layout attribution; overview-answer is68.8/84.4ms,
with30–39ms forced layout. Clean main79c4 also reproduces the answer slowdown.
No experimental overview/focus modifications are included in this branch.
The earlier full functional run stopped at safe-area76/91 with220ms flights;
affected immediate-layout controls later held35/35. Full final functional
qualification and fresh full review remain pending. Do not relabel those failed
or incomplete runs green.

The current installed production app has not been replaced by these fixes.
A separate macOS test build from `667a7a5e0` is in progress.
