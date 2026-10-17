Verdict: could-not-run

| Check | Chromium | WebKit |
| --- | --- | --- |
| mcp-apps-gateway | pass | could-not-run |
| gateway-window | pass | could-not-run |
| scripted-scenarios | pass | could-not-run |

Relevant log lines:

- console chromium: requestfailed: http://127.0.0.1:35295/browser/check net::ERR_ABORTED (aborted after a 204 response, #485) (harmless)
- console chromium: requestfailed: http://127.0.0.1:35295/mcp-resources net::ERR_ABORTED (aborted after a 200 response, full body, #473) (harmless)
- launch webkit: webkit did not launch (browserType.launch: ). Run: pnpm exec playwright install webkit
- launch webkit: webkit did not launch (browserType.launch: ). Run: pnpm exec playwright install webkit
- launch webkit: webkit did not launch (browserType.launch: ). Run: pnpm exec playwright install webkit
- 2026-10-17T02:02:24.390342Z ERROR nessa_server::conversation::application::service: conversation agent opening failed conversation_id=7a9bd655-956b-473a-8897-2f29c1c2f0bc error=agent: Unsupported("provider does not support restoring a closed session")
