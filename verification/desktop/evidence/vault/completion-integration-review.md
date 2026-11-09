# Independent mainline integration review for #1158

## Result and reviewed source

**No actionable findings at any priority in the scoped integration.**

Reviewed `/Users/nessa/.codex/worktrees/github-webhooks-1107/nessa-agent` at HEAD
`fd4dbda66e8e6aa4cd1e27fb8d204e2f33a12505`, with the uncommitted in-progress merge
of main `d0d253e0201d8644a47f001df97e17f39bb92545`. This is a dirty-source review,
not an approval of an unspecified later commit. I compared actual working source
against both HEAD and origin/main, and read the ADR 1152 integration table. The
resolved manually owned integration files listed below have combined SHA-256
`6323504be522c14642d5369ea92367286ca0546182ba87dfec6153fc93569423` (path-NUL-content concatenation in listed order).

## Relationships and boundaries checked

- **Product ownership and publication:** `protocol/product/manifest.json` and
  `v1.json`, regenerated Rust/TypeScript product outputs, generator method-bound
  derivation, `product/state.rs`, `product/socket.rs` and composition/local_auth.
  Main's `server.reach`, socket attachment write and both pairing tunnel methods
  coexist with GitHub live events, four vault methods and three Boat methods.
  The ready-method ceiling is derived from the combined manifest (99 non-handshake
  methods), not picked from either conflicted numeric literal. The socket retains
  all dispatch branches; composition registers vault, workspace and tunnel scopes
  independently, and the vault connection guard remains owned by the socket loop.
  No added route bypasses the generated grant catalogue through these resolutions.
- **Transport and CLI:** `nessa-server/Cargo.toml`, CLI entrypoint arguments and
  composition/root. The production WebSocket dependency retains inbound handshake
  support and outbound connect/TLS roots. Main's typed tunnel option survives
  parser, command variant and dispatch; the GitHub passphrase-stdin CLI entries
  survive in HELP and parser. The merge does not substitute tunnel transport for
  GitHub relay ownership or remove the mainline tunnel flags.
- **CI partition/runner ownership:** `.github/workflows/local-auth.yml`,
  `scripts/ci-result.test.mjs`, `scripts/desktop/ci-harness.test.mjs`, package scripts
  and the Linux native installer/release baseline. Ubuntu 22.04 remains the declared
  shipping baseline, which also avoids main's Ubuntu 26 `/usr/bin/env` constraint.
  The Linux cache includes distribution identity. Vault stays in all eight-package
  selections; Windows server-library and remaining partitions remain complementary.
  The dynamic partition assertion does not weaken baseline ownership: the separate
  harness test compares the verification runner to the release matrix. Main's
  package-candidate installer fix and new WebKit menu/onboarding checks survive.
- **UI and verification coexistence:** `new-channel.tsx`, `new-channel.css`, channel
  draft/model/store/gateway tests and the relevant model/commands. Placement remains
  independent for coordinator and workers with Local defaults. Main's file-drop
  watcher, pending landing admission guard, overlay and focus behavior remain;
  both CSS groups are present with separate balanced rules. A placement question
  does not become a resources drop state. No file-mount policy is inferred from a
  retained resource path. `conversation-stress.mjs` retains both the disposable
  GitHub/vault environment and fake cloudflared environment, vault reload unlocks,
  new mainline stress paths and Boat placement assertions. `smoke.mjs` uses main's
  current shared tab selectors/copy rather than the obsolete literal expectation.

The review used AGENTS.md/CODING_STANDARDS.md local gate, agreement across layers,
organization and relevant gates 15–19. No new product state or fallback policy is
introduced by these conflict resolutions; the ADR integration table correctly
states preservation invariants. This review did not copy another review's clean
conclusion as proof of the merge.

## Independently executed checks

- `node --test scripts/ci-result.test.mjs scripts/desktop/ci-harness.test.mjs scripts/ci-harness.test.mjs`: **27 passed**, no failures (host Node 26).
- `node scripts/check-product-protocol.mjs`: **61 generator/admission regression tests passed**, then `Product protocol schema, fixtures, and generated types match`. Includes its own private-fixture malformed grammar/owner-bound/protocol publication refusals; I did not alter the shared source or run a generator manually.
- CI Node 24.19.0 direct Vitest invocation of channel-draft, channel-commands and channel-gateway test files: **3 files / 29 tests passed**. Log `/tmp/nessa-1158-integration-channel-tests.log`.
- Read the current exact UI pin `ca6014a179384154a22978cbbe8eecd341a20cae` prepared by the coordinator. No dependencies or source were changed by this reviewer.

## Limits and final gating

This is a focused conflict/contract integration review, not a fresh exhaustive
security audit of all 375 differing files or every previously reviewed Boat/vault
implementation. Security/lifecycle review is separately assigned. I did not run
Cargo, real Boat provisioning, a Cloudflare deployment, browser/native UI checks
or hosted CI. The coordinator owns full eight-package checks, frozen combined
browser/stress verification and final platform CI before landing. In particular,
29 channel unit checks are not a real-browser layout/focus proof. Existing review
and test results at older source are supporting history, not final-head approval.
No source mutations or independent shared-tree revert probes were performed.

### Fingerprinted integration files (ordered)

- `.github/workflows/local-auth.yml`
- `crates/nessa-server/Cargo.toml`
- `crates/nessa-server/src/cli/entrypoint/arguments.rs`
- `crates/nessa-server/src/product/state.rs`
- `protocol/product/manifest.json`
- `protocol/product/v1.json`
- `scripts/ci-result.test.mjs`
- `scripts/desktop/ci-harness.test.mjs`
- `src/desktop/workspace/ui/channel-creation/new-channel.css`
- `verification/desktop/scripts/conversation-stress.mjs`
- `verification/desktop/scripts/smoke.mjs`
