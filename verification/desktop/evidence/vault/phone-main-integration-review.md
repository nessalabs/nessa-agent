# Scoped phone/main integration review of #1158

Verdict: no actionable findings at any priority in the assigned integration/ownership scope. This is not a whole-tree shipping approval: strict native checks, the refreshed matching Worker image, provider verification and fresh UI evidence remain coordinator-owned.

## Exact snapshot

- Checkout: `/Users/nessa/.codex/worktrees/github-webhooks-1107/nessa-agent`
- HEAD: `31237ca42b287b3d87d6fc4516531d73377b2526`
- MERGE_HEAD: `41339b0ba52dbddb2441389c9ebce1e4f7426c9e`
- Previous main: `d0d253e0201d8644a47f001df97e17f39bb92545`
- In-progress resolved merge; 188 staged files, 4,821 additions/264 deletions relative to HEAD. No unmerged index entries.
- Staged binary diff SHA-256: `c894bee648e090012a6fccdf0207c3298942ca382a871ba3a30172c57d191a0b` (same before and after checks).
- Unstaged diff: only the coordinator's 18-line premerge invariant addition to `docs/adr/todo/1152-local-vault.md`; SHA-256 `8fd6014e40aee30ec6bd3ad6e4467050613ac0e1c600899ef6ac7ef52ac1bef6`.
- No repository edits, commits, Cargo commands, browser interactions or provider operations performed by this reviewer. Test runners used the supplied Node 24 runtime.

## Boundaries and agreement checked

1. Conflict resolutions retain both `ThreadEnvironments`/artifact-channel exports and `ConversationInputFiles`/input audit exports, and both `ThreadWorkspaces` and platform-gated `LocalInputFiles`. The client test union contains independently closed tests for cloud create deadlines and immutable file-only uploads. Workflow early Clippy contains all eight packages including `nessa-vault`; no obsolete late duplicate remains.
2. `composition/local_auth.rs` injects the file delivery port alongside the existing workspace environment adapter. `ConversationService::start_slot` resolves the provider using `AgentPlace`, opens the selected environment, and installs a resolver once on that lease's fence before `Agent::prepare`. `CurrentAgentResolver::placed` sets the provider workspace to the selected channel folder; the new configured workspace fallback therefore agrees with local provider construction. A remote lease's reported workspace takes precedence. Remote terms select `LeaseHold::fetch`, rather than the local writer. Refused openings have no hold and cannot install a delivery resolver.
3. `LeaseFence::with_inner` shares resolver, stop notice and lease state, rather than manufacturing another mutable authority. Ending the lease publishes the stop notice. `LeaseFence::open` binds the captured resolver only to the provider session opened under that lease.
4. File identity remains the original digest/media/size/name reference in admitted/stored input. The delivery copy alone becomes a linked path. `ProviderSession::prepare_input` checks count, order and exact reference correlation and then repeats input validation. `ConversationInputFiles::one` checks the exact conversation hold, verifies the returned workspace path against its expected destination and emits correlated lease/execution/attempt evidence. Its detached physical work keeps the semaphore permit and evidence ownership after caller abandonment. The selected SSH/Boat transport remains the existing held-source fetch/placement mechanism; the workspace connector only forwards explicit existing runtime/relay/file capabilities. No implicit folder mount or GitHub credential forwarding is introduced at these intersections.
5. Lazy `ProviderInputFuture` is polled after ACP acquires its live generation and starts the operation deadline, with generation lock released. Preparation, image reads and native handoff share that deadline. Explicit context-close/channel evidence controls retirement; a resolver diagnostic alone does not retire a native generation. The SDK active invocation and `WorkPermit::native_handoff` still surround the provider call, so preparation does not independently authorize dispatch.
6. No `github_relay` or vault source is changed by this main integration. The pre-existing HTTP410 shared fixture and encrypted inbox/vault ownership are not replaced by the file-delivery path. This is a no-regression intersection check, not a repeat adversarial audit of those subsystems.
7. Module maps in `docs/codebase-structure.md` retain both attachment delivery ownership (including local Unix implementation split) and workspace/channel placement ownership. `docs/ARCHITECTURE.md` retains the no-local-mount alpha contract. Generated product declarations agree with their combined schema in the check below. The Unix-only workspace adapter and cross-platform local-file refusal remain distinct.

## Evidence

Passed on the above staged snapshot:

- `pnpm exec vitest run --config vitest.config.ts packages/nessa-client/src/presentation/conversation-api.test.ts packages/nessa-client/src/protocol/conversation-validate.test.ts`: 204 tests, including both conflict-retained cases.
- `pnpm protocol:check`: generic protocol check passed; 61 product protocol publication/wire/hostile-shape tests passed; product schema, fixtures and generated Rust/TypeScript match.
- `node --test verification/desktop/scripts/lib/stress-plan.test.mjs verification/desktop/scripts/lib/mdns-probe.test.mjs`: 9 tests passed, including live macOS Bonjour registration/withdrawal and stress plan ownership/counts.
- `git diff --cached --check`: passed; `git ls-files -u`: empty.

Read adjacent native regressions, not executed here: server input-file tests cover held identity mismatch, retained input retry after upload release, corrupt source bytes, failed admission evidence, nonblocking FIFO refusal and bounded audit; SSH artifact tests exercise real lease/fetch/SFTP placement before dispatch with exact lease correlation; ACP contract tests cover preparation deadline/close, steering cancellation and hostile source readiness. These tests are coordinator/native CI evidence until run on the combined snapshot; reading their names is not presented as a test pass.

## Explicit limits and dispositions

- No fresh native compile/test result claimed; coordinator owns the eight-package strict Clippy and native suites. This reviewer did not run Cargo or mutate a rule as a revert probe.
- No new claim that Boat-specific uploaded-file product composition has been exercised on a live provider. Source agreement and existing SSH transport regressions were reviewed; coordinator should retain fresh matching-artifact/provider verification before shipping.
- Worker readiness is correctly false until the new lease-fingerprinted source receives a matching published image. The old image cannot establish new-tree readiness; do not weaken the source pin or rollout guard.
- Ubuntu conversation stress explicitly sets `--inputFiles 0`; the Swift phone product remains owned by the separate nessa-ios macOS phone-files stress. This change does not establish Linux phone-product coverage.
- UI checks and exact sourceRevision documentation checkpoint advancement remain with the coordinator. The old UI issues #1162/#1173/#1178/#1179 are not new-tree verification.
- Accepted P3 native audit cap #1185 remains separately tracked; no new finding or scope expansion here.
- Main #1159 as a complete feature and the full relay/vault/Boat stack were deliberately not re-reviewed.
