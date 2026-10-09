# OAuth refresh gateway verification

Date: 2026-10-08 (America/Vancouver). Source: `becde6097`, base `45458da9d`.
macOS, Rust 1.98.1, Node 26.8.1, pnpm 11.9.0. The evidence commit adds this
report without changing executable source. This run is not a pass and does not
establish release readiness. The WebKit page error is tracked in #693.

Command: `pnpm test:e2e:scripted -- --evidence /tmp/nessa-audit-oauth-scripted`.
Exit: 1. Detailed local capture: `/tmp/nessa-audit-oauth-scripted`.

| Actual assertions held | Result |
| --- | --- |
| MCP Apps gateway | 16/17; global console guard failed |
| Gateway window | 14/15; WebKit console guard failed |
| Scripted scenarios | 9/10; WebKit console guard failed |

The generated summary below has an imperfect per-engine MCP Apps row: its global
console failure is retained in the actual counts above and the overall verdict.

## Generated scripted-run summary

Verdict: fail

| Check | Chromium | WebKit |
| --- | --- | --- |
| mcp-apps-gateway | pass | pass |
| gateway-window | pass | fail |
| scripted-scenarios | pass | fail |

Relevant log lines:

- console: requestfailed: http://127.0.0.1:50900/browser/check net::ERR_ABORTED (aborted after a 204 response, #485) (harmless)
- console: pageerror: ResizeObserver loop completed with undelivered notifications.
- console chromium: console.error: Failed to load resource: the server responded with a status of 404 (Not Found) (http://127.0.0.1:51127/favicon.ico) (harmless)
- console webkit: pageerror: ResizeObserver loop completed with undelivered notifications.
- console chromium: console.error: Failed to load resource: the server responded with a status of 404 (Not Found) (http://127.0.0.1:51320/favicon.ico) (harmless)
- console webkit: pageerror: ResizeObserver loop completed with undelivered notifications.
- 2026-10-09T02:58:18.767955Z  WARN nessa_sdk::infrastructure::acp::executions::worker: provider refused code=-32603 phase="prompt" the scenario stopped the turn
- 2026-10-09T02:58:21.824800Z ERROR nessa_server::conversation::application::service: conversation agent opening failed conversation_id=cf74b369-6215-4a0a-bb44-37ea995d8ac5 error=agent: Unsupported("provider does not support restoring a closed session")
- 2026-10-09T02:58:27.613510Z  WARN nessa_sdk::infrastructure::acp::executions::worker: provider refused code=-32603 phase="prompt" the scenario stopped the turn
- 2026-10-09T02:58:30.466730Z ERROR nessa_server::conversation::application::service: conversation agent opening failed conversation_id=729a0c3e-db17-4771-a03b-2da20e0b4008 error=agent: Unsupported("provider does not support restoring a closed session")

## Focused regression evidence

- Original omission reproduction: 1 failed, rotation control 1 passed.
- Rejected-bearer omission reproduction: 1 failed before the fix.
- Revert the retention rule: both omission regressions fail (0 passed / 2 failed).
- Restore the rule: authorization scope 88 passed / 0 failed.
- Workspace formatting, CI-selected all-target Clippy with warnings denied,
  ADR formatting and architecture boundary checks pass.

The owner tests use scripted HTTP, in-memory storage and an injected clock;
owner restoration is exercised, not a live provider or filesystem restart.
