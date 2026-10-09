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

## Registration identity review follow-up

Date: 2026-10-08 (America/Vancouver). Executable source
`733a98be5492f0519ea01dfa08c50532037a7386`, same base `45458da9d`.
macOS, Node 26.8.1 / pnpm 11.9.0. Follow-up to the Codex finding on PR #692:
callback membership in `6c8fae3b9` could revive a removed snapshot entry when
that same function was registered again.

- Four new regressions fail on `daf8e0d3e`: same-function replacement ordering,
  stale unsubscribe handles, duplicate-handle removal after replacement, and
  nested event delivery. Nine existing lifecycle cases pass there.
- With the fix, all 710 client tests across 31 files pass; lifecycle 13/13.
- Six rule-revert probes, applied and verified separately, each exit 1:
  snapshotting, dispatch registration identity, unsubscribe registration identity,
  active registration reuse, terminal closure, exception isolation. After restoring
  the source, the lifecycle scope passes 13/13. Local logs:
  `/tmp/nessa-pr-692-review-fix`.
- Client typecheck, changed-file ESLint/Prettier, all 74 architecture tests,
  architecture boundary check, and `git diff --check` pass.
- The client README ordering table and transport module map track registration
  identity. Ownership stays in WireSession; no new dependencies or moved modules.

Command: `pnpm test:e2e:scripted -- --evidence /tmp/nessa-pr-692-review-scripted`.
Exit: 1. Actual assertions: MCP Apps gateway 16/17, gateway window 14/14,
scripted scenarios 9/10. All functional assertions pass. MCP Apps and scenarios
fail their WebKit console guards with the existing #693 ResizeObserver error.
The MCP Apps per-engine row still omits the global console failure; the overall
verdict stays failed. No native WKWebView or live-provider acceptance is claimed.

### Follow-up generated scripted-run summary

Verdict: fail

| Check | Chromium | WebKit |
| --- | --- | --- |
| mcp-apps-gateway | pass | pass |
| gateway-window | pass | pass |
| scripted-scenarios | pass | fail |

Relevant log lines:

- console: requestfailed: http://127.0.0.1:57366/browser/check net::ERR_ABORTED (aborted after a 204 response, #485) (harmless)
- console: pageerror: ResizeObserver loop completed with undelivered notifications.
- console chromium: console.error: Failed to load resource: the server responded with a status of 404 (Not Found) (http://127.0.0.1:57568/favicon.ico) (harmless)
- console chromium: console.error: Failed to load resource: the server responded with a status of 404 (Not Found) (http://127.0.0.1:57754/favicon.ico) (harmless)
- console webkit: pageerror: ResizeObserver loop completed with undelivered notifications.
- 2026-10-09T03:52:39.683227Z  WARN nessa_sdk::infrastructure::acp::executions::worker: provider refused code=-32603 phase="prompt" the scenario stopped the turn
- 2026-10-09T03:52:42.683555Z ERROR nessa_server::conversation::application::service: conversation agent opening failed conversation_id=8385eb35-e1b1-4ee4-85cb-4f5d79b55f32 error=agent: Unsupported("provider does not support restoring a closed session")
- 2026-10-09T03:52:48.086070Z  WARN nessa_sdk::infrastructure::acp::executions::worker: provider refused code=-32603 phase="prompt" the scenario stopped the turn
- 2026-10-09T03:52:50.944899Z ERROR nessa_server::conversation::application::service: conversation agent opening failed conversation_id=bd60a2be-7a9c-4c29-ba47-61eab7fe7267 error=agent: Unsupported("provider does not support restoring a closed session")

### Independent adversarial review

A fresh reviewer inspected clean executable head
`733a98be5492f0519ea01dfa08c50532037a7386` against base
`45458da9df5365014f8cc4f7337a85b91a63d4a7` before push.
No findings at any priority remain in the PR dispatcher scope.

Inspected: full PR diff; transport lifecycle/request cleanup; protocol dispatch;
handshake subscriptions; public client delegation; ManagedSession adoption and
recovery; README ordering table; transport module map; prior verification evidence.
Checked relationships: callback/registration identity, active dedup/stale handles,
snapshot/current membership, termination/delivery authority, event/session isolation,
persistent subscriptions/replacement transport ownership.

Independent checks: 10/10 adversarial probes (raw socket successive replacements,
self replacement/order, nested removal/addition, three-level replacement, duplicate
handles, event/session isolation, replacement then throw, nested closure, real
WireSession-backed ManagedSession recovery with stale handles). Focused lifecycle,
ordinary transport, and ManagedSession suites: 55/55. `git diff --check` passed.
Temporary review probes were removed and the reviewed checkout was clean.
The reviewer inspected the supplied full-suite/revert/static-check evidence without
rerunning those checks. No native WKWebView, live-provider, persistence/audit or
cross-platform acceptance was exercised by this review.

A separate pre-existing P2 finding in unchanged ManagedSession code is outside this
PR: two persistent registrations of one callback share one WireSession registration.
An off handle stops current delivery, but the remaining persistent record resumes
delivery on recovery. It requires its own issue and does not invalidate this fix.
The failed #693 scripted console guard remains a release limitation.
