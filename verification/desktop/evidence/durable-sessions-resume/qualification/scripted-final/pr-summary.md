Verdict: pass

| Check | Chromium | WebKit |
| --- | --- | --- |
| resize-observers | pass | pass |
| mcp-apps-gateway | pass | pass |
| gateway-window | pass | pass |
| scripted-scenarios | pass | pass |

Relevant log lines:

- console chromium: requestfailed: http://127.0.0.1:32895/browser/check net::ERR_ABORTED (aborted after a 204 response, #485) (harmless)
- console chromium: requestfailed: http://127.0.0.1:32895/mcp-resources net::ERR_ABORTED (aborted after a 200 response, full body, #473) (harmless)
- console chromium: requestfailed: http://127.0.0.1:35237/mcp-resources net::ERR_ABORTED (consumed EOF, verified length/SHA256, same uncanceled live mount at browser observer cutoff, #996) (harmless)
