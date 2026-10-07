Verdict: fail

| Check | Chromium | WebKit |
| --- | --- | --- |
| mcp-apps-gateway | pass | pass |
| gateway-window | fail | pass |
| scripted-scenarios | pass | pass |

Relevant log lines:

- console: requestfailed: http://127.0.0.1:41849/browser/check net::ERR_ABORTED (aborted after a 204 response, #485) (harmless)
- console chromium: requestfailed: http://127.0.0.1:46355/mcp-resources net::ERR_ABORTED
- 2026-10-07T10:52:25.979448Z ERROR nessa_server::conversation::application::service: conversation agent opening failed conversation_id=4adfef5c-c4c7-4d0c-af08-60c92ebd6c8d error=agent: Unsupported("provider does not support restoring a closed session")
- 2026-10-07T10:52:42.355838Z ERROR nessa_server::conversation::application::service: conversation agent opening failed conversation_id=48ecf0a4-5f57-48f7-8623-b26777e6d13e error=agent: Unsupported("provider does not support restoring a closed session")
