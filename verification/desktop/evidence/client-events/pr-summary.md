# Client push event gateway verification

Date: 2026-10-08 (America/Vancouver). Source `6c8fae3b9`, base `45458da9d`.
macOS, Rust 1.98.1, Node 26.8.1, pnpm 11.9.0. This evidence commit changes no
executable source. The browser run failed and is not release clearance.

Command: `pnpm test:e2e:scripted -- --evidence /tmp/nessa-audit-events-scripted`.
Exit: 1. Detailed local evidence: `/tmp/nessa-audit-events-scripted`.

Actual assertion counts:

- mcp-apps-gateway: 16/17 assertions held; console guard failed.
- scripted-scenarios: 9/10 assertions held; console guard failed.
- gateway-window: 14/15 assertions held; console guard failed.

The same ResizeObserver error tracked in #693 occurs on this separate branch,
which has no OAuth owner changes. This is not a pristine-main or native-window
reproduction. The MCP Apps summary's per-engine row misses its global console
failure; retain the failed overall verdict and actual counts.

## Generated scripted-run summary

Verdict: fail

| Check | Chromium | WebKit |
| --- | --- | --- |
| mcp-apps-gateway | pass | pass |
| gateway-window | pass | fail |
| scripted-scenarios | pass | fail |

Relevant log lines:

- console: requestfailed: http://127.0.0.1:51995/browser/check net::ERR_ABORTED (aborted after a 204 response, #485) (harmless)
- console: pageerror: ResizeObserver loop completed with undelivered notifications.
- console chromium: console.error: Failed to load resource: the server responded with a status of 404 (Not Found) (http://127.0.0.1:52203/favicon.ico) (harmless)
- console webkit: pageerror: ResizeObserver loop completed with undelivered notifications.
- console chromium: console.error: Failed to load resource: the server responded with a status of 404 (Not Found) (http://127.0.0.1:52386/favicon.ico) (harmless)
- console webkit: pageerror: ResizeObserver loop completed with undelivered notifications.
- 2026-10-09T03:03:18.202966Z  WARN nessa_sdk::infrastructure::acp::executions::worker: provider refused code=-32603 phase="prompt" the scenario stopped the turn
- 2026-10-09T03:03:21.337534Z ERROR nessa_server::conversation::application::service: conversation agent opening failed conversation_id=4427ab4e-b105-45a4-afae-78de039ff896 error=agent: Unsupported("provider does not support restoring a closed session")
- 2026-10-09T03:03:26.870097Z  WARN nessa_sdk::infrastructure::acp::executions::worker: provider refused code=-32603 phase="prompt" the scenario stopped the turn
- 2026-10-09T03:03:29.760828Z ERROR nessa_server::conversation::application::service: conversation agent opening failed conversation_id=c0020d2f-6cdc-4cde-a5bc-806e56e8580a error=agent: Unsupported("provider does not support restoring a closed session")

## Focused validation

- Client suite: 706 passed / 0 failed across 31 files.
- Initial lifecycle regression: 4 failed / 5 passed. The unsubscribe control
  already passed before snapshotting.
- Snapshot, exception, unsubscribe and close rule-revert probes: each exit 1.
- Restored lifecycle scope: 9 passed / 0 failed.
- Client typecheck, changed-file lint/format, 74 architecture tests and the
  architecture boundary check passed.
- Independent review and CI remain pending.
