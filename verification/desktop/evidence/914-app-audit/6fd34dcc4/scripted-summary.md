Verdict: pass

| Check | Chromium | WebKit |
| --- | --- | --- |
| mcp-apps-gateway | not run: dev server only | not run: dev server only |
| gateway-window | pass | pass |
| scripted-scenarios | pass | pass |

Relevant log lines:

- console chromium: console.error: Failed to load resource: the server responded with a status of 404 (Not Found) (http://127.0.0.1:53476/favicon.ico) (harmless)
- console chromium: console.error: Failed to load resource: the server responded with a status of 404 (Not Found) (http://127.0.0.1:53709/favicon.ico) (harmless)
