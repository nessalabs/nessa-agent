# Bug and risk register

This register started as the investigation queue from the user-flow map in
[PR #385](https://github.com/nessalabs/nessa-agent/pull/385). Follow-up work in
[issue #386](https://github.com/nessalabs/nessa-agent/issues/386) and
[PR #387](https://github.com/nessalabs/nessa-agent/pull/387) adds controlled
reproductions, regression fixes and negative controls. Test evidence below
distinguishes real application paths with substituted outside effects from
production/native execution; remaining boundaries are investigation prompts.

## Start here

| ID | Evidence status | User trigger and possible result | Owning flow and next useful check |
| --- | --- | --- | --- |
| R1 | Three native command regressions red → green; nine setup tests pass | Setup previously failed the native `main`-only endpoint and credential guards. The fix reuses the host-owned bundled-surface admission rule for exactly `main` and `setup`, retaining stage and destination verification. | [Startup](startup.md). Native command tests must verify trusted Setup attribution and unrelated-window refusal before effects. |
| R2 | Actual App and Chromium/WebKit regressions red → green | A second web-image URL during a pending download previously vanished silently. The fix reports the existing reading-files refusal. | [Attachment drop routing](attachments.md#user-flow-drop-files-folders-text-or-a-web-image), [App regressions](../../../src/panel/ui/use-file-attachments-races.test.ts). Controlled host and scenario gateway effects; six browser cases pass, and the pre-fix hook fails four intended cases. |
| R3 | Lost-acknowledgement client regression red → green | Structured-question answers previously exposed a replay callback despite one-shot ask consumption. They now use the existing non-retryable control-action path; the caller can reconcile current state before a deliberate new answer. No duplicated provider execution was established. | [Runtime questions and permissions](runtime.md), [client regressions](../../../packages/nessa-client/src/presentation/conversation-api.test.ts). Real SDK consumption control is separate from the substituted client transport. |
| R4 | Actual App and Chromium/WebKit regressions red → green | Enter previously sent the text draft while native picked-image bytes were pending. That read now marks its captured conversation pending and displays pending attachments until admission finishes. | [File readiness](attachments.md#user-flow-wait-for-a-cloud-file-without-attaching-it-to-another-tab), [App regressions](../../../src/panel/ui/use-file-attachments-races.test.ts). Controlled native read and scenario gateway effects; Chromium/WebKit confirm blocked send before completion and one text-plus-image send afterward. |
| R5 | Controlled fetch/body and actual onboarding regressions red → green | A fetch or JSON body that never settled left Check again disabled indefinitely. One 10-second deadline now bounds both stages, aborts transport and releases retry; existing generation checks prevent stale overwrite. | [Startup risk table](startup.md), [adapter regressions](../../../src/onboarding/adapters/agents.test.ts), [onboarding regression](../../../src/onboarding/ui/onboarding-readiness-timeout.test.tsx). Controlled effects/fake time plus four Chromium/WebKit cases against stalled real HTTP responses (10.02–10.10 seconds); not a production outage. |
| R6 | Falsified for both built-in stores; contingent for custom state producers | The suspected stale-empty cleanup requires exact `complete_empty`. Real InMemoryStorage and RecordStorage instead project a prepared empty conversation as `complete`, so closing its tab detaches without issuing automatic close, including after external work admission. | [Chat tab lifecycle](chat.md), [gateway projection regression](../../../crates/nessa-server/tests/conversation/projection.rs), [frontend negative control](../../../src/conversation/adapters/store/attachments.test.ts). No product fix; custom producers of `complete_empty` remain a separate question. |

R1's enforcement sites are
[endpoint loading](../../../src-tauri/src/gateway_endpoint/entrypoint/command.rs)
and [surface credential loading](../../../src-tauri/src/surface_credential.rs);
the requesting setup composition is [main.tsx](../../../src/main.tsx). The
updated `another_window_is_refused_before_the_gateway_is_asked_for_anything`
test uses an unrelated window; positive setup tests reconcile trusted admission
with the existing readiness, stage and endpoint checks.

R2 and R4 originate in
[use-file-attachments.ts](../../../src/panel/ui/use-file-attachments.ts). R3's
client boundary is
[conversation-api.ts](../../../packages/nessa-client/src/presentation/conversation-api.ts).
The detailed feature maps carry the corresponding backend, test and provenance
links, plus the conditions needed to falsify each hypothesis.

## Other boundaries worth exercising

| Boundary | Probe | Evidence to follow |
| --- | --- | --- |
| Snapshot refresh → current tab state | Switch tabs during a delayed read; complete an older read after a newer failure or view. Check conversation/request identity and whether output is replaced rather than duplicated. | [Chat streaming/recovery](chat.md) |
| Queue view → SDK admission | Race Remove with dispatch, reorder with another surface, and acknowledgement loss with reconnect. Distinguish a refreshed queue from proof that provider work was reversed. | [Chat follow-ups](chat.md), [SDK scheduling](runtime.md) |
| Permission selection → provider effect → audit | Lose the caller after selection; fail audit or provider delivery; then refresh and close. Keep selected, written, failed and physically released facts separate. | [Runtime permission flows](runtime.md) |
| Stop → attachment/provider cleanup | Cross cancellation with acknowledgement and uncertain process cleanup. Successful local close does not prove external effects were rolled back. | [Runtime shutdown](runtime.md), [upload cleanup](attachments.md) |
| Runtime replacement → retained executable use | Inspect a superseded artifact that remains on disk. Establish whether it has outstanding ownership, unsettled delivery or a failed reclamation audit before treating it as a leak. | [Runtime installation](runtime.md) |
| Record-read deadline → retained capacity | Time out a receiver while source IO still runs. Verify bounded ownership survives until source completion; freeing capacity at response timeout alone can over-admit work. | [Runtime physical reads](runtime.md) |
| Native shortcut/window request → OS effect | Unregister failure, show/hide/focus refusal, tiny work area, or unavailable tray. Managed state and a returned boolean do not prove the OS performed the request. | [Desktop/panel](desktop.md) |
| Consumer stream → renderer | Use the wrong transport mapper, reuse an event identity incorrectly, or assume all capabilities exist in a recorded fixture. | [Stream workshop and UI](extensions-ui.md) |

These are concrete probes attached to current boundaries, not claims that each
boundary contains a defect. Existing lifecycle and adversarial tests may already
cover particular rows; inspect the named tests before adding duplicate work.

## Capability gaps that should stay visible

| Current boundary | What a person can observe | Classification |
| --- | --- | --- |
| Desktop workspace default composition uses `inMemorySource` and scripted replies | Desktop interactions work as a prototype; they do not prove a real agent answered. | Fixture boundary — [desktop](desktop.md) |
| `AppWidgetPlugin` renders an unavailable fallback | A tool's MCP App cannot yet render through Nessa's planned sandbox/bridge, although MCP connections and native widget hosts exist. | Placeholder — [extensions and UI](extensions-ui.md) |
| Experiments checkout contains domain/samples; server/app slices are planned | Passing its tests/build command does not establish a runnable experiments MCP server or app. | Unbuilt product slices — [extensions and UI](extensions-ui.md) |
| Drafts and unresolved actions live in current frontend memory | Reload can lose draft/retry intent. Browser composition saves tab references/names; native panel does not wire that tab persistence. | Current persistence limit — [chat](chat.md) |
| Gateway reads are bounded current replacement views | Polling does not provide exact historical event replay. Physical reads are a separate authenticated API. SDK-observed text since the last successful commit can be lost on crash; panel reads expose committed text. | Current transport/durability limit — [chat](chat.md), [runtime](runtime.md) |
| Linux native provider-key storage is unavailable | The native key-entry path cannot provide the macOS keychain behavior. | Platform capability limit — [startup](startup.md) |
| Rich rendering differs between full panel Markdown and desktop `RichText` | The same authored content can have different rendering capabilities on the two surfaces. | Current surface difference — [attachments](attachments.md) |

Historical bugs repaired by the referenced PRs belong in the feature provenance,
not this open queue. None of the rows above is a claim that those repairs failed.

## Verification performed for this mapping

Six agents traced separate scopes, followed by independent reviews across
startup/chat/runtime and desktop/extensions/attachments. Reviews corrected retry
ownership, final post-hook persistence, empty-tab cleanup semantics, and native
widget versus MCP App terminology. All maps identify source and existing tests;
native/provider interleavings were not executed as part of the documentation work.

The coordinator checks relative links, anchors, Mermaid parsing and the final
documentation diff. Environment setup validation is reported separately from
the source inspection: passing setup tests does not reproduce the risk probes.
