Verdict: fail

| Check | Chromium | WebKit |
| --- | --- | --- |
| mcp-apps-gateway | pass | pass |
| gateway-window | fail | fail |
| scripted-scenarios | fail | fail |

Relevant log lines:

- console: console.error: Failed to load resource: the server responded with a status of 403 (Forbidden) (http://127.0.0.1:32933/@fs/workspace/nessa-agent/node_modules/.pnpm/@fontsource-variable+geist@5.3.0/node_modules/@fontsource-variable/geist/files/geist-latin-wght-normal.woff2)
- console: requestfailed: http://127.0.0.1:32933/@fs/workspace/nessa-agent/node_modules/.pnpm/@fontsource-variable+geist@5.3.0/node_modules/@fontsource-variable/geist/files/geist-latin-wght-normal.woff2 net::ERR_ABORTED
- console: console.error: Failed to load resource: the server responded with a status of 403 (Forbidden) (http://127.0.0.1:32933/@fs/workspace/nessa-agent/node_modules/.pnpm/@fontsource-variable+geist-mono@5.3.0/node_modules/@fontsource-variable/geist-mono/files/geist-mono-latin-wght-normal.woff2)
- console: requestfailed: http://127.0.0.1:32933/@fs/workspace/nessa-agent/node_modules/.pnpm/@fontsource-variable+geist-mono@5.3.0/node_modules/@fontsource-variable/geist-mono/files/geist-mono-latin-wght-normal.woff2 net::ERR_ABORTED
- console: requestfailed: http://127.0.0.1:32933/browser/check net::ERR_ABORTED (aborted after a 204 response, #485) (harmless)
- console: console.error: Failed to load resource: the server responded with a status of 403 (Forbidden) (http://127.0.0.1:32933/@fs/workspace/nessa-agent/node_modules/.pnpm/@fontsource-variable+geist@5.3.0/node_modules/@fontsource-variable/geist/files/geist-latin-wght-normal.woff2)
- console: requestfailed: http://127.0.0.1:32933/@fs/workspace/nessa-agent/node_modules/.pnpm/@fontsource-variable+geist@5.3.0/node_modules/@fontsource-variable/geist/files/geist-latin-wght-normal.woff2 Load request cancelled
- console: console.error: Failed to load resource: the server responded with a status of 403 (Forbidden) (http://127.0.0.1:32933/@fs/workspace/nessa-agent/node_modules/.pnpm/@fontsource-variable+geist-mono@5.3.0/node_modules/@fontsource-variable/geist-mono/files/geist-mono-latin-wght-normal.woff2)
- console: requestfailed: http://127.0.0.1:32933/@fs/workspace/nessa-agent/node_modules/.pnpm/@fontsource-variable+geist-mono@5.3.0/node_modules/@fontsource-variable/geist-mono/files/geist-mono-latin-wght-normal.woff2 Load request cancelled
- console chromium: console.error: Failed to load resource: the server responded with a status of 403 (Forbidden) (http://127.0.0.1:35627/@fs/workspace/nessa-agent/node_modules/.pnpm/@fontsource-variable+geist@5.3.0/node_modules/@fontsource-variable/geist/files/geist-latin-wght-normal.woff2)
- console chromium: requestfailed: http://127.0.0.1:35627/@fs/workspace/nessa-agent/node_modules/.pnpm/@fontsource-variable+geist@5.3.0/node_modules/@fontsource-variable/geist/files/geist-latin-wght-normal.woff2 net::ERR_ABORTED
- console chromium: console.error: Failed to load resource: the server responded with a status of 403 (Forbidden) (http://127.0.0.1:35627/@fs/workspace/nessa-agent/node_modules/.pnpm/@fontsource-variable+geist-mono@5.3.0/node_modules/@fontsource-variable/geist-mono/files/geist-mono-latin-wght-normal.woff2)
- console chromium: requestfailed: http://127.0.0.1:35627/@fs/workspace/nessa-agent/node_modules/.pnpm/@fontsource-variable+geist-mono@5.3.0/node_modules/@fontsource-variable/geist-mono/files/geist-mono-latin-wght-normal.woff2 net::ERR_ABORTED
- console webkit: console.error: Failed to load resource: the server responded with a status of 403 (Forbidden) (http://127.0.0.1:35627/@fs/workspace/nessa-agent/node_modules/.pnpm/@fontsource-variable+geist@5.3.0/node_modules/@fontsource-variable/geist/files/geist-latin-wght-normal.woff2)
- console webkit: requestfailed: http://127.0.0.1:35627/@fs/workspace/nessa-agent/node_modules/.pnpm/@fontsource-variable+geist@5.3.0/node_modules/@fontsource-variable/geist/files/geist-latin-wght-normal.woff2 Load request cancelled
- console webkit: console.error: Failed to load resource: the server responded with a status of 403 (Forbidden) (http://127.0.0.1:35627/@fs/workspace/nessa-agent/node_modules/.pnpm/@fontsource-variable+geist-mono@5.3.0/node_modules/@fontsource-variable/geist-mono/files/geist-mono-latin-wght-normal.woff2)
- console webkit: requestfailed: http://127.0.0.1:35627/@fs/workspace/nessa-agent/node_modules/.pnpm/@fontsource-variable+geist-mono@5.3.0/node_modules/@fontsource-variable/geist-mono/files/geist-mono-latin-wght-normal.woff2 Load request cancelled
- console chromium: console.error: Failed to load resource: the server responded with a status of 403 (Forbidden) (http://127.0.0.1:35719/@fs/workspace/nessa-agent/node_modules/.pnpm/@fontsource-variable+geist@5.3.0/node_modules/@fontsource-variable/geist/files/geist-latin-wght-normal.woff2)
- console chromium: requestfailed: http://127.0.0.1:35719/@fs/workspace/nessa-agent/node_modules/.pnpm/@fontsource-variable+geist@5.3.0/node_modules/@fontsource-variable/geist/files/geist-latin-wght-normal.woff2 net::ERR_ABORTED
- console chromium: console.error: Failed to load resource: the server responded with a status of 403 (Forbidden) (http://127.0.0.1:35719/@fs/workspace/nessa-agent/node_modules/.pnpm/@fontsource-variable+geist-mono@5.3.0/node_modules/@fontsource-variable/geist-mono/files/geist-mono-latin-wght-normal.woff2)
- console chromium: requestfailed: http://127.0.0.1:35719/@fs/workspace/nessa-agent/node_modules/.pnpm/@fontsource-variable+geist-mono@5.3.0/node_modules/@fontsource-variable/geist-mono/files/geist-mono-latin-wght-normal.woff2 net::ERR_ABORTED
- console webkit: console.error: Failed to load resource: the server responded with a status of 403 (Forbidden) (http://127.0.0.1:35719/@fs/workspace/nessa-agent/node_modules/.pnpm/@fontsource-variable+geist@5.3.0/node_modules/@fontsource-variable/geist/files/geist-latin-wght-normal.woff2)
- console webkit: requestfailed: http://127.0.0.1:35719/@fs/workspace/nessa-agent/node_modules/.pnpm/@fontsource-variable+geist@5.3.0/node_modules/@fontsource-variable/geist/files/geist-latin-wght-normal.woff2 Load request cancelled
- console webkit: console.error: Failed to load resource: the server responded with a status of 403 (Forbidden) (http://127.0.0.1:35719/@fs/workspace/nessa-agent/node_modules/.pnpm/@fontsource-variable+geist-mono@5.3.0/node_modules/@fontsource-variable/geist-mono/files/geist-mono-latin-wght-normal.woff2)
- console webkit: requestfailed: http://127.0.0.1:35719/@fs/workspace/nessa-agent/node_modules/.pnpm/@fontsource-variable+geist-mono@5.3.0/node_modules/@fontsource-variable/geist-mono/files/geist-mono-latin-wght-normal.woff2 Load request cancelled
- 2026-10-07T07:48:35.461161Z ERROR nessa_server::conversation::application::service: conversation agent opening failed conversation_id=e3e9dc14-2627-47fd-af45-d8ab5644fcf9 error=agent: Unsupported("provider does not support restoring a closed session")
- 2026-10-07T07:48:50.741393Z ERROR nessa_server::conversation::application::service: conversation agent opening failed conversation_id=5181ddae-1ade-47fd-9b39-879d0bf8036b error=agent: Unsupported("provider does not support restoring a closed session")
