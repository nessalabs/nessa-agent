# Native retained gateway review

The signed-out scripted gateway checks passed on the local verification merge
`52e2d86abfc1a2e66ef32e73cf767fd272e155d6`, combining native PR head
`13acd0206406193320e7c7e702e48990fd6b442d` with main
`8468b36bd2a8e58b68f7b6c2d18dcc98313196f0`. The merge required no source edits.

Command: `node verification/desktop/scripts/scripted-e2e.mjs --channel bundled`.
The run used the frozen source, its compiled gateway, its pinned UI checkout,
and locally installed Chromium/WebKit. [checks.json](checks.json) retains each
check outcome.

Verdict: **pass**

| Check | Chromium | WebKit | Shared setup |
| --- | --- | --- | --- |
| mcp-apps-gateway | 8/8 pass | 7/7 pass | 1/1 pass |
| gateway-window | 6/6 pass | 6/6 pass | 1/1 pass |
| scripted-scenarios | 4/4 pass | 4/4 pass | — |

Relevant line: `requestfailed: /browser/check net::ERR_ABORTED` followed a 204
response and is the existing harmless #485 case. All recorded checks passed.

The exact seven-package Rust selection also passed against the then-current
main integration candidate: server 2,662 tests, SDK 1,212 tests, and the separate
application, enrollment, core, native and documentation selections.

The consent-class getter was independently reviewed at `13acd020`; all three
Auth classes across seven receipt phases and retry preserve the Auth-owned wire
bytes. A deliberate debug-spelling mutation fails the regression; restored
native tests pass. Fresh generated Swift compiled with Swift 6 complete
concurrency checks and warnings denied, and its five paired lifecycle stages
passed with independently supplied Auth class names.
