# Parent Stop browser evidence

The screenshots show the ordinary parent transcript after Stop, with the
cancelled turn and a usable reply composer, for the default and explicit
`subagents.stopChildrenOnParentStop: false` settings.

Captured in Chromium with the **dev frontend** and actual gateway executable at
runtime commit `ffec1c29758d7f79d92d438c653d6426ef1dce0a`. Earlier production
wording for these screenshots was inaccurate. They illustrate the surface;
scripted assertions prove the ownership and delivery behavior.

## Final runtime verification

Runtime source `3541e761a0a86c00f036b16708f3f303b3fd5ca5`, including main
`82bc38e7a1715cc7b3440f4f24e5d6e46522d4a4`, was clean throughout verification.
The actual gateway executable used Cargo's dev profile, SHA256
`5ea0849d60a47f7cf8b895afc5976162eb081d08a0dc371c9844b955683f2329`.

- Production frontend, Chromium and WebKit: window 11/11, permission/failure/
  cancellation scenarios 8/8, parent Stop 4/4; **23/23 passed**.
- Focused Codex capability profile, production frontend, Chromium and WebKit:
  **4/4 passed**.
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
