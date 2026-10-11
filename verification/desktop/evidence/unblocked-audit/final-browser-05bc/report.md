# Final browser verification

Exact clean source: `05bc3c6c3b1ab1a9148bfc45f7cb133cc6f855fe`. Private checkout: `/Users/nessa/.codex/worktrees/stability-audit/nessa-agent`.

Command: `node verification/desktop/scripts/run-all.mjs --skip-perf --engine chromium,webkit --channel bundled --shots <owned directory> --out <owned result.json>`; default dev functional mode, all 23 default checks, layouts and sizes; message-sync builds its own production fixture. No quick tier. Correct private gateway rebuilt first (0.39 s).

**PASS: 23/23 groups, 990/990 held, exit 0; 3112.5 s (51m52.5s).**

| Check | Held | Seconds |
| --- | ---: | ---: |
| smoke | 32/32 | 15 |
| icon-buttons | 12/12 | 24 |
| shared-controls | 32/32 | 78 |
| focus | 80/80 | 294 |
| drag | 172/172 | 739 |
| responsive | 34/34 | 225 |
| safe-area | 90/90 | 81 |
| columns | 84/84 | 495 |
| committed-transcript | 114/114 | 7 |
| lease-details | 72/72 | 34 |
| load-fallback | 16/16 | 6 |
| gateway-states | 14/14 | 36 |
| conversation-unread | 4/4 | 6 |
| mode-publication | 2/2 | 11 |
| panel-list-follow | 2/2 | 13 |
| linked-devices | 14/14 | 24 |
| settings-page | 10/10 | 58 |
| widgets | 48/48 | 131 |
| subagents | 12/12 | 128 |
| mcp-apps | 92/92 | 412 |
| app-review | 32/32 | 191 |
| message-sync | 14/14 | 89 |
| command-order | 8/8 | 12 |

The four dev-only retained-app rows were explicitly not run inside the production message fixture. The separate actual dev run held 4/4 (7.9 s), at source9c1; production, fixture, selector/helper and script bytes relevant to that check are unchanged at05bc (source-identity.json). Mandatory signed-out scripted coverage held37/37 at9c1 (49.4 s: MCP16, gateway13, scenarios8), with the same executable/source identity. Neither is silently relabeled as a rerun at05bc.

| Production delivery | Runs | Active median/max ms | Held median/max ms | Max frame ms | Over50 |
| --- | ---: | ---: | ---: | ---: | ---: |
| chromium columns | 3 | 24.75/39.60 | 24.60/31.20 | 32.60 | 0 |
| chromium sidebar | 3 | 24.95/50.40 | 24.00/34.20 | 34.00 | 0 |
| webkit columns | 3 | 23.00/33.00 | 25.00/34.00 | 23.00 | 0 |
| webkit sidebar | 3 | 24.00/33.00 | 25.00/33.00 | 21.00 | 0 |

Chromium delivery uses calibrated4×CPU throttle; WebKit is unthrottled. Active delivery max50.4ms is latency against the unchanged600ms delivery contract; it is not a frame-budget failure. Maximum measured delivery frame34ms, zero over50. DOM plus two animation-frame opportunities do not claim transport/provider startup or compositor paint.

**Performance remains FAILED33/35** on production source9c1, byte-identical to05bc for this measurement: headed bundled Chromium, all default17 interactions/both layouts, three runs, throttle4. Calibration ratio2.94, known120ms frame measured120.5ms with LoAF attribution. Overview-answer columns max/median76.8/73.3ms and sidebar72.7/65.5ms, three over50 in each row. All other31 interaction rows pass. All six slow frames carry rootkeydownB_ script56–58ms/forcedlayout37–39ms. No budget increase or pass-seeking repetition occurred.

Actual baseline at main88f34e4d: selected drag-drop/overview-answer, five headed runs/layout, calibrated4×; all ten overview-answer runs fail50ms. Max/median87.9/76.5ms columns and75.6/73.8ms sidebar; drag passes10/10. Main later advanced to2d2a7a71, so this baseline is explicitly88, not mislabeled as the later tip. No underlying attribution to deferred#710 or product performance fix is claimed.

Initial clean9c1 full run978/990/exit1 and all original partial/failed rows remain under newcut-9c1c672f. Three verification readiness corrections, individual source-removal failures, original/boundary diagnostics, P2 closure, clean canonical tests and independent review are reported separately under readiness-author. Final05bc whole990/990 is actual fresh evidence, not corrected arithmetic over the earlier run.

Pixel-inspected actual small screenshots: mode-publication-unavailable-chromium (retained draft plus disabled Tool approval), panel-list-follow-live-chromium (already-mounted live Messages row), focus-retired-chromium-sidebar (composer surface after retirement). All are under functional/shots; no generated pixels or raw capability traces included.

All owned browsers, dev servers, gateways, probes, compilers and caffeinate processes ended. AC power100% charged at preparation; only bounded caffeinate-i used, no persistent power preference change. Unrelated Chrome47065 and Vite47997/nessa-agent-657 were untouched. Checkout remains clean exact05bc. Native WKWebView, live providers, OS file pickers, Linux installation and manual all-theme/zoom coverage are not claimed. The unchanged performance failure remains a merge blocker; all actual metrics and limits must remain in the PR.
