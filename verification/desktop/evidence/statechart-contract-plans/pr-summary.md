# Scripted gateway baseline for the statechart plans

Command: `pnpm test:e2e:scripted -- --channel bundled --evidence /tmp/nessa-statechart-plan-scripted`.

Checked source head: `b7813b30096d68bfba2fc32969828b79f2bc38d3`, on macOS
with a signed-out scripted provider and Playwright Chromium/WebKit. This run
checks the existing gateway/app/review/cancel baseline. It does not verify the
proposed subagent ownership or remote HTTP/OAuth product implementation.

The aggregate held 30 of 30 checks: MCP Apps 11/11, gateway window 11/11,
and permission/failure/cancel scenarios 8/8. Temporary gateways and dev servers
were stopped by their owning verification scripts.

Verdict: pass

| Check | Chromium | WebKit |
| --- | --- | --- |
| mcp-apps-gateway | pass | pass |
| gateway-window | pass | pass |
| scripted-scenarios | pass | pass |

Relevant log lines:

- renders chromium: requestfailed: http://127.0.0.1:54790/browser/check net::ERR_ABORTED (aborted after a 204 response, #485) (harmless)
