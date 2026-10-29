# Daybreak defensive-security closing review — PR #1158

## Disposition

**No actionable findings at any priority. Merge-ready within the reviewed boundaries below.**

This is a defensive closing review, not a blanket security guarantee. I reviewed frozen clean HEAD `fa0449da95f00fc5b7e1be483711d97f96c229a4` (last code commit `05bb0383823d28e37f563ba28cea72fa9c6de9a3`) against base `2f0e4db7e9836855f0bdb743d49e5775a93817d5`, and checked the prospective assembled merge `5e2acef29f619d17ba8d09a79a8922de4032cbdd`. The newer-main delta does not overlap the repaired Boat admission, workspace receipt, relay transport, relay credential, vault, or GitHub binding implementations; its server changes concern permission deadlines/composition and its remaining changes are UI/stress coverage.

## Prior findings re-derived

I treated `/tmp/nessa-1158-daybreak-security-review.md` (SHA-256 `40eda4258e2563ffb9f581b9c1a2348acc9cc5239c8f883299e264181c9aba8f`) as scope evidence only and re-derived the repaired paths from source.

- **Boat posture and receipt admission:** `EnvironmentConfiguration::admit` has one explicit supported posture; the raw environment reader also requires personal-scope access, agreement between declared and effective flags, explicit null default profile, false proxying, a unique current environment/version identity, and refuses unknown fields/states. Create, reconnect, and resume paths recheck before SDK/SSH/harness/MCP authority. Unknown or incomplete creation bodies do not fabricate a target. Complete bounded bodies retain a syntactically valid target before typed SDK decoding. Ready/idempotent replay and unrecognized acknowledgements retain their targets; only the recognized pending-create acknowledgement carries rollback disposition.
- **Receipt lifecycle and cleanup:** the durable history owner accepts only provisioning to assigned/refused/unverified, or assigned to refused with immutable target/image/admission, with a maximum of three transitions. Legacy receipts deserialize only to reader-only `Unverified`. Target, admitted identity, rejection, and cleanup disposition remain coupled through assignment publication, refusal publication, audit failure/panic, cleanup failure/panic, dropped waiters, and restart. Refusal audit precedes cleanup and independently records the target, admitted identity/version, observed mismatch, cause, and disposition. `CleanupOutcome::{Retained,Unconfirmed,Confirmed}` does not turn provider acknowledgement into confirmed erasure; the production Boat remover deliberately returns `CleanupUncertain` after delete acknowledgement.
- **Relay resource bounds and credential scope:** the existing redacted, zeroizing `AccessToken` is acquired per page and dropped before commit/idle. The zeroizing watch ticket is dropped after subscription and before idle. The same bounded collector handles 200/409/410 bodies. WebSocket configuration bounds both frames and assembled messages while preserving fragmented `changed` notifications and full RFC control frames; ping receives pong. Pinned tungstenite 0.29 also bounds handshake accumulation at 65,536 bytes / 512 packets (and 124 parsed headers).
- **Identity/namespace agreement:** binding identity is the signed-in GitHub user plus immutable repository ID and generation; page validation and encrypted-store commit enforce user/repository/cursor continuity before mutation. Stage is compiled configuration and checked in HTTP page/gap responses and the watch request. Existing workspace reconnect retains the saved image and admitted environment identity rather than relocating under changed settings.

## Regression fixtures and negative paths

The committed Boat fixtures are grounded in an authorized read-only personal-scope environment response and a representative VM response. They are useful because they preserve the provider's asymmetric access shapes (`ResourceAccess` object for environments, string owner access for VMs), effective flags, version identifiers, and current receipt vocabulary. Tests mutate those fixtures one field at a time across every credential/setup posture flag, unknown fields/states, missing and mismatched environment/version, changed environment identity, pre/post-resume drift, malformed/oversized receipts, replayed/unknown acknowledgements, and stored-login presence/absence.

The relay negative cases cross real local HTTP/WebSocket decoders: oversized 409 refusal bodies, oversized frame headers, fragmented over-limit messages, valid fragmented notifications, and 125-byte ping/pong. Allocation tests cross fresh versus replayed targets with save/audit/cleanup success, error, panic, cancellation, concurrent opens, and restart fences.

Fixture blind spots remain explicit: these fixtures do not attest Boat's live service, image contents, provider/admin behavior, immutable create-version selection (the API offers none), post-check provider mutation, secure erasure, production TLS/CDN behavior, global denial-of-service resistance, or end-to-end encryption. The raw provider fixtures are snapshots, so a legitimate provider schema addition will fail closed until reviewed rather than prove the new field safe. No live provider, credential, keychain, cloud, or GitHub operation was used in this review.

## Checks run in this review

On the frozen checkout, using only local fixtures/probes:

- `cargo test -p nessa-server workspaces::boat_admission -- --nocapture`: **8 passed**.
- `cargo test -p nessa-server workspaces::allocation -- --nocapture`: **16 passed**, including intentional caught panic paths.
- `cargo test -p nessa-server watch_ -- --nocapture`: **9 passed** (3 relay watch cases plus unrelated name matches).
- `cargo test -p nessa-server oversized_refusal_bodies -- --nocapture`: **1 passed**.
- Working tree remained clean. The fresh build emitted the repository's long const-evaluation warning and a macOS linker compact-unwind warning; neither was a test failure or security finding.

I also inspected the supplied byte-identical verification evidence for the workspace, GitHub/relay, admission audit, receipt audit, all-target Clippy, formatting, architecture, mutation probes, and three relay revert probes. Those results support but do not replace the source/lifecycle review. Current-head remote CI has not run, so merge readiness remains conditional on the normal required CI completing successfully after push.

## Reviewed boundaries and exclusions

Reviewed: Boat environment/VM representation boundaries; admission identity and posture ownership; create/reconnect/resume ordering; assignment persistence/restoration/history; refusal/recovery audit and cleanup disposition; relay HTTP/WebSocket parsing and bounds; per-page token/ticket lifetime; GitHub user/repository/stage/cursor agreement; repaired delta `079798f6..fa0449da9`; and prospective-merge overlap.

Relied on existing full-suite evidence rather than repeating it: SQLCipher vault lifecycle and verified legacy migration, worker Cedar authorization/fair-use implementation, broader offline catch-up/disconnect behavior, all platform/feature builds, UI/browser verification, and documentation rendering. Excluded: real secrets/credentials, live providers or VMs, live GitHub/Cloudflare, deployment/provisioning, secure-erasure measurement, global DoS/E2EE attestation, and unrelated accepted follow-ups #1184/#1185.

**Closing verdict:** the three initial Daybreak findings are closed by code and regression evidence, I found no adjacent actionable defect in this round, and PR #1158 is defensively merge-ready once the frozen head is pushed and its required CI is green.
