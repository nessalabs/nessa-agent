Verdict: fail

| Check | Chromium | WebKit |
| --- | --- | --- |
| mcp-apps-gateway | fail | pass |
| gateway-window | pass | pass |
| scripted-scenarios | pass | pass |

Relevant log lines:

- deny chromium: frame.evaluate: Frame was detached
- release chromium: not run: deny failed
- message chromium: not run: deny failed
- context chromium: not run: deny failed
- console: requestfailed: http://127.0.0.1:59549/browser/check net::ERR_ABORTED (aborted after a 204 response, #485) (harmless)
- console: requestfailed: http://127.0.0.1:59549/browser/check net::ERR_ABORTED (aborted after a 204 response, #485) (harmless)
- 2026-10-07T02:39:22.916190Z  WARN nessa_sdk::infrastructure::acp::executions::worker: provider refused code=-32603 phase="prompt" the scenario stopped the turn
- 2026-10-07T02:39:26.483627Z ERROR nessa_server::conversation::application::service: conversation agent opening failed conversation_id=b7a10bbc-547d-485c-95fe-38dfd7810a93 error=agent: Unsupported("provider does not support restoring a closed session")
- 2026-10-07T02:39:31.072497Z  WARN nessa_sdk::infrastructure::acp::executions::worker: provider refused code=-32603 phase="prompt" the scenario stopped the turn
- 2026-10-07T02:39:34.179050Z ERROR nessa_server::conversation::application::service: conversation agent opening failed conversation_id=2886219a-0d1a-492f-80e3-72babd529783 error=agent: Unsupported("provider does not support restoring a closed session")
