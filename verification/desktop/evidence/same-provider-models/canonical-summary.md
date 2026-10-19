Source: `66f0dfbe78abbf4fe14c190189d2eb4c44d782f9`, assembled immutable main73fab.
Gateway SHA256: `ec0c3b79ccc564a79428102f35ea822ed7d67d114ed86a7d0f2d17192763aaaa`.
Final source57b0e0f changes only the stress observer/test and CHECKLIST; canonical
program, SDK, server, frontend/client/protocol and gateway bytes are unchanged.

Verdict: pass

| Check | Chromium | WebKit |
| --- | --- | --- |
| mcp-apps-gateway | pass | pass |
| gateway-window | pass | pass |
| scripted-scenarios | pass | pass |

Relevant log lines:

- console chromium: requestfailed: http://127.0.0.1:56061/browser/check net::ERR_ABORTED (aborted after a 204 response, #485) (harmless)
- console chromium: console.error: Failed to load resource: the server responded with a status of 404 (Not Found) (http://127.0.0.1:56967/favicon.ico) (harmless)
