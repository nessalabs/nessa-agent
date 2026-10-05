---
id: "risks"
title: "Risks and verification evidence"
kind: "guide"
status: "mixed"
summary: "Risks and verification evidence"
parent: "nessa"
sources:
  - "src/panel/ui/use-file-attachments-races.test.ts"
  - "packages/nessa-client/src/presentation/conversation-api.test.ts"
  - "src/onboarding/adapters/agents.test.ts"
  - "src/onboarding/ui/onboarding-readiness-timeout.test.tsx"
  - "crates/nessa-server/tests/conversation/projection.rs"
  - "src/conversation/adapters/store/attachments.test.ts"
  - "src-tauri/src/gateway_endpoint/entrypoint/command.rs"
  - "src-tauri/src/surface_credential.rs"
  - "src/main.tsx"
  - "src/panel/ui/use-file-attachments.ts"
  - "packages/nessa-client/src/presentation/conversation-api.ts"
diagramLinks: {}
---

# Risks and verification evidence

This page keeps earlier findings and their evidence. R1–R5 were corrected in the work tracked by [issue #386](https://github.com/nessalabs/nessa-agent/issues/386) and [PR #387](https://github.com/nessalabs/nessa-agent/pull/387). R6 was not reproduced for the built-in stores. This rewrite did not rerun those runtime tests.

## Historical regressions

| ID | What happened | Evidence and limits |
| --- | --- | --- |
| R1 | Native setup was rejected by guards that admitted only the main window. That fix admitted main and setup while retaining stage and destination checks. Later, #419 added the desktop window as a ready-only gateway reader; it cannot reconcile the gateway. | Native command regressions failed before the fix and passed afterward; nine setup tests passed. Unrelated windows remain refused. See [startup](services/host/startup/README.md). |
| R2 | A second image-URL drop vanished while the first read was pending. It now reports the reading-files refusal and preserves the first read. | [Actual-App regressions](../../src/panel/ui/use-file-attachments-races.test.ts) and Chromium/WebKit checks used controlled host and gateway effects. The pre-fix hook failed the intended cases. |
| R3 | A lost reply to a structured-question answer offered unsafe replay after the answer was consumed. It now uses a non-retryable control path. | [Client regression](../../packages/nessa-client/src/presentation/conversation-api.test.ts) failed before and passed after. No duplicated provider execution was established; SDK consumption was a separate control. |
| R4 | Enter sent text while a native image read was pending. The captured conversation now remains pending until the read settles. | The same [App regressions](../../src/panel/ui/use-file-attachments-races.test.ts) and both browser engines checked refusal before completion and a single text-plus-image send afterward. Native and gateway effects were controlled. |
| R5 | A stalled readiness fetch or JSON body left Check again disabled. One ten-second deadline now covers both stages and releases Retry. | [Adapter](../../src/onboarding/adapters/agents.test.ts), [onboarding](../../src/onboarding/ui/onboarding-readiness-timeout.test.tsx) and browser checks covered delayed responses. This was a controlled stall, not a production outage. |
| R6 | A stale empty tab was suspected of stopping another surface's work. Automatic close requires exact complete_empty, while both built-in stores report prepared empty sessions as complete. Closing therefore detaches. | [Projection](../../crates/nessa-server/tests/conversation/projection.rs) and [tab controls](../../src/conversation/adapters/store/attachments.test.ts) falsified the suspected path for both built-in stores. Custom producers of complete_empty remain a separate question. No product fix was made. |

## Useful failure checks

These are investigation prompts, not confirmed defects:

- Switch tabs during a delayed read. Check that a late view cannot replace another conversation's content.
- Lose a send reply, then reconnect. Check that recovery keeps the original request and does not replay an uncertain effect.
- Cross queue removal with dispatch. A refreshed queue is not proof that agent work was reversed.
- Fail answer delivery or audit after a choice. Keep consumed choice, response delivery and provider action distinct.
- Cross Stop with completion. Check process cleanup separately from cancellation and saved results.
- Time out a saved-record read while source IO continues. Capacity must remain held until the retained read settles.
- Refuse a native show or hide request. The reported state must say what was actually confirmed.

## Current capability boundaries

| Area | Current meaning |
| --- | --- |
| Desktop workspace | Native desktop uses the gateway. Ordinary browser previews use sample data; those replies are not proof of real agent execution. |
| MCP App display | Gateway composition connects server apps; sample composition keeps fixture wiring. Missing sandbox capability remains explicit. See [app lifecycle](services/desktop/workspace/mcp-app-view-lifecycle.md). |
| Experiments | Domain and samples exist; server and app product slices remain planned. |
| Draft recovery | Browser tab IDs and titles can persist. Drafts and uncertain-send intent remain in current frontend memory. |
| Conversation views | Limited replacement views, not exact event replay. SDK live text since the last successful save can be lost on crash; panel reads expose committed text. |
| Provider-key storage | Native secure-store save is available on macOS and unavailable in the other adapter. |
| Rich content | Panel Markdown and desktop rich text have different rendering capabilities. |

## Verification boundaries

The original mapping inspected source and existing tests. The later controlled regressions above have their own execution evidence. Neither establishes every real provider, operating-system or external-package behavior.

This documentation pass checks metadata, links and diagram rendering. Read [gaps and next work](gaps.md) for current follow-ups.
