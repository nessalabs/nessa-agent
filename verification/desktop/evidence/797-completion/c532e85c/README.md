# Final assembly verification at c532e85c

Checked source: `c532e85c7441e88e0560d8b9024f2ad865b467df`.
Includes main `7dbb400a`, #877 `841ec648`, #859 `90c99e64`, and
#828 `5edf42a4`. All remaining feature changes are assembled. This is a
verification branch, not a claim that those three PRs are already merged.

Its entire source differs from `19606433` only by the independently reviewed
one-line SDK test fixture repair. The unchanged frontend at `19606433` passed
4,583 tests / 346 files, types, lint, format, architecture, protocol,
documentation checks and production build. The repair itself passes the exact
SDK CI package selection, cancellation 1/1, deadline suite 20/20 and a fresh
one-file review; omission reproduces E0063. Supported-platform CI remains a
separate gate and was pending when these artifacts were saved.

## Current-source execution

- Fresh server build passed. Explicit private target binary was supplied through
  `MCP_LIVE_NESSA` to the normal signed-out real-gateway runner.
- Real-gateway MCP apps 16/16, gateway window 13/13, scenarios 8/8, both engines.
- Actual browser focus 84/84, clock snapshots 4/4, app-review 36/36.
- Actual-App attachments 40/40, zero page errors; 12 captures here are from this
  replay and were visually inspected. The script-overwritten canonical historical images were restored
  after these current copies were preserved.
- Isolated production delivery 14/14, three fresh pages per engine/layout,
  calibrated 4× Chromium and unthrottled WebKit. Frame max 39.2/50 ms;
  active max 44.8/600 ms, median 24.0 ms;
  held max 34.2/600 ms, median 23.0 ms.

Browser suites ran sequentially; no root-owned Cargo, build or other browser
suite overlapped the isolated delivery measurement. The unrounded calibration,
measurements, frame attribution and functional assertions are retained in JSON.
The earlier overlapping failed run remains in the source-attributed e27b1752
folder; it is not relabeled green or causally explained by this later pass.

These checks do not attribute the unidentified Fastify window or the intermittent
Windows endpoint replacement denial (#874). The latter has a passing unchanged
source rerun and an open investigation; actual blocking handle/subcall is not
identified. Final actual-main source equality is checked after merges land.
