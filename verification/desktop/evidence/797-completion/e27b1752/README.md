# Completion verification at e27b1752

Checked runtime source: `e27b175228572df206ee418070f9dbaf71286b6c`, including
main `79c2eca773e79e5f41f4a0f0fcc2e860f5191b97` (#846) and the published
#823 `22fa4831`, #828 `e980b792`, #834 `0daa8ad1`, #838 `d320d5ec`,
#859 `fef42a36`, #871 `5f5f21e3`.
Runtime source matches `0c08be9f`; the final parent merge adds only a Rust test
fixture correction. #871 had a subsequently reported test scheduling review finding; the test-only
follow-up is recorded below. This evidence does not establish green CI or
review closure for any PR.

## Executed checks

- Full frontend at `0c08be9f`: 4,577 tests / 346 files; types, lint, format,
  architecture, protocol, documentation checks and production build passed.
- Fresh gateway build at `0c08be9f`; normal signed-out real-gateway runner:
  MCP apps 16/16, gateway window 13/13, scripted scenarios 8/8 in both engines.
  Private rebuilt binary supplied through `MCP_LIVE_NESSA`.
- Focus 84/84 and clock snapshots 4/4 at `e27b1752`, Chromium/WebKit.
- Actual-App attachments 40/40, zero page errors, at `e27b1752`.
  Four current captures were visually inspected; canonical historical images
  overwritten by the script were restored after these copies were preserved.
- Isolated production delivery 14/14, three fresh pages per engine/layout,
  calibrated 4× Chromium and unthrottled WebKit. Maximum frame 39.9/50 ms;
  active delivery max 43.6/600 ms, median 25.6 ms;
  held delivery max 34.0/600 ms, median 23.2 ms.

## Retained failed run

The preceding delivery run overlapped the root-owned attachment suite and
failed two Chromium columns frame assertions: 60.2 and 53.6 ms against 50 ms.
Its JSON is retained separately. Both LoAF entries have empty script attribution,
zero blocking time, and style/layout duration 0 and 1 ms respectively. The
isolated replay passed on unchanged source and budgets. This supports treating
the overlapping run as unsuitable for isolated measurement; it does not prove
an exact OS scheduling cause or a product fix. The older 154 ms sample and the
unidentified Fastify error window also remain without causal attribution.

These are local results. Supported-platform CI and review closure are separate
merge gates, tracked in the PR completion document. Earlier directories retain
historical source attribution rather than being relabeled as current.

## Test-only follow-up

Published #871 `6608f4b16c23b177407d50c4ca41532fcc8ce1af` resolves the
pre-read scheduling finding by pausing Tokio time and advancing it only after
read entry. Author targeted test, 250 ms wall-delay adversarial probe and
timeout/cancellation negative controls pass; fresh full source review is clean.
The thread is resolved; current-head CI remains a separate pending gate.

Combined merge `33651c10084f7eabe3c494a76721020e627c58fd` incorporates it.
Against the checked runtime `e27b1752`, only that five-line test correction and
this evidence directory differ. Each feature head's three-way merge preview
inherits the correction cleanly; this does not substitute for checking the actual
main tree once the merges land.
