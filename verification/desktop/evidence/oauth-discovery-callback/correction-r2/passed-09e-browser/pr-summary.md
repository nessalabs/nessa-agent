Verdict: pass

| Check | Chromium | WebKit |
| --- | --- | --- |
| mcp-apps-gateway | pass | pass |
| gateway-window | pass | pass |
| scripted-scenarios | pass | pass |

Relevant log lines:

- console: requestfailed: http://127.0.0.1:43843/browser/check net::ERR_ABORTED (aborted after a 204 response, #485) (harmless)
- console: requestfailed: http://127.0.0.1:43843/mcp-resources net::ERR_ABORTED (aborted after a 200 response, full body, #473) (harmless)
