# Parent Stop browser evidence

The screenshots show the ordinary parent transcript after Stop, with the
cancelled turn and a usable reply composer, for the default and explicit
`subagents.stopChildrenOnParentStop: false` settings.

Captured in Chromium with the **dev frontend** and actual gateway executable at
runtime commit `ffec1c29758d7f79d92d438c653d6426ef1dce0a`. Earlier production
wording for these screenshots was inaccurate. They illustrate the surface;
scripted assertions prove the ownership and delivery behavior.

## Earlier runtime verification

Runtime source `fd0a02e680d6d5371718921e5badf5693ffcc95b`, including main
`db681dbbc1d9650201433bc7cdd6842d3aefc3dd`, was clean throughout verification.
The actual gateway executable used Cargo's dev profile, SHA256
`8b614b2b9fcc0fc7685a1ecbf7be33b96c917365b92e1783e39607f37bcf68bd`.

- Production frontend, Chromium and WebKit: window 11/11, permission/failure/
  cancellation scenarios 8/8, parent Stop 4/4; **23/23 passed**.
- Focused Codex capability profile, production frontend, Chromium and WebKit:
  **4/4 passed on repeat**. The first run passed 3/4; WebKit override
  failed initial conversation admission before Stop assertions. Its cause remains
  unresolved, and the failed run is retained with the verification logs. The retained
  ACP trace stops before WebKit's initial parent scenario, page opening, or Stop.
  The helper retained only a generic mutation message; no raw RPC cause/code or
  request identity survived, so create versus submit and the underlying cause
  cannot be determined. No Stop assertion ran in that failed scenario.
- MCP Apps requires dev sandbox metadata and is intentionally skipped by the
  canonical production runner. Its separate dev frontend run in Chromium and
  WebKit: **16/16 passed**.

The checks cover recursive default, explicit override, grandchildren, unrelated
active and queued executions, no immediate parent restart, and
retained report delivery after later person input. Commands use the existing
`verification/desktop/scripts/scripted-e2e.mjs --mode prod`,
`parent-stop.mjs --mode prod --agent codex`, and
`mcp-apps-gateway.mjs --scripted --mode dev` entry points.
The pull request includes their short result lines and final source provenance.

All gateway configurations and provider fixtures used isolated temporary CI
namespaces. Live configuration, security settings and credentials were unchanged.
Playwright WebKit coverage does not claim native macOS WKWebView or live-account
provider verification.

## Current-main verification — 2026-10-19

Runtime head `a6e3f5e993d45db2ccceba51bd5a61b9df1519e3`, based on
main `9e480fa018ae0b93b8897ae5d4c08d4ea933f8fe`, passed the following
unchanged browser checks in Chromium and Playwright WebKit:

- Canonical dev frontend: 43/43; canonical production frontend: 23/23.
- Production parent Stop with the scripted Codex capability profile: 4/4.
- Default stress: 64/64; raised parent stops: 68/68.
- Additional raised seed 3, exercising parent Stop after recovery: 66/66.

Default chaos passed seed 1 and **failed seed 2**: after 182 actions, a resumed
`stress watch` execution remained unresolved at the original 240-second
settlement deadline. All eight built-in reduction attempts passed; they do not
invalidate the original failure. SDK replay of a copy of the preserved state
confirms a complete, current stream and that execution still owed. Cause remains
under investigation. This is a failed verification gate.

The four `parent-stop-{chromium,webkit}-{default,override}.png` files below are
fresh production-frontend pane captures, individually inspected at original
detail: complete title and assistant marker, Cancelled status, usable Reply
composer, and no clipped or misattributed text. The earlier two screenshots above
remain historical dev evidence.

![Chromium default](parent-stop-chromium-default.png)
![Chromium override](parent-stop-chromium-override.png)
![WebKit default](parent-stop-webkit-default.png)
![WebKit override](parent-stop-webkit-override.png)

The actual Cargo-dev/default-feature gateway SHA256 was
`e32069744b2ded05049f522db7c91864d12ccbf2b06afeffa59b11c2aba32d82`.
All seven command groups joined and physically reaped their owned descendants;
the browser-process baseline remained 249, with zero new processes. Stress
retained every planned row, 12 same-target aside pairs, and 12 fresh ACP session
observations. These observations are not an independent raw receipt of the
runner's `checked` event.

Subsequent main `c0119e8b35b8b537b701638ee87a1a682c74cff8` adds only a
SDK record-writer test. Its integration at `23707631d1438e3907ce45c55fe04efb707dc7df`
has byte-identical production/frontend/browser inputs; the new test passed
separately, with SDK all-target Clippy and formatting. The browser runs above
were executed at a6, not rerun at that merge.

Required CI on a6 failed the Windows peer-forget test, tracked in
[#1032](https://github.com/nessalabs/nessa-agent/issues/1032); its cause is not
established. The distinct post-merge #1029 retained-read failures remain tracked
in [#1030](https://github.com/nessalabs/nessa-agent/issues/1030). Neither failure
has been waived or resolved by rerunning CI. Fresh source reviews cannot waive
these failed gates. No native WKWebView, live-provider or release-gateway result
is claimed.
