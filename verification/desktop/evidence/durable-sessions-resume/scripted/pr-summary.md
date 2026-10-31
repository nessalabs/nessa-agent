Verdict: could-not-run

| Check | Chromium | WebKit |
| --- | --- | --- |
| resize-observers | pass | could-not-run |
| mcp-apps-gateway | not run: dev server only | not run: dev server only |
| gateway-window | could-not-run | could-not-run |
| scripted-scenarios | could-not-run | could-not-run |

Relevant log lines:

- launch webkit: webkit did not launch (browserType.launch: ). Run: pnpm exec playwright install webkit
- gateway-window: could not run: the gateway refused the claude conversation (a live claude: is it signed in on this machine?): Conversation command failed
- 2026-10-30T04:49:27.394141Z ERROR nessa_server::conversation::application::service: conversation agent opening failed conversation_id=fcba4505-96b5-489c-aaec-1433fd1fe829 error=agent: OperationAndCleanupFailure { operation_error: Transport("provider stdout closed"), cleanup_error: CleanupUncertain }
- 2026-10-30T04:49:34.446304Z ERROR nessa_server::composition::root: gateway shutdown did not confirm all cleanup error=stage reached: complete; webhooks ok; record reader ok; catalogue reader ok; review audit ok; conversations failed Retirement([("fcba4505-96b5-489c-aaec-1433fd1fe829", OperationAndCleanupFailure { operation_error: Transport("provider stdout closed"), cleanup_error: CleanupUncertain })]); MCP stop ok; native drain ok
- 2026-10-30T04:49:34.446955Z ERROR nessa_server::core::ending: nessa failed error=shutdown did not confirm all cleanup: stage reached: complete; webhooks ok; record reader ok; catalogue reader ok; review audit ok; conversations failed Retirement([("fcba4505-96b5-489c-aaec-1433fd1fe829", OperationAndCleanupFailure { operation_error: Transport("provider stdout closed"), cleanup_error: CleanupUncertain })]); MCP stop ok; native drain ok exit_code=27 reason="shutdown" restart=Worthwhile recorded=false
- ask chromium: could not run: the gateway refused the claude conversation (a live claude: is it signed in on this machine?): Conversation command failed
- permission chromium: not run: the permission was not asked
- allow chromium: not run: the permission was not asked
- fail chromium: not run: the permission was not asked
- cancel chromium: not run: the permission was not asked
- launch webkit: webkit did not launch (browserType.launch: ). Run: pnpm exec playwright install webkit
- 2026-10-30T04:50:15.583705Z ERROR nessa_server::conversation::application::service: conversation agent opening failed conversation_id=c9962288-659d-4f96-b45f-7d73adb55b41 error=agent: OperationAndCleanupFailure { operation_error: Transport("provider stdout closed"), cleanup_error: CleanupUncertain }
- 2026-10-30T04:50:22.940415Z ERROR nessa_server::composition::root: gateway shutdown did not confirm all cleanup error=stage reached: complete; webhooks ok; record reader ok; catalogue reader ok; review audit ok; conversations failed Retirement([("c9962288-659d-4f96-b45f-7d73adb55b41", OperationAndCleanupFailure { operation_error: Transport("provider stdout closed"), cleanup_error: CleanupUncertain })]); MCP stop ok; native drain ok
- 2026-10-30T04:50:22.941267Z ERROR nessa_server::core::ending: nessa failed error=shutdown did not confirm all cleanup: stage reached: complete; webhooks ok; record reader ok; catalogue reader ok; review audit ok; conversations failed Retirement([("c9962288-659d-4f96-b45f-7d73adb55b41", OperationAndCleanupFailure { operation_error: Transport("provider stdout closed"), cleanup_error: CleanupUncertain })]); MCP stop ok; native drain ok exit_code=27 reason="shutdown" restart=Worthwhile recorded=false
