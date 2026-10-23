Verdict: could-not-run

| Check | Chromium | WebKit |
| --- | --- | --- |
| resize-observers | pass | could-not-run |
| mcp-apps-gateway | pass | could-not-run |
| gateway-window | pass | could-not-run |
| scripted-scenarios | pass | could-not-run |

Relevant log lines:

- console chromium: console.error: Failed to load resource: the server responded with a status of 404 (Not Found) (http://127.0.0.1:1438/favicon.ico) (harmless)
- launch webkit: webkit did not launch (browserType.launch: Executable doesn't exist at /tmp/nessa-playwright/webkit-2359/pw_run.sh). Run: pnpm exec playwright install webkit
- console chromium: requestfailed: http://127.0.0.1:41813/browser/check net::ERR_ABORTED (aborted after a 204 response, #485) (harmless)
- launch webkit: webkit did not launch (browserType.launch: Executable doesn't exist at /tmp/nessa-playwright/webkit-2359/pw_run.sh). Run: pnpm exec playwright install webkit
- launch webkit: webkit did not launch (browserType.launch: Executable doesn't exist at /tmp/nessa-playwright/webkit-2359/pw_run.sh). Run: pnpm exec playwright install webkit
- console chromium: console.error: Failed to load resource: the server responded with a status of 404 (Not Found) (http://127.0.0.1:45907/favicon.ico) (harmless)
- launch webkit: webkit did not launch (browserType.launch: Executable doesn't exist at /tmp/nessa-playwright/webkit-2359/pw_run.sh). Run: pnpm exec playwright install webkit
