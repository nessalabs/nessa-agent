Verdict: fail

| Check | Chromium | WebKit |
| --- | --- | --- |
| mcp-apps-gateway | pass | pass |
| gateway-window | fail | pass |
| scripted-scenarios | pass | pass |

Relevant log lines:

- console: requestfailed: http://127.0.0.1:36041/browser/check net::ERR_ABORTED (aborted after a 204 response, #485) (harmless)
- console chromium: requestfailed: http://127.0.0.1:37323/mcp-resources net::ERR_ABORTED
- 2026-10-07T06:25:24.910413Z ERROR nessa_server::conversation::application::service: conversation agent opening failed conversation_id=ae1f3e94-d8dd-4125-b4b7-d0dc189294dd error=agent: Unsupported("provider does not support restoring a closed session")
- 2026-10-07T06:25:41.486901Z ERROR nessa_server::conversation::application::service: conversation agent opening failed conversation_id=b1111df4-294d-40bf-ab65-c15aac88ab91 error=agent: Unsupported("provider does not support restoring a closed session")
