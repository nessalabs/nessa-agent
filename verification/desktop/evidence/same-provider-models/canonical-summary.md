Source: `f9b861d7f98547dc98f30f1afdffe6fe0538397f`, immutable main c011.
Gateway SHA256: `000f43485fdd52788dbb747234d63632c27be432dadab75b390d8166c4084acf`.
Later checker commit `76e33529412a833eeab4c9f00da669f5a1c94774` changes only session-row verification and CHECKLIST; canonical inputs and gateway are byte-identical. UI pin: `7a8fe71fe996305a71baba319387df6e81ffbe0e`.

Verdict: pass

| Check | Chromium | WebKit |
| --- | --- | --- |
| mcp-apps-gateway | pass | pass |
| gateway-window | pass | pass |
| scripted-scenarios | pass | pass |

Relevant log lines:

- console chromium: requestfailed: http://127.0.0.1:54273/browser/check net::ERR_ABORTED (aborted after a 204 response, #485) (harmless)
- console chromium: console.error: Failed to load resource: the server responded with a status of 404 (Not Found) (http://127.0.0.1:55155/favicon.ico) (harmless)
