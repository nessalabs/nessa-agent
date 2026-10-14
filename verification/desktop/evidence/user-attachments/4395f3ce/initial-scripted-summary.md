Verdict: fail

| Check | Chromium | WebKit |
| --- | --- | --- |
| mcp-apps-gateway | pass | pass |
| gateway-window | fail | pass |
| scripted-scenarios | pass | pass |

Relevant log lines:

- console chromium: requestfailed: http://127.0.0.1:52123/browser/check net::ERR_ABORTED (aborted after a 204 response, #485) (harmless)
- console chromium: requestfailed: http://127.0.0.1:52759/desktop.html net::ERR_ABORTED
- console chromium: console.error: Failed to load resource: the server responded with a status of 404 (Not Found) (http://127.0.0.1:52759/favicon.ico) (harmless)
- console chromium: console.error: Failed to load resource: the server responded with a status of 404 (Not Found) (http://127.0.0.1:53309/favicon.ico) (harmless)
- 2026-10-14T21:57:53.313889Z  WARN nessa_sdk::infrastructure::acp::executions::worker: provider refused code=-32603 phase="prompt" the scenario stopped the turn
- 2026-10-14T21:57:58.761519Z ERROR nessa_server::conversation::application::service: conversation agent opening failed conversation_id=f68768d4-873c-4ae2-bb71-39b109ca2647 error=agent: Unsupported("provider does not support restoring a closed session")
- 2026-10-14T21:58:10.563050Z  WARN nessa_sdk::infrastructure::acp::executions::worker: provider refused code=-32603 phase="prompt" the scenario stopped the turn
- 2026-10-14T21:58:15.291512Z ERROR nessa_server::conversation::application::service: conversation agent opening failed conversation_id=11ec7173-24b4-4c10-9b43-957e0d739e66 error=agent: Unsupported("provider does not support restoring a closed session")
