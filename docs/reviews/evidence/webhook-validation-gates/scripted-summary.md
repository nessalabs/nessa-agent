Verdict: could-not-run

| Check | Chromium | WebKit |
| --- | --- | --- |
| resize-observers | could-not-run | could-not-run |
| mcp-apps-gateway | could-not-run | could-not-run |
| gateway-window | could-not-run | could-not-run |
| scripted-scenarios | could-not-run | could-not-run |

Relevant log lines:

- launch chromium: chromium did not launch (browserType.launch: Executable doesn't exist at /home/agent/.cache/ms-playwright/chromium_headless_shell-1243/chrome-headless-shell-linux64/chrome-headless-shell). Run: pnpm exec playwright install chromium
- launch webkit: webkit did not launch (browserType.launch: Executable doesn't exist at /home/agent/.cache/ms-playwright/webkit-2359/pw_run.sh). Run: pnpm exec playwright install webkit
- mcp-apps-gateway: could not run: the gateway did not start: build the gateway first (cargo build -p nessa-server), or set MCP_LIVE_NESSA
- gateway-window: could not run: the gateway did not start: build the gateway first (cargo build -p nessa-server), or set MCP_LIVE_NESSA
- scripted-scenarios: could not run: the gateway did not start: build the gateway first (cargo build -p nessa-server), or set MCP_LIVE_NESSA
