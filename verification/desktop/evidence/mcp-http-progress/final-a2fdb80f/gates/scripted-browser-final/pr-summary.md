Verdict: fail

| Check | Chromium | WebKit |
| --- | --- | --- |
| mcp-apps-gateway | pass | pass |
| gateway-window | fail | pass |
| scripted-scenarios | pass | pass |

Relevant log lines:

- console: requestfailed: http://127.0.0.1:44179/browser/check net::ERR_ABORTED (aborted after a 204 response, #485) (harmless)
- console chromium: requestfailed: http://127.0.0.1:45625/mcp-resources net::ERR_ABORTED
- 2026-10-08T06:44:58.871367Z ERROR nessa_server::conversation::application::service: conversation agent opening failed conversation_id=3c94c86d-601e-4a2b-b10f-cb5f3370a565 error=agent: Unsupported("provider does not support restoring a closed session")
- 2026-10-08T06:45:16.138555Z ERROR nessa_server::conversation::application::service: conversation agent opening failed conversation_id=62f24343-7ed2-4c61-bfaf-bcf6dd1230d3 error=agent: Unsupported("provider does not support restoring a closed session")
