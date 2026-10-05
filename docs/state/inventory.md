---
id: "inventory"
title: "Repository state and lifecycle inventory"
kind: "guide"
status: "reference"
summary: "Repository state and lifecycle inventory"
parent: "nessa"
sources:
  - "Cargo.toml"
  - "docs/codebase-structure.md"
  - "docs/ARCHITECTURE.md"
diagramLinks: {}
---

# Repository state and lifecycle inventory

## Inspection scope

The sweep starts from local revision `1bc93353ffebaf7553c14c624c269fbd21b71583`
plus the earlier chat-diagram edits in this checkout. It covers source ownership
in the nine workspace crates present at that revision, the Tauri host, frontend verticals, product client,
protocol publications, build scripts, existing design records and regression
files. This is a static source inspection and documentation migration, not an
exhaustive run of every branch or external provider.

The migration extracts **70 detailed flow pages** from the six feature maps,
retaining source provenance and explaining the behavior in shorter language. The
launch/send sequence is on the system entry page. New source-backed charts
cover owners that were previously buried in implementation or design tables.

## Current-main reconciliation

Before opening the documentation PR, the atlas was reconciled with main revision `11b622d8e5c07f6a40cdec4fd172e3f26e3eab4b`. This bounded pass retains later flow-map updates for desktop connection modes, gateway app integration, native reader authority, paired protected reads and forwarded tool results. Source links follow the protocol/client-core extraction. It is not a new exhaustive runtime audit; other flow evidence retains the inspection limits above.

## Ownership coverage

| Source area | Principal state or operation | Browse | Authority and interpretation |
| --- | --- | --- | --- |
| `src-tauri/src/startup.rs`, `gateway/`, `attachments/`, window modules | Host refusal/readiness, gateway reconciliation, file tickets, native windows | [Host](services/host/README.md) | Native application owns effects; host-ready and gateway-ready are separate facts. |
| `src/onboarding/`, `src/panel/` | Setup steps/handoff, reads, upload pump, composer display | [Startup](services/host/startup/README.md), [attachments](services/gateway/attachments/README.md) | A UI phase is not provider readiness or gateway authority. |
| `src/conversation/` | Draft submission, local receipts, current views, tab retention, catalogue actions | [Chat](services/desktop/chat/README.md) | Gateway and SDK own server admission; local draft recovery belongs to the UI. |
| `src/desktop/` | Workspace selection, panes, focus, drag, widgets, settings | [Workspace](services/desktop/workspace/README.md) | Label fixture sources and product integrations individually. |
| `src/session/`, `packages/nessa-client/` | Surface/client supervision, wire request settlement, mutations and uncertainty | [Client](services/client/README.md) | Connection retirement and an already-admitted operation's outcome are distinct. |
| `crates/nessa-server/` | Authenticated product admission, deletion, upload and resource tickets, app reviews | [Gateway](services/gateway/README.md) | Metadata tombstones, committed evidence, live provider ownership and audit have named owners. |
| `crates/nessa-sdk/` | Attachment, scheduling, execution, reviews, cleanup, records, fold and checkpoint | [SDK](services/sdk/README.md) | Lifecycle generations and immutable record identity fence stale evidence. |
| `crates/nessa-mcp/`, SDK/gateway MCP modules | Upstream sessions, stand-in grants, app calls, shell command audit | [MCP](services/mcp/README.md) | One upstream session per harness stand-in; no fabricated universal tool lifecycle. |
| `crates/nessa-auth/` | Credential lifetime, admission, membership/audience checks, refusal evidence | [Auth](services/auth/README.md) | Expiry is clock-derived eligibility, not an implicit persisted revocation. |
| `crates/nessa-local-storage/`, `nessa-local-database/` | Retained publication, sync acknowledgement, database opening/version refusal | [Storage](services/storage/README.md) | Physical rename can precede failed acknowledgement; platform exclusions remain explicit. |
| `crates/nessa-images/` | Normalization and format/platform bounds | [Images](services/images/README.md) | Stateless pipeline, not a durable image-service machine. |
| `crates/nessa-agent-credentials/` | Agent secret/namespace values | [Credentials](services/credentials/README.md) | Pure contract; callers own read/write effects. |
| `crates/nessa-gateway-endpoint/` | Listener publication and health-identity correlation | [Endpoint](services/endpoint/README.md) | Discovery is separate from session authentication. |
| `crates/nessa-protocol/` | Shared wire types, conversation views and projection | [Architecture](../ARCHITECTURE.md) | Shared values and projection, without gateway IO or routing authority. |
| `crates/nessa-client-core/` | Device enrollment, private cache and retained reads | [Protected reads](services/sdk/runtime/an-authorized-receiver-reads-physical-history-without-opening-an-agent.md) | Rust client ownership is separate from the TypeScript surface client. |
| `protocol/`, generated contracts | Published vocabulary, close policy and timing/configuration defaults | [Architecture](../ARCHITECTURE.md) | Contracts supply values to actual owner charts; generated code is not a new runtime owner. |
| `scripts/desktop/`, release preparation | Download, validation, staging/publication | [Tooling](services/tooling/README.md) | Developer operations, not long-lived runtime services. |
| External `nessa_ui` and `nessa-extensions` references | Reusable components, streams, MCP App bridge/reference hosts | [Extensions](services/extensions/README.md) | Historical sibling-repository evidence retained; do not claim this sweep audited unavailable siblings. |

## Existing design tables remain useful

The atlas links to, rather than superseding, detailed ordering matrices in
[conversation admission](../design/conversation-admission.md),
[MCP connections](../design/mcp-connections.md),
[app calls](../design/mcp-app-calls.md),
[streaming commit](../design/streaming-message-commit.md),
[transcript fold](../design/transcript-fold.md),
[retained publication](../design/retained-directory-publication.md), and
[credential audit](../design/auth/credential-transition-audit.md).
ADRs still own decisions and preserve accepted/proposed/implemented distinctions.

## Verification boundaries

New documentation validates navigation, file references, metadata and rendering.
Inspect linked tests for behavioral evidence; their presence is not an execution
result. External providers, cloud file providers, native platform behavior,
release provisioning and sibling-repository packages need their own dynamic
verification. The gap register records those limits instead of converting uncertainty
into success.
