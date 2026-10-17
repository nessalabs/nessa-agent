# Workspace visibility — #914

Runtime-qualified source `8354279471796013198a28286f099b9b4bcc329b`, actual main `4cdbb0b87a0341253c6c577f8b9a2338e0fffce7`, private Nessa UI pin `0794a41bb1482aa7e677fd2b7012874bcc3e29b2`. These are fresh production Chromium and Playwright WebKit captures. Pane, full-chat widget and answered-peek captures follow two animation frames; screenshot transport can advance time. Separate browser samplers own first-mounted cumulative opacity, header/body paint and unit-transform assertions. Local-control shots show keyboard focus on one request while the pointer hovers another.

## Current qualification

- Full frontend: 364 Vitest files / 4883 tests plus all chained checks, exit 0.
- Own current-main gateway: all three canonical programs (`mcp-apps-gateway`, `gateway-window`, `scripted-scenarios`) passed Chromium and WebKit (16/14/9).
- Affected production programs: smoke 33/33; overview responsive 13/13; mounted visibility 13/13; widgets 13/13; focus 85/85; safe area 91/91; stationary drag 17/17. Total 265/265 held result rows.
- Incoming main-card coverage: responsive approval-card 5/5 (including Show All before measuring), command order 9/9, deadline 3/3 on an owned dev fixture. Total 17/17. No unrelated full matrix was repeated.
- Full production headed budget: `perf-budget: 35/35 held`, 33 interaction rows × five fresh-page runs, both layouts, unchanged 4× CPU setting, action windows and unrounded 50 ms decision. Calibration 22→64 ms (ratio 2.88); known 120 ms frame measured 120.400 ms and was LoAF-attributed. All rows have zero frames over 50 ms; maximum anywhere 44.800 ms. Send-home maxima/medians: columns 44.800/35.900 ms, sidebar 39.800/36.900 ms. Overview answer: columns 40.900/39.900 ms, sidebar 38.600/37.100 ms.
- All 4813 tracked-file hashes and 878 production artifact hashes remained unchanged through qualification. Sorted-map SHA256: source `880526841db83eabf4b0276dafcd1413b7f726dfb764d934f20c2bedcb577e3a`; artifact `b682f18ea60d7ffab88b901aca1a053c6bde39d7e289f9c07daf878e280e134e`. No other owned runtime during measurement; own preview/dev servers, runners and sleep-prevention processes retired and ports closed afterward. The result formatter warning did not affect exit 0.

## Regression and omission evidence

Before this first-send correction, the current-main matrix held 34/35: one native send-home sample reached 50.2 ms. Exact mapped diagnostic stacks identified native focus loss during React's removal of the focused home after preceding DOM mutations. The existing focus owner now retires only that home's focus before those mutations, synchronously restoring only an unchanged not-asked home when focus authority remains; the existing post-paint controller owns the next composer. No focus deferral, extra frame owner, timer, cache or state was added.

A fresh old-owner build rejected the new native ordering property in all eight engine/layout/motion cases. Fixed public tests held 44/44 and the full focus program 85/85. Omitting only `held.blur()` rejected all eight cases again; exact fresh restoration held 9/9 (including console), each native focusout preceding pane mutations while the original home/field remained connected. Restored 878 artifact files were byte-identical to the fixed full-focus artifact. The final budget records the retirement inside the measured native Enter action in all ten send-home samples; no work moved before the window.

Earlier local-row before/fixed checks and independent action/time guard omissions prove the deliberate row-local affordance rule. Whole-view fade/entry and overview-placement-owner before/fixed/omission records remain preserved at their original source-qualified checkpoints. Historical failed calibrations, 33/35 and 34/35 matrices, headless drag samples and rejected focus/scoping experiments are retained in the investigation packet; none is relabelled a current pass or treated as a waived gate. The original requested-80-ms observation was unmeasured. The replacement queued-input property explicitly uses synthetic DOM events; genuine native-repeat coverage remains.

No installed AppKit/native WKWebView, live provider or user-data operation is claimed. The subsequent evidence-only commit changes these eight pictures and this README; runtime/checker source and measured artifact bytes remain exact.

## Fresh screenshots

![agents answer chromium columns](agents-answer-chromium-columns.png)

![agents answer webkit sidebar](agents-answer-webkit-sidebar.png)

![new pane chromium columns](new-pane-chromium-columns.png)

![new pane webkit sidebar](new-pane-webkit-sidebar.png)

![chat widget chromium columns](chat-widget-chromium-columns.png)

![chat widget webkit sidebar](chat-widget-webkit-sidebar.png)

![local controls chromium columns](local-controls-chromium-columns.png)

![local controls webkit sidebar](local-controls-webkit-sidebar.png)

## Full headed budget

| Layout | Action | Maximum ms | Median ms | Frames >50 ms |
| --- | --- | ---: | ---: | ---: |
| columns | split-right | 28.300 | 26.500 | 0 |
| columns | split-down | 30.700 | 26.900 | 0 |
| columns | move | 25.800 | 21.200 | 0 |
| columns | close | 37.400 | 25.400 | 0 |
| columns | sidebar | 23.300 | 20.700 | 0 |
| columns | session-list | 22.600 | 19.100 | 0 |
| columns | send-home | 44.800 | 35.900 | 0 |
| columns | typing | 22.500 | 20.700 | 0 |
| columns | stream-1-of-4 | 26.200 | 24.400 | 0 |
| columns | drag-drop | 39.200 | 27.000 | 0 |
| columns | drag-cancel | 26.700 | 25.400 | 0 |
| columns | thinking-walk | 22.600 | 21.800 | 0 |
| columns | thinking-drag | 20.500 | 19.400 | 0 |
| columns | overview-open | 32.600 | 25.600 | 0 |
| columns | overview-select | 29.600 | 25.800 | 0 |
| columns | overview-answer | 40.900 | 39.900 | 0 |
| columns | overview-leave | 24.300 | 20.700 | 0 |
| sidebar | split-right | 27.500 | 27.400 | 0 |
| sidebar | split-down | 30.100 | 29.200 | 0 |
| sidebar | move | 22.700 | 21.800 | 0 |
| sidebar | close | 36.900 | 28.000 | 0 |
| sidebar | sidebar | 29.400 | 27.600 | 0 |
| sidebar | send-home | 39.800 | 36.900 | 0 |
| sidebar | typing | 22.700 | 21.400 | 0 |
| sidebar | stream-1-of-4 | 28.900 | 26.800 | 0 |
| sidebar | drag-drop | 27.800 | 26.600 | 0 |
| sidebar | drag-cancel | 27.400 | 26.900 | 0 |
| sidebar | thinking-walk | 21.900 | 20.300 | 0 |
| sidebar | thinking-drag | 18.700 | 18.100 | 0 |
| sidebar | overview-open | 30.900 | 24.900 | 0 |
| sidebar | overview-select | 29.400 | 24.800 | 0 |
| sidebar | overview-answer | 38.600 | 37.100 | 0 |
| sidebar | overview-leave | 20.200 | 19.200 | 0 |
