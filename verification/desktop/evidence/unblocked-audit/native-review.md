# Unblocked audit native/SDK/OAuth review — ordinary round 1

Result: **no findings at any priority** in the assigned region at clean head `4fea9814cb7ba5bc6debaff9cf0a01e1cbcc5b3b`, intended base `965797a9f2cb1a71923f32eee4197aa501669ebf`.

Private checkout: `/Users/nessa/.codex/worktrees/audit-sdk-client-lifetimes/nessa-agent`. Scope: #685 SDK native approval control; #688/#689/#690 shared service-directory authority, canonical installed command replacement and owned configuration publication; #711 OAuth refresh secret generation. Blocked #678/#680/#681/#682/#683/#686/#696 were excluded. This approval covers this region, not the later API additions or whole browser/CI acceptance.

I read the current AGENTS/standards gates, review/agreement/audit/organization/SDK documentation requirements, architecture, codebase maps and typed dependency-injection guidance. I reviewed the actual base-to-head implementation, its published lifecycle/ADR tables and neighboring failure paths. The parent’s native-source identity record establishes identity to parked source; historical approvals were supporting context, rather than substitutes for these fresh probes and checks.

## Relationships and boundaries checked

- `Agent::set_approval_mode` retains the Agent and scheduler exclusion in one owned task. Caller waits own no control permit. `SessionLifecycle::accept_control`, generation-bound `WorkPermit`, and `poll_control` own admission, first-poll stop exclusion, preservation of an already-ready acknowledgement and fencing of uncertain provider state. Mode publication names the accepted provider generation; a replacement attachment returns to the provider binding preset. I followed pending/ready response, waiting scheduler control, caller loss, close, backend construction panic and later queue admission together. The owning common lifecycle retains physical cleanup and audit separately; caller disappearance is not release authority.
- Shared-directory policy is owned by local-storage’s `DirectoryPermissions` and retained descriptor chain. Current-user-owned read/search sharing is accepted, group/other write is refused, later operations verify retained ancestry identity and permission class, and file payloads stay private/unaliased. `open_shared_path` is consumed only for appropriate service publication; runtime/data requirements remain private. Retained leaf and ancestor changes do not silently reacquire authority from an absolute replacement. I inspected publication, file-open, binding-check and removal consumers, including symlink/private-file neighbors.
- Linux `rendered_declaration` parses whole canonical arguments and requires unique policy-bearing assignments and ExecStart. `owned_render` reconstructs the installed Claude directory and PATH, then compares exact complete bytes against the owned render. A desired PATH/directory change therefore uses a fresh complete command generation while recovery/retirement retain the actual prior evidence. I inspected running and inactive replacement, recovery, stop and retained-definition consumers. Foreign definitions refuse before replacement; parsing a plausible attribute alone does not authorize them.
- `ClaudePublication` owns directory changes through durable settings acknowledgement, live host publication, native reconciliation settlement and protected rollback. The command injects owned copies of the existing Gateway/settings store rather than introducing a second settings authority. Equal callers retain the original receipt; conflicting requests wait through settlement. Pointer identity controls claim removal. Native physical success plus failed audit keeps actual durable/live desired configuration and returns typed audit failure; an equal save does not convert failed startup into recovery. Native unconfirmed failure preserves uncertainty even if local known priors are restored. I followed target/before/after/cause/original initiator and later conflicting publication together.
- OAuth `drive_refresh` captures `Reply` generation/resource, compares retained `TokenMaterial.generation` before taking its refresh secret, and rechecks current slot authority before dispatch. Token publication and `release_bearer` check current authority/generation again. Mismatched retained material remains untouched and does not authorize a token POST; same-generation expiry/rejection/rotation and detached-flight neighbors remain accepted. I inspected failure-before-dispatch, persistence/HTTP loss and late publication/refusal paths rather than only the added filter.
- Native feature wiring keeps Linux production dependencies target-specific and injected Linux tests available on macOS. These tests do not assert actual Linux system-manager acceptance. Module maps and ADR 688/689 identify the same owners as code and tests; no source/layout changes were made by this review.

## Independent adversarial probes

Temporary source and logs are under `/tmp/nessa-unblocked-native-review/`; the exact commands/results are in `results.json`.

1. Accept an Auto change behind a held provider response, drop its caller, poll and drop a second Ask caller, then release the response. Both owned effects settle in order and the next admission records Ask. This extends single lost-caller coverage to two callers waiting across the same exclusion boundary.
2. Hold the first approval response, poll a second control while the scheduler is occupied, then close. Both requests return Closed, the old response is retired, physical close is called once and backend mode application count stays zero.
3. Drop the first native directory caller during registration; poll eight same-directory waiters plus a conflicting directory caller. Cross native success with outcome-audit success/failure. Equal waiters retain the original receipt, the conflicting request waits until settlement, exactly two native registrations occur, and final durable/live/installed values agree. Original Main initiator and later Setup initiator remain distinct in intent evidence.
4. Reuse one retained shared service directory across eight permitted owner/read/search modes, preserving each mode while reservations remain 0600; then change it through six group/other-writable modes and refuse binding/acquisition/publication. Restore 0755 and prove a 0644 payload remains refused. This uses real filesystem operations through public storage APIs.

These probes passed: SDK 2, native 1 (two audit outcomes with eight equal waiters each), storage 1 (mode corpus). They add no production test seam and were removed after capture.

## Source-asserted load-bearing checks

`mutations.json` records original/mutant hashes, expected failure and restored result; each diff was captured before running.

- Replace owned approval task execution with caller-owned execution. The two lost-caller probe fails because the later request completes while the original held response should still own scheduler exclusion. Mutant exit 101; source restored with fresh mtime; the original seven approval regressions pass.
- Remove only the refresh secret’s generation filter. The mismatch regression fails for uncached expired generation 2 backed by generation-1 material: actual admission is a generation-3 token instead of Unauthorized. Mutant exit 101; source restored with fresh mtime; all 89 authorization tests pass.

These are deliberate private mutations, not findings against the final source.

## Fresh checks at this head

| Check | Result |
| --- | --- |
| Independent SDK approval probes | 2 passed |
| Independent native publication probe | 1 passed |
| Independent retained-directory probe | 1 passed |
| Restored SDK approval suite | 7 passed, also repeated after mutation restoration |
| Restored native `gateway` suite, `nessa-app --no-default-features`, macOS | 403 passed, including injected Linux unit/reconciliation/staging contracts |
| Restored local-storage `--all-features` | 41 unit + 29 integration = 70 passed |
| Restored server `mcp_authorization` suite | 89 passed, also repeated after mutation restoration |
| Final exact head / source hash / git status / diff check | expected head, captured files restored, clean, diff check passed |

Zero-selected child executables are excluded from these counts. The private target is this checkout’s own `target/`; no primary/shared target was invalidated. All reviewer commands and compilers have ended. Final receipt: `/tmp/nessa-unblocked-native-review/final-receipt.json`.

## Limits

No real Linux systemd/pidfd/atomic exchange acceptance, real launchd service replacement, Windows compilation, signed packaged app, live provider/token endpoint, UI webview/browser, or performance measurement was performed. Native manager/provider/HTTP effects use existing injected substitutes; storage mode probes use real macOS files. The parent owns current combined source checks, actual CI package/feature/platform selection, production/scripted browser acceptance and quiet performance. A later head needs regional source identity or another review before applying this scoped conclusion.
