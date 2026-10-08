Verdict: fail

| Check | Chromium | WebKit |
| --- | --- | --- |
| mcp-apps-gateway | pass | pass |
| gateway-window | fail | pass |
| scripted-scenarios | pass | pass |

Relevant log lines:

- console: requestfailed: http://127.0.0.1:38241/browser/check net::ERR_ABORTED (aborted after a 204 response, #485) (harmless)
- console chromium: requestfailed: http://127.0.0.1:35501/mcp-resources net::ERR_ABORTED
- 2026-10-08T07:14:46.284313Z ERROR nessa_server::conversation::application::service: conversation agent opening failed conversation_id=ec748641-8aa7-4be5-8759-794f0c770f8c error=agent: Unsupported("provider does not support restoring a closed session")
- 2026-10-08T07:15:02.751922Z ERROR nessa_server::conversation::application::service: conversation agent opening failed conversation_id=f87de8c2-f823-4666-b39e-21b22c871967 error=agent: Unsupported("provider does not support restoring a closed session")
