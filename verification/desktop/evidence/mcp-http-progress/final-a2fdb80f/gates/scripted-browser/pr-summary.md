Verdict: fail

| Check | Chromium | WebKit |
| --- | --- | --- |
| mcp-apps-gateway | could-not-run | could-not-run |
| gateway-window | fail | could-not-run |
| scripted-scenarios | fail | could-not-run |

Relevant log lines:

- console: console.error: Failed to load resource: the server responded with a status of 500 (Internal Server Error) (http://127.0.0.1:45325/src/desktop/workspace/ui/quick-switcher/quick-switcher.tsx)
- console: requestfailed: http://127.0.0.1:45325/src/desktop/workspace/ui/quick-switcher/quick-switcher.tsx net::ERR_ABORTED
- console: console.error: Failed to load resource: the server responded with a status of 500 (Internal Server Error) (http://127.0.0.1:45325/src/desktop/workspace/ui/session-list/session-list.tsx)
- console: requestfailed: http://127.0.0.1:45325/src/desktop/workspace/ui/session-list/session-list.tsx net::ERR_ABORTED
- console: console.error: Failed to load resource: the server responded with a status of 500 (Internal Server Error) (http://127.0.0.1:45325/src/desktop/workspace/ui/source-list/source-list.tsx)
- console: requestfailed: http://127.0.0.1:45325/src/desktop/workspace/ui/source-list/source-list.tsx net::ERR_ABORTED
- console: console.error: [nessa] Could not load http://127.0.0.1:45325/desktop.html?gateway. The dev server did not serve /src/desktop/main.tsx. (http://127.0.0.1:45325/desktop.html?gateway) (harmless)
- open chromium: could not run: the desktop page did not render [data-pane-key], [data-surface] at http://127.0.0.1:45325/desktop.html?gateway: page.waitForSelector: Timeout 30000ms exceeded.
- renders chromium: not run: the page did not open
- hidden chromium: not run: the page did not open
- allow chromium: not run: the page did not open
- deny chromium: not run: the page did not open
- release chromium: not run: the page did not open
- message chromium: not run: the page did not open
- context chromium: not run: the page did not open
- launch webkit: webkit did not launch (browserType.launch: ). Run: pnpm exec playwright install webkit
- console chromium: console.error: Failed to load resource: the server responded with a status of 500 (Internal Server Error) (http://127.0.0.1:37279/src/desktop/workspace/ui/session-list/session-list.tsx)
- console chromium: requestfailed: http://127.0.0.1:37279/src/desktop/workspace/ui/session-list/session-list.tsx net::ERR_ABORTED
- console chromium: console.error: Failed to load resource: the server responded with a status of 500 (Internal Server Error) (http://127.0.0.1:37279/src/desktop/workspace/ui/quick-switcher/quick-switcher.tsx)
- console chromium: requestfailed: http://127.0.0.1:37279/src/desktop/workspace/ui/quick-switcher/quick-switcher.tsx net::ERR_ABORTED
- console chromium: console.error: Failed to load resource: the server responded with a status of 500 (Internal Server Error) (http://127.0.0.1:37279/src/desktop/workspace/ui/source-list/source-list.tsx)
- console chromium: requestfailed: http://127.0.0.1:37279/src/desktop/workspace/ui/source-list/source-list.tsx net::ERR_ABORTED
- console chromium: console.error: [nessa] Could not load http://127.0.0.1:37279/desktop.html. The dev server did not serve /src/desktop/main.tsx. (http://127.0.0.1:37279/desktop.html) (harmless)
- open chromium: could not run: the desktop page did not render [data-pane-key], [data-surface] at http://127.0.0.1:37279/desktop.html: page.waitForSelector: Timeout 30000ms exceeded.
- handshake chromium: not run: the page did not open
- lists chromium: not run: the page did not open
- opens chromium: not run: the page did not open
- live chromium: not run: the page did not open
- apps chromium: not run: the page did not open
- launch webkit: webkit did not launch (browserType.launch: ). Run: pnpm exec playwright install webkit
- console chromium: console.error: Failed to load resource: the server responded with a status of 500 (Internal Server Error) (http://127.0.0.1:37917/src/desktop/workspace/ui/session-list/session-list.tsx)
- console chromium: requestfailed: http://127.0.0.1:37917/src/desktop/workspace/ui/session-list/session-list.tsx net::ERR_ABORTED
- console chromium: console.error: Failed to load resource: the server responded with a status of 500 (Internal Server Error) (http://127.0.0.1:37917/src/desktop/workspace/ui/quick-switcher/quick-switcher.tsx)
- console chromium: requestfailed: http://127.0.0.1:37917/src/desktop/workspace/ui/quick-switcher/quick-switcher.tsx net::ERR_ABORTED
- console chromium: console.error: Failed to load resource: the server responded with a status of 500 (Internal Server Error) (http://127.0.0.1:37917/src/desktop/workspace/ui/source-list/source-list.tsx)
- console chromium: requestfailed: http://127.0.0.1:37917/src/desktop/workspace/ui/source-list/source-list.tsx net::ERR_ABORTED
- console chromium: console.error: [nessa] Could not load http://127.0.0.1:37917/desktop.html. The dev server did not serve /src/desktop/main.tsx. (http://127.0.0.1:37917/desktop.html) (harmless)
- open chromium: could not run: the desktop page did not render [data-pane-key], [data-surface] at http://127.0.0.1:37917/desktop.html: page.waitForSelector: Timeout 30000ms exceeded.
- permission chromium: not run: the page did not open
- allow chromium: not run: the page did not open
- fail chromium: not run: the page did not open
- cancel chromium: not run: the page did not open
- launch webkit: webkit did not launch (browserType.launch: ). Run: pnpm exec playwright install webkit
