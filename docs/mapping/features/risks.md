# Bug and risk register

This is the investigation queue produced by tracing the user flows. No product
code was changed to fix these findings. Source-confirmed behavior and contract
inconsistencies are separated from hypotheses that still need reproduction.
Existing test links describe evidence to inspect or rerun; they do not establish
that an untested race occurred in a live application.

## Start here

| ID | Evidence status | User trigger and possible result | Owning flow and next useful check |
| --- | --- | --- | --- |
| R1 | Source-confirmed access mismatch; independently reviewed; native reproduction unrun | First-run setup tries to download an agent. Its own session cannot obtain the native endpoint or surface credential because both command implementations accept `main`, while setup's native label is `setup`. | [Startup](startup.md). Launch fresh native setup, inspect session failure and installation availability. Check both default endpoint discovery and an explicit URL override; the latter still reaches the credential guard. |
| R2 | Source-confirmed silent branch; independently reviewed; runtime reproduction unrun | Drop a second web-image URL while the first download is pending. `addImageUrl` returns on `busyRef` without a refusal or another fetch. | [Attachment drop routing](attachments.md#user-flow-drop-files-folders-text-or-a-web-image). Hold the first fetch pending; record notice state and fetch count after the second drop. |
| R3 | Source-confirmed API retry inconsistency; lost-answer impact is a hypothesis | Answer an agent's structured question and lose the acknowledgement. The client exposes retry through the general mutation path, while the backend consumes the ask without a same-action outcome recovery receipt. | [Runtime questions and permissions](runtime.md). Pause response delivery after audited selection; retry the retained action and compare the current view with the returned failure. Do not infer that retry repeats provider execution. |
| R4 | Hypothesis; independently checked against source | Choose an image, then press Enter while the native byte read is pending after host readiness ends. The hook marks `reading`, but the composer send guard reads `isPending`, which does not include this selected-file read. Existing text might send before the image joins it. | [File readiness](attachments.md#user-flow-wait-for-a-cloud-file-without-attaching-it-to-another-tab). Defer `readAttachmentBytes`, end host readying, press Enter and inspect the sent content before resolving the read. |
| R5 | Hypothesis | A readiness HTTP request never settles. The check stays pending and Check again remains disabled; repeated controller checks join the existing request. The fetch adapter has no frontend deadline, even though server-side probing is bounded. | [Startup risk table](startup.md). Supply a controlled never-settling fetch and verify whether gateway-state change, unmount or another action releases the check. |
| R6 | Hypothesis | Closing a tab based on a last-known empty/idle remote snapshot races another surface admitting work. Empty-tab cleanup invokes general `conversation.close`, which can stop the work admitted after that snapshot. | [Chat tab lifecycle](chat.md). Pause between empty-view classification and close, submit from a second surface, then release the close. Evaluate the intended authority for automatic cleanup before proposing a fix. |

R1's enforcement sites are
[endpoint loading](../../../src-tauri/src/gateway_endpoint/entrypoint/command.rs)
and [surface credential loading](../../../src-tauri/src/surface_credential.rs);
the requesting setup composition is [main.tsx](../../../src/main.tsx). The
existing `another_window_is_refused_before_the_gateway_is_asked_for_anything`
test asserts setup refusal, so an intended change must reconcile the command's
authority and the setup workflow rather than simply removing a guard.

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
