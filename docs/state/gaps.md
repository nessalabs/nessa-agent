---
id: "gaps"
title: "Gaps and system improvements"
kind: "guide"
status: "reference"
summary: "Evidence-backed integration gaps, operational limits and statechart-informed follow-ups"
parent: "nessa"
sources: ["src/desktop/dependencies.ts", "src/conversation/application/saved-tabs.ts", "crates/nessa-gateway-endpoint/src/infrastructure/file.rs", "crates/nessa-server/src/mcp_servers/infrastructure/resource_tickets.rs"]
diagramLinks: {}
---

# Gaps and improvements

These findings came from the original [inspection snapshot](inventory.md#inspection-scope). They are not newly reproduced runtime bugs. A bounded current-main refresh marks the shipped desktop and app integrations below. Recheck the remaining findings against their owners before planning implementation; this pass did not repeat a full runtime audit.

## Recommended next work

| ID | Current gap or limit | Useful next step |
| --- | --- | --- |
| G01 | **Resolved integration gap.** Native desktop and browser gateway mode now use the gateway. Other browser previews retain sample data, except a query that names a seeded run. | Keep [connection modes](services/desktop/workspace/README.md#connection-modes) explicit. Sample replies still do not prove a provider ran. |
| G02 | **Resolved integration gap.** [Gateway composition](../../src/desktop/dependencies.ts) now connects server apps through the workspace client. Sample mode retains fixture apps. | Keep resource loading, mount readiness and tool approval separate. This documentation pass did not run a live app. |
| G03 | **Documentation correction.** Older maps called that renderer a placeholder. The [current lifecycle](../../src/desktop/widgets/app/model/lifecycle.ts) and bridge implement it. | Keep the renderer lifecycle separate from workspace connection mode and tool permission. Update the owning pages when composition changes. |
| G04 | **Persistence limit.** [Saved tabs](../../src/conversation/application/saved-tabs.ts) retain IDs and titles, but not drafts or uncertain-send identity and content. | Decide whether crash-safe recovery is needed. If so, design retained exact requests before adding retry behavior. |
| G05 | **Publication limit.** [Endpoint writing](../../crates/nessa-gateway-endpoint/src/infrastructure/file.rs) can fail after replacing the file. | Retain whether replacement happened, separately from final durability confirmation. An error must not imply rollback. |
| G06 | **Audit backlog limit.** [Ticket expiry and release](../../crates/nessa-server/src/mcp_servers/infrastructure/resource_tickets.rs) free bytes separately from audit. End events use an unbounded channel. No memory exhaustion was reproduced. | Design a bounded or durable audit handoff. Test a stalled sink and shutdown without blocking cleanup or silently losing evidence. |
| G07 | **Waiting limit.** [Ticket redemption](../../crates/nessa-server/src/mcp_servers/entrypoint/http.rs) consumes the ticket before waiting for audit, with no independent audit deadline. No hang was reproduced. | Define a bounded wait. A timeout must keep the ticket consumed. |
| G08 | **Deliberate format limit.** The [database](../../crates/nessa-local-database/README.md) refuses unsupported schemas without migration. Its contract also excludes protection from same-user file-name replacement. | Preserve honest refusal. Introduce migration only through a separate design when needed. |
| G09 | **Platform limit.** [Private file publication](../design/retained-directory-publication.md) has different durability evidence on Unix and Windows. | Keep those qualifications and run target-specific checks when storage changes. Do not promise arbitrary power-loss survival. |
| G10 | **Secret-delivery limit.** [Credential recovery](../../crates/nessa-auth/src/application/credential_admin.rs) can confirm issuance without returning its secret again. | Explain that recovery cannot reveal a once-only secret. Changing delivery policy needs a separate design. |
| G11 | **Verification limit.** Source inspection did not exercise real providers, native provisioning, cloud files or external sibling packages. | Run the relevant platform and provider harness for the next implementation slice. Keep [historical controlled regressions](risks.md) separate. |
| G12 | **Handling gap.** [Frontend startup](../../src/main.tsx) treats a rejected host-status question as ready. | Add an explicit unavailable outcome. Test rejection, refusal, ready and late replies. No native failure was reproduced here. |
| G13 | **Acknowledgement gap.** [Panel toggle](../../src-tauri/src/panel.rs) ignores a hide error while reporting a hidden result. | Define what the reply confirms and test a refusing native window. Requested visibility is not confirmed visibility. |
| G14 | **Platform limit.** [Provider-key storage](../../src-tauri/src/agent_credentials/infrastructure/mod.rs) is unavailable outside macOS. | Keep the unavailable result visible. A new adapter needs intent, write and outcome evidence. |
| G15 | **Desktop adapter questions.** [Workspace commands](../../src/desktop/workspace/adapters/store/commands.ts) need clear refusal feedback and agreement on retained resend content versus current model selection. The original questions came from sample-mode inspection. | Recheck the current gateway adapter before treating these as defects. No gateway-backed resend defect was reproduced. |

## Patterns to preserve

- Keep accepted work, agent readiness, completion and saved results distinct. A successful reply about one does not prove the others.
- Keep cleanup and its audit record distinct. A process can stop while its audit fails, or an audit can succeed while cleanup remains unfinished.
- Keep the first cause and caller for consequential changes. Repeated requests must not rewrite that history.
- Say what restoration remembers. A tab reference, saved transcript and external agent context have different lifetimes.

## Applying statecharts

[Harel's statecharts](https://www.state-machine.com/doc/Harel87.pdf) help by grouping related states and keeping simultaneous concerns separate. Start with one owner, such as a queued input or app mount. Use a nested chart to explain that owner's detail. Use concurrent regions only when the facts can coexist and their coordination is known.

A shared Stop request reaches several owners. It does not make cancellation, process cleanup, history saving and audit one atomic event. Those separate results are the useful part of the model.

## Disposition

This PR changes repository Markdown. The browser is maintained separately in [nessa-docs](https://github.com/nessalabs/nessa-docs). It records runtime follow-ups without claiming they shipped. [R1–R6](risks.md) retain their earlier reproduction and correction status.
