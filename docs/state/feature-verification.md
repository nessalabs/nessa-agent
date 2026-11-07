---
id: feature-verification
title: App feature ownership and model coverage
kind: guide
status: reference
summary: Every feature page has a declared owner, bounded model scope or explicit non-lifecycle boundary.
parent: nessa
sourceRevision: "3f0392b148ee998bf2df949bced96f5a61059769"
sources:
  - package.json
  - scripts/state-docs/coverage.mjs
  - src/desktop/main.tsx
  - src/main.tsx
  - crates/nessa-server/src/composition/mod.rs
diagramLinks: {}
---

# App feature ownership and model coverage

Start with the feature you want to understand, then open its owning page. A direct
model explores that owner's declared finite safety rules. Shared coverage links
to the owner of one part of a journey; the page states what remains outside that
model. A component index, pure contract or external library reference is accounted
for without calling it a formal pass.

The initial app source review is frozen at
`77a6b620af4d94b234294f6d273fd74b3dfb44c7`. GPT-6.1 Sol subagents traced
desktop/extensions, gateway/host, and runtime/core features; the integrating review
added missing backup, peer, environment, tunnel and agent-tool owners and checked
the combined catalog. Existing full-page checkpoints remain unchanged after partial
reviews. New model sections name their current source-review scope. The tables do
not stamp an entire page, service or application as implementation-verified.

Newer main was then integrated at
`719dfce93892e09dcb95e86a060b6d2f42c8f3cc`. The affected owners were
reconciled for context admission/validation, captured Stop recovery, busy-cache
discovery, native FIFO submissions and reader-scroll intent. Their pages name
that new checkpoint; unchanged pages retain their prior scope.

## Check the current map

Run `pnpm docs:state:coverage` with the renderer configured as described in
[verification commands](services/tooling/verification.md). It reads direct models
from the canonical catalog and shared links from each page's Model coverage section.
An unmapped stateful page fails, including a lifecycle parent that happens to have
child pages. Ordinary cross-links and mentions of external providers do not count
as mapped coverage. Run `pnpm docs:state:models` separately for the actual TLC
results. This page is a review snapshot, not a second navigation manifest or a
claim that every source statement, interleaving or operating system was tested.

## Coverage snapshot

The feature owners below currently link **102 executable models**.
TLC results shown are the last saved bounded run. Source guards, policy evaluators,
storage and process owners remain implementation authority; model exploration is
design evidence. See [provenance](provenance.md) for executed checks and exclusions.

| Disposition | Pages | Meaning |
| --- | --- | --- |
| Value or representation contract | 5 | Accounted for; this is not a TLC pass. |
| Direct model | 102 | A unique linked spec/config and declared safety rules. |
| External source boundary | 5 | Accounted for; this is not a TLC pass. |
| Component/feature index | 20 | Accounted for; this is not a TLC pass. |
| Reference guide | 8 | Accounted for; this is not a TLC pass. |
| Shared owner; partial scope | 33 | Open the owner link for the modeled part; inspect the feature for exclusions. |

## Auth features

| Feature or index | Disposition | Model owner and saved result |
| --- | --- | --- |
| [Identity and access](services/auth/README.md) | Component/feature index | Not a checked lifecycle |
| [Authentication admission and credential receipts](services/auth/auth-admission.md) | Direct model | 2 rules held; 11 states |
| [Credential lifecycle and access](services/auth/auth-credential.md) | Direct model | 4 rules held; 5 states |
| [Registry refusal evidence](services/auth/auth-registry-refusal.md) | Direct model | 4 rules held; 7 states |

## Client features

| Feature or index | Disposition | Model owner and saved result |
| --- | --- | --- |
| [Product client](services/client/README.md) | Component/feature index | Not a checked lifecycle |
| [Wire requests and command uncertainty](services/client/commands.md) | Direct model | 4 rules held; 31 states |
| [Managed connection](services/client/connection.md) | Direct model | 4 rules held; 15 states |
| [Event handler registration generations](services/client/event-registration.md) | Direct model | 4 rules held; 13 states |
| [Queue a native product submission](services/client/native-submissions.md) | Direct model | 6 rules held; 383 states |
| [Subscription opening and bounded consumption](services/client/subscriptions.md) | Direct model | 4 rules held; 17 states |

## Credentials features

| Feature or index | Disposition | Model owner and saved result |
| --- | --- | --- |
| [Agent credential values](services/credentials/README.md) | Component/feature index | Not a checked lifecycle |
| [Credential value contracts](services/credentials/credentials-values.md) | Value or representation contract | Not a checked lifecycle |

## Desktop features

| Feature or index | Disposition | Model owner and saved result |
| --- | --- | --- |
| [Desktop and panel](services/desktop/README.md) | Component/feature index | Not a checked lifecycle |
| [Chat](services/desktop/chat/README.md) | Component/feature index | Not a checked lifecycle |
| [close, switch, reopen, and restore tabs](services/desktop/chat/close-switch-reopen-and-restore-tabs.md) | Direct model | 3 rules held; 4 states |
| [compose and send the first message](services/desktop/chat/compose-and-send-the-first-message.md) | Direct model | 2 rules held; 313 states |
| [find, archive, or delete a conversation](services/desktop/chat/find-archive-or-delete-a-conversation.md) | Shared owner; partial scope | [load, browse, and resynchronize the workspace](services/desktop/workspace/load-browse-and-resynchronize-the-workspace.md); [Conversation identity and deletion](services/gateway/conversation.md) |
| [read a conversation and watch the response](services/desktop/chat/read-a-conversation-and-watch-the-response.md) | Shared owner; partial scope | [Managed connection](services/client/connection.md); [Keep the reader position when a transcript grows](services/desktop/workspace/transcript-scroll.md); [Committed history and restoration](services/sdk/history.md) |
| [recover a refused or unconfirmed send](services/desktop/chat/recover-a-refused-or-unconfirmed-send.md) | Direct model | 2 rules held; 9 states |
| [rename a tab and inspect conversation details](services/desktop/chat/rename-a-tab-and-inspect-conversation-details.md) | Direct model | 3 rules held; 55 states |
| [send and manage follow-ups](services/desktop/chat/send-and-manage-follow-ups.md) | Shared owner; partial scope | [recover a refused or unconfirmed send](services/desktop/chat/recover-a-refused-or-unconfirmed-send.md); [Submission scheduling and recovery](services/sdk/scheduling.md) |
| [stop active and queued work](services/desktop/chat/stop-active-and-queued-work.md) | Shared owner; partial scope | [Stop the captured parent turn and descendants](services/gateway/parent-stop.md); [Work admission and provider cleanup](services/sdk/cleanup.md); [Submission scheduling and recovery](services/sdk/scheduling.md) |
| [Desktop workspace](services/desktop/workspace/README.md) | Component/feature index | Not a checked lifecycle |
| [Configure native harness hooks](services/desktop/workspace/configure-native-hooks.md) | Shared owner; partial scope | [Finish or retire a settings action](services/desktop/workspace/settings-action-owners.md) |
| [Share a conversation and revoke its grant](services/desktop/workspace/conversation-sharing.md) | Direct model | 3 rules held; 26 states |
| [Create a channel](services/desktop/workspace/create-a-channel.md) | Direct model | 2 rules held; 41 states |
| [Inspect devices and add a computer](services/desktop/workspace/devices-dashboard.md) | Direct model | 2 rules held; 16 states |
| [discover subagents and experimental features](services/desktop/workspace/discover-subagents-and-experimental-features.md) | Value or representation contract | Not a checked lifecycle |
| [drag or move a session or pane](services/desktop/workspace/drag-or-move-a-session-or-pane.md) | Shared owner; partial scope | [open, split, resize, and close panes](services/desktop/workspace/open-split-resize-and-close-panes.md) |
| [Enable waiting-review notifications](services/desktop/workspace/enable-review-notices.md) | Shared owner; partial scope | [Finish or retire a settings action](services/desktop/workspace/settings-action-owners.md) |
| [fold, reveal, and resize side columns](services/desktop/workspace/fold-reveal-and-resize-side-columns.md) | Direct model | 4 rules held; 14 states |
| [follow focus and route keyboard actions](services/desktop/workspace/follow-focus-and-route-keyboard-actions.md) | Direct model | 3 rules held; 144 states |
| [Pair, approve and revoke a linked device](services/desktop/workspace/linked-devices.md) | Direct model | 2 rules held; 32 states |
| [load, browse, and resynchronize the workspace](services/desktop/workspace/load-browse-and-resynchronize-the-workspace.md) | Direct model | 2 rules held; 16 states |
| [Manage MCP integrations](services/desktop/workspace/manage-mcp-integrations.md) | Direct model | 4 rules held; 11 states |
| [MCP App view lifecycle](services/desktop/workspace/mcp-app-view-lifecycle.md) | Direct model | 3 rules held; 19 states |
| [open a widget inline, in the panel, or over the window](services/desktop/workspace/open-a-widget-inline-in-the-panel-or-over-the-window.md) | Shared owner; partial scope | [MCP App view lifecycle](services/desktop/workspace/mcp-app-view-lifecycle.md); [open, resize, expand and restore the right panel](services/desktop/workspace/open-resize-expand-and-restore-the-right-panel.md) |
| [open, dismiss, and reopen the desktop window](services/desktop/workspace/open-dismiss-and-reopen-the-desktop-window.md) | Direct model | 4 rules held; 74 states |
| [open, resize, expand and restore the right panel](services/desktop/workspace/open-resize-expand-and-restore-the-right-panel.md) | Direct model | 3 rules held; 18 states |
| [open, search, and change desktop settings](services/desktop/workspace/open-search-and-change-desktop-settings.md) | Shared owner; partial scope | [Pair, approve and revoke a linked device](services/desktop/workspace/linked-devices.md); [Manage MCP integrations](services/desktop/workspace/manage-mcp-integrations.md); [Revoke a remembered permission rule](services/desktop/workspace/permission-rule-revocation.md); [Finish or retire a settings action](services/desktop/workspace/settings-action-owners.md); [Keep a local window preference](services/desktop/workspace/window-preferences.md); [Channel workspaces](services/gateway/channel-workspaces.md); [GitHub reads](services/gateway/github-reads.md); [save provider credentials and download a runtime](services/host/startup/save-provider-credentials-and-download-a-runtime.md); [Local vault](services/gateway/local-vault.md) |
| [open, split, resize, and close panes](services/desktop/workspace/open-split-resize-and-close-panes.md) | Direct model | 3 rules held; 18 states |
| [Revoke a remembered permission rule](services/desktop/workspace/permission-rule-revocation.md) | Direct model | 3 rules held; 37 states |
| [personalize the home header and composer](services/desktop/workspace/personalize-the-home-header-and-composer.md) | Direct model | 3 rules held; 78 states |
| [pin, archive, or close a workspace session view](services/desktop/workspace/pin-archive-or-close-a-workspace-session-view.md) | Shared owner; partial scope | [load, browse, and resynchronize the workspace](services/desktop/workspace/load-browse-and-resynchronize-the-workspace.md); [open, split, resize, and close panes](services/desktop/workspace/open-split-resize-and-close-panes.md) |
| [Recover provider sign-in](services/desktop/workspace/provider-sign-in.md) | Direct model | 1 rules held; 10 states |
| [Join an answer already being sent](services/desktop/workspace/question-answer-flights.md) | Direct model | 2 rules held; 7 states |
| [resize and refit the panel across displays](services/desktop/workspace/resize-and-refit-the-panel-across-displays.md) | Direct model | 3 rules held; 45 states |
| [review agents, answer requests, and reply from the overview](services/desktop/workspace/review-agents-answer-requests-and-reply-from-the-overview.md) | Shared owner; partial scope | [Join an answer already being sent](services/desktop/workspace/question-answer-flights.md); [Permission choice and response delivery](services/sdk/reviews.md) |
| [search sessions and jump with the quick switcher](services/desktop/workspace/search-sessions-and-jump-with-the-quick-switcher.md) | Shared owner; partial scope | [follow focus and route keyboard actions](services/desktop/workspace/follow-focus-and-route-keyboard-actions.md) |
| [Finish or retire a settings action](services/desktop/workspace/settings-action-owners.md) | Direct model | 2 rules held; 46 states |
| [start a workspace session, send, and retry](services/desktop/workspace/start-a-workspace-session-send-and-retry.md) | Shared owner; partial scope | [recover a refused or unconfirmed send](services/desktop/chat/recover-a-refused-or-unconfirmed-send.md); [Submission scheduling and recovery](services/sdk/scheduling.md) |
| [step back from a widget and return to its conversation](services/desktop/workspace/step-back-from-a-widget-and-return-to-its-conversation.md) | Shared owner; partial scope | [open, resize, expand and restore the right panel](services/desktop/workspace/open-resize-expand-and-restore-the-right-panel.md) |
| [summon or dismiss the floating panel](services/desktop/workspace/summon-or-dismiss-the-floating-panel.md) | Direct model | 2 rules held; 7 states |
| [switch the floating panel's surface](services/desktop/workspace/switch-the-floating-panel-s-surface.md) | Direct model | 3 rules held; 40 states |
| [trace verified pull-request contributions](services/desktop/workspace/trace-verified-pull-request-contributions.md) | Value or representation contract | Not a checked lifecycle |
| [Keep the reader position when a transcript grows](services/desktop/workspace/transcript-scroll.md) | Direct model | 3 rules held; 181 states |
| [Talk to a conversation by voice](services/desktop/workspace/voice-call.md) | Direct model | 3 rules held; 21 states |
| [Keep a local window preference](services/desktop/workspace/window-preferences.md) | Direct model | 2 rules held; 57 states |

## Endpoint features

| Feature or index | Disposition | Model owner and saved result |
| --- | --- | --- |
| [Gateway endpoint discovery](services/endpoint/README.md) | Component/feature index | Not a checked lifecycle |
| [Publication and discovery](services/endpoint/endpoint-discovery.md) | Direct model | 2 rules held; 10 states |
| [Endpoint advertisement publication](services/endpoint/endpoint-publication.md) | Shared owner; partial scope | [Private publication and acknowledgement](services/storage/storage-publication.md) |

## Extensions features

| Feature or index | Disposition | Model owner and saved result |
| --- | --- | --- |
| [Extensions and UI integration](services/extensions/README.md) | Component/feature index | Not a checked lifecycle |
| [Extensions and UI](services/extensions/ui/README.md) | Component/feature index | Not a checked lifecycle |
| [call an app tool, approve it, or fetch its resource through the gateway](services/extensions/ui/call-an-app-tool-approve-it-or-fetch-its-resource-through-the-gateway.md) | Shared owner; partial scope | [One-use app resource tickets](services/mcp/resource-tickets.md); [Conversation grants and upstream sessions](services/mcp/sessions.md) |
| [consume UI packages or copied registry source and theme a surface](services/extensions/ui/consume-ui-packages-or-copied-registry-source-and-theme-a-surface.md) | External source boundary | Sibling/package implementation not covered by this app-source checkpoint |
| [develop an MCP App in the fake host and reference host](services/extensions/ui/develop-an-mcp-app-in-the-fake-host-and-reference-host.md) | External source boundary | Sibling/package implementation not covered by this app-source checkpoint |
| [discover reusable UI families for an application](services/extensions/ui/discover-reusable-ui-families-for-an-application.md) | Value or representation contract | Not a checked lifecycle |
| [expose an extension tool with a view and text fallback](services/extensions/ui/expose-an-extension-tool-with-a-view-and-text-fallback.md) | External source boundary | Sibling/package implementation not covered by this app-source checkpoint |
| [inspect experiment results through a validated definition](services/extensions/ui/inspect-experiment-results-through-a-validated-definition.md) | External source boundary | Sibling/package implementation not covered by this app-source checkpoint |
| [open a tool's MCP App and interact with its host](services/extensions/ui/open-a-tool-s-mcp-app-and-interact-with-its-host.md) | Shared owner; partial scope | [MCP App view lifecycle](services/desktop/workspace/mcp-app-view-lifecycle.md) |
| [replay or render an agent stream in the workshop or a consumer app](services/extensions/ui/replay-or-render-an-agent-stream-in-the-workshop-or-a-consumer-app.md) | External source boundary | Sibling/package implementation not covered by this app-source checkpoint |
| [see an MCP tool's result and discover its view](services/extensions/ui/see-an-mcp-tool-s-result-and-discover-its-view.md) | Shared owner; partial scope | [MCP App view lifecycle](services/desktop/workspace/mcp-app-view-lifecycle.md) |

## Gateway features

| Feature or index | Disposition | Model owner and saved result |
| --- | --- | --- |
| [Local gateway](services/gateway/README.md) | Component/feature index | Not a checked lifecycle |
| [Delegate to another agent thread](services/gateway/agent-thread-tools.md) | Direct model | 5 rules held; 50 states |
| [Attachments and content](services/gateway/attachments/README.md) | Component/feature index | Not a checked lifecycle |
| [Attach files with the + picker or clipboard](services/gateway/attachments/attach-files-with-the-picker-or-clipboard.md) | Direct model | 2 rules held; 12 states |
| [Drop files, folders, text, or a web image](services/gateway/attachments/drop-files-folders-text-or-a-web-image.md) | Shared owner; partial scope | [Wait for a cloud file without attaching it to another tab](services/gateway/attachments/wait-for-a-cloud-file-without-attaching-it-to-another-tab.md) |
| [Have a large or unfamiliar image normalized and kept once](services/gateway/attachments/have-a-large-or-unfamiliar-image-normalized-and-kept-once.md) | Direct model | 3 rules held; 9 states |
| [Open a link without replacing the app](services/gateway/attachments/open-a-link-without-replacing-the-app.md) | Direct model | 5 rules held; 168 states |
| [Paste text or code and inspect the full pasted payload](services/gateway/attachments/paste-text-or-code-and-inspect-the-full-pasted-payload.md) | Direct model | 2 rules held; 12 states |
| [Preview or remove a draft attachment](services/gateway/attachments/preview-or-remove-a-draft-attachment.md) | Direct model | 2 rules held; 5 states |
| [Read rich text, code, math, and diagrams in a message](services/gateway/attachments/read-rich-text-code-math-and-diagrams-in-a-message.md) | Direct model | 6 rules held; 376 states |
| [Reopen a message and understand its reference-only attachments](services/gateway/attachments/reopen-a-message-and-understand-its-reference-only-attachments.md) | Shared owner; partial scope | [Preview or remove a draft attachment](services/gateway/attachments/preview-or-remove-a-draft-attachment.md) |
| [Send a non-image file by path and approve its read](services/gateway/attachments/send-a-non-image-file-by-path-and-approve-its-read.md) | Direct model | 2 rules held; 4 states |
| [Send an uploaded file from the phone](services/gateway/attachments/send-an-uploaded-file-from-the-phone.md) | Direct model | 3 rules held; 12 states |
| [Send uploaded images to the agent](services/gateway/attachments/send-uploaded-images-to-the-agent.md) | Shared owner; partial scope | [Have a large or unfamiliar image normalized and kept once](services/gateway/attachments/have-a-large-or-unfamiliar-image-normalized-and-kept-once.md) |
| [Close or delete a conversation and release its uploaded images](services/gateway/attachments/stop-or-delete-a-conversation-and-release-its-uploaded-images.md) | Shared owner; partial scope | [Conversation identity and deletion](services/gateway/conversation.md) |
| [Understand an attachment refusal and choose a useful recovery](services/gateway/attachments/understand-an-attachment-refusal-and-choose-a-useful-recovery.md) | Direct model | 3 rules held; 52 states |
| [Upload an image and retry a failed upload](services/gateway/attachments/upload-an-image-and-retry-a-failed-upload.md) | Direct model | 2 rules held; 22 states |
| [Wait for a cloud file without attaching it to another tab](services/gateway/attachments/wait-for-a-cloud-file-without-attaching-it-to-another-tab.md) | Direct model | 2 rules held; 22 states |
| [Background tasks](services/gateway/background-tasks.md) | Direct model | 3 rules held; 40 states |
| [Export a consistent backup cut](services/gateway/backup-export.md) | Direct model | 3 rules held; 10 states |
| [Verify, quarantine and accept a backup](services/gateway/backup-restore.md) | Direct model | 3 rules held; 244 states |
| [Channel workspaces](services/gateway/channel-workspaces.md) | Direct model | 3 rules held; 6 states |
| [Conversation identity and deletion](services/gateway/conversation.md) | Direct model | 3 rules held; 22 states |
| [Approve and activate a linked device](services/gateway/device-pairing.md) | Direct model | 4 rules held; 82 states |
| [Admit a Drive turn and its tool calls](services/gateway/drive-tool-admission.md) | Direct model | 3 rules held; 80 states |
| [Run harnesses through environment leases](services/gateway/environment-leases.md) | Direct model | 5 rules held; 72 states |
| [Run a bounded environment command for a turn](services/gateway/environment-tools.md) | Direct model | 3 rules held; 224 states |
| [External event admission](services/gateway/external-events.md) | Direct model | 2 rules held; 12 states |
| [GitHub reads](services/gateway/github-reads.md) | Direct model | 2 rules held; 15 states |
| [Local vault](services/gateway/local-vault.md) | Direct model | 3 rules held; 28 states |
| [Stop the captured parent turn and descendants](services/gateway/parent-stop.md) | Direct model | 4 rules held; 132 states |
| [Enroll, read and forget peer gateways](services/gateway/peer-gateways.md) | Direct model | 6 rules held; 11 states |
| [Wait for busy record discovery within one read](services/gateway/record-read-discovery.md) | Direct model | 4 rules held; 43 states |
| [Bounded physical history reads](services/gateway/record-reads.md) | Direct model | 2 rules held; 8 states |
| [Reach a gateway through a tunnel](services/gateway/tunnel-connectivity.md) | Direct model | 4 rules held; 12 states |
| [Outbound webhook admission](services/gateway/webhook-admission.md) | Direct model | 3 rules held; 12 states |
| [Outbound webhook foundation](services/gateway/webhook-foundation.md) | Direct model | 3 rules held; 78 states |
| [Webhook credential issuance](services/gateway/webhook-issuance.md) | Direct model | 3 rules held; 7 states |
| [Durable outbound webhook state](services/gateway/webhook-outbox.md) | Direct model | 3 rules held; 14 states |

## Github-webhooks features

| Feature or index | Disposition | Model owner and saved result |
| --- | --- | --- |
| [GitHub webhook relay](services/github-webhooks/README.md) | Direct model | 2 rules held; 12 states |
| [Select private live event repositories](services/github-webhooks/live-selection.md) | Direct model | 2 rules held; 6 states |

## Host features

| Feature or index | Disposition | Model owner and saved result |
| --- | --- | --- |
| [Native host](services/host/README.md) | Component/feature index | Not a checked lifecycle |
| [Startup and access](services/host/startup/README.md) | Component/feature index | Not a checked lifecycle |
| [authenticate, reconnect, expire, or revoke a surface session](services/host/startup/authenticate-reconnect-expire-or-revoke-a-surface-session.md) | Shared owner; partial scope | [Surface session supervision](services/host/startup/session-supervision.md) |
| [bootstrap and diagnose access through the CLI](services/host/startup/bootstrap-and-diagnose-access-through-the-cli.md) | Direct model | 2 rules held; 22 states |
| [complete setup, choose an agent, and learn the shortcut](services/host/startup/complete-setup-choose-an-agent-and-learn-the-shortcut.md) | Direct model | 2 rules held; 9 states |
| [launch and choose the first surface](services/host/startup/launch-and-choose-the-first-surface.md) | Direct model | 2 rules held; 9 states |
| [recover or update the managed gateway](services/host/startup/recover-or-update-the-managed-gateway.md) | Direct model | 3 rules held; 6 states |
| [Download a tested agent runtime](services/host/startup/runtime-download.md) | Direct model | 2 rules held; 24 states |
| [save provider credentials and download a runtime](services/host/startup/save-provider-credentials-and-download-a-runtime.md) | Direct model | 2 rules held; 13 states |
| [Surface session supervision](services/host/startup/session-supervision.md) | Direct model | 2 rules held; 6 states |
| [sign in, restore, renew, and sign out in the browser](services/host/startup/sign-in-restore-renew-and-sign-out-in-the-browser.md) | Direct model | 2 rules held; 5 states |
| [Observe gateway startup and retry](services/host/startup/startup-observation.md) | Direct model | 2 rules held; 11 states |

## Images features

| Feature or index | Disposition | Model owner and saved result |
| --- | --- | --- |
| [Image normalization](services/images/README.md) | Component/feature index | Not a checked lifecycle |
| [Normalization pipeline](services/images/images-normalization.md) | Value or representation contract | Not a checked lifecycle |

## Mcp features

| Feature or index | Disposition | Model owner and saved result |
| --- | --- | --- |
| [MCP sessions and tools](services/mcp/README.md) | Component/feature index | Not a checked lifecycle |
| [One-use app resource tickets](services/mcp/resource-tickets.md) | Direct model | 5 rules held; 8 states |
| [Conversation grants and upstream sessions](services/mcp/sessions.md) | Direct model | 3 rules held; 12 states |

## Sdk features

| Feature or index | Disposition | Model owner and saved result |
| --- | --- | --- |
| [Agent SDK](services/sdk/README.md) | Component/feature index | Not a checked lifecycle |
| [Provider attachment](services/sdk/attachment.md) | Direct model | 4 rules held; 11 states |
| [Child preparation and initial task](services/sdk/child-preparation.md) | Direct model | 4 rules held; 18 states |
| [Work admission and provider cleanup](services/sdk/cleanup.md) | Direct model | 5 rules held; 18 states |
| [Native context admission and opening](services/sdk/context-admission.md) | Direct model | 6 rules held; 136 states |
| [Validate retained context admission evidence](services/sdk/context-validation.md) | Direct model | 5 rules held; 874 states |
| [History erasure and incarnation recovery](services/sdk/history-erasure.md) | Direct model | 4 rules held; 7 states |
| [History preparation and restoration](services/sdk/history-restoration.md) | Direct model | 3 rules held; 14 states |
| [Committed history and restoration](services/sdk/history.md) | Direct model | 4 rules held; 15 states |
| [Parent and child agent ownership](services/sdk/ownership.md) | Direct model | 4 rules held; 27 states |
| [Remembered permission rules and review notices](services/sdk/permission-rules.md) | Direct model | 5 rules held; 11 states |
| [Permission choice and response delivery](services/sdk/reviews.md) | Direct model | 4 rules held; 10 states |
| [Runtime and tools](services/sdk/runtime/README.md) | Component/feature index | Not a checked lifecycle |
| [a conversation opens its selected provider](services/sdk/runtime/a-conversation-opens-its-selected-provider.md) | Shared owner; partial scope | [Provider attachment](services/sdk/attachment.md) |
| [an admitted message streams through the SDK and settles](services/sdk/runtime/an-admitted-message-streams-through-the-sdk-and-settles.md) | Shared owner; partial scope | [Submission scheduling and recovery](services/sdk/scheduling.md) |
| [an app calls a tool through gateway policy and review](services/sdk/runtime/an-app-calls-a-tool-through-gateway-policy-and-review.md) | Shared owner; partial scope | [Permission choice and response delivery](services/sdk/reviews.md) |
| [an app sends a message and gives the model context](services/sdk/runtime/an-app-sends-a-message-and-gives-the-model-context.md) | Shared owner; partial scope | [Submission scheduling and recovery](services/sdk/scheduling.md) |
| [an authorized receiver reads physical history without opening an agent](services/sdk/runtime/an-authorized-receiver-reads-physical-history-without-opening-an-agent.md) | Shared owner; partial scope | [Committed history and restoration](services/sdk/history.md); [History preparation and restoration](services/sdk/history-restoration.md) |
| [an MCP tool shares its upstream session with its app](services/sdk/runtime/an-mcp-tool-shares-its-upstream-session-with-its-app.md) | Shared owner; partial scope | [Conversation grants and upstream sessions](services/mcp/sessions.md) |
| [approve, deny, withdraw, or answer an agent's review](services/sdk/runtime/approve-deny-withdraw-or-answer-an-agent-s-review.md) | Shared owner; partial scope | [Permission choice and response delivery](services/sdk/reviews.md) |
| [install or replace a pinned agent runtime](services/sdk/runtime/install-or-replace-a-pinned-agent-runtime.md) | Direct model | 4 rules held; 12 states |
| [read an app resource once and release its ticket](services/sdk/runtime/read-an-app-resource-once-and-release-its-ticket.md) | Shared owner; partial scope | [One-use app resource tickets](services/mcp/resource-tickets.md) |
| [restart and restore committed conversation history](services/sdk/runtime/restart-and-restore-committed-conversation-history.md) | Shared owner; partial scope | [Committed history and restoration](services/sdk/history.md); [History preparation and restoration](services/sdk/history-restoration.md) |
| [select a model, approval preset, or reasoning effort](services/sdk/runtime/select-a-model-approval-preset-or-reasoning-effort.md) | Direct model | 4 rules held; 11 states |
| [Turn Stop and attachment cleanup have different owners](services/sdk/runtime/stop-cancels-owned-work-and-confirms-process-cleanup.md) | Shared owner; partial scope | [Work admission and provider cleanup](services/sdk/cleanup.md) |
| [the first agent launch is warmed in the background](services/sdk/runtime/the-first-agent-launch-is-warmed-in-the-background.md) | Direct model | 3 rules held; 9 states |
| [Usage limits and scheduled continuation](services/sdk/runtime/usage-limit-continuation.md) | Direct model | 5 rules held; 9 states |
| [Submission scheduling and recovery](services/sdk/scheduling.md) | Direct model | 7 rules held; 81 states |
| [Finish a captured Stop after restart](services/sdk/stop-recovery.md) | Direct model | 5 rules held; 42 states |

## Storage features

| Feature or index | Disposition | Model owner and saved result |
| --- | --- | --- |
| [Private local persistence](services/storage/README.md) | Component/feature index | Not a checked lifecycle |
| [Database opening and version refusal](services/storage/storage-database.md) | Direct model | 5 rules held; 9 states |
| [Private publication and acknowledgement](services/storage/storage-publication.md) | Direct model | 4 rules held; 9 states |

## Tooling features

| Feature or index | Disposition | Model owner and saved result |
| --- | --- | --- |
| [Build and release tooling](services/tooling/README.md) | Component/feature index | Not a checked lifecycle |
| [Runtime and release assembly](services/tooling/tooling-runtime.md) | Direct model | 4 rules held; 9 states |
| [Verification commands and model evidence](services/tooling/verification.md) | Reference guide | Not a checked lifecycle |

## Scope beyond the model

The pure credential values and stateless normalization contract have no invented
retained lifecycle. External UI/workshop references keep their source boundary.
OS openers, actual provider availability, native device behavior, cryptography,
billable cloud resources and browser measurements need their own execution evidence.
Models can intentionally permit uncertain cleanup, lost replies or older stored
values when the source does; a successful check does not turn those outcomes into
successful external effects. No universal fairness or eventual-completion claim is made.

## Subsequent placement and access source review

[Refresh 1268](feature-refresh-1268.md) maps newer pool order, channel movement and remembered-app revocation at source `31d023ed`. These source-only guides do not add model coverage or change this earlier verification receipt.
