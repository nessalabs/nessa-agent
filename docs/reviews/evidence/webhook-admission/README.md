# Webhook admission checkpoint

Source: `d4b6e6cba2dd345c129d00f4115d272944f0f079`, dependent on frozen
foundation `2613b8f68a6f12d08a74903a14fddce2b3cdba1d` (PR #1091).
This verifies authority holds and admission APIs. Registration, outbox, replay,
the delivery worker and usable product delivery E2E remain owed for #896.

## Executed checks

- `cargo test -p nessa-auth --lib`: 209 passed, zero failed/ignored.
- `cargo test -p nessa-server --lib webhooks --no-fail-fast`: 29 passed,
  zero failed/ignored; 3749 unrelated tests filtered out.
- `cargo test -p nessa-server --lib a_grant_is_keyed_by_principal -- --nocapture`:
  one passed, including principal hold, waiting unshare and stale re-share epoch.
- `cargo test -p nessa-server --lib product_revoke_waits_for_effect_hold -- --nocapture`:
  one passed. It observes a queued physical writer before aborting its observer,
  then checks revocation publication after releasing the hold.
- Required CI package-selection Clippy passed with `--all-targets -- -D warnings`:
  local-storage, auth, server, protocol, client-core, device-client and SDK.
- Formatting, architecture and diff whitespace checks passed.

The toolchain is isolated Rust 1.99.1 in `/tmp`. Rustc prints its forced
long-constant-evaluation notice for the pre-existing lease-protocol digest in
`env/config.rs`; commands exit successfully. Initial server test builds caught
missing test-only imports; those were fixed before the passing checks above.
The first architecture invocation lacked Cargo on PATH; the corrected invocation
using the isolated toolchain passed. No unrelated source repair was made.

## Independent review

The admission reviewer inspected all registry/pairing persistence paths,
principal grant gate reuse, the actual composition owner, identity/tenant/audience
correlation, Cedar restrictions, expiry ordering and tests. Findings fixed:

1. Recheck lifetime after grant/metadata waits; expose a dispatch recheck for the
   future worker's post-persistence boundary. The suspended-grant expiry test
   advances time while admission is pending and then expects refusal.
2. Actual LocalAdmin previously called non-waiting revoke_sync. Its retained
   blocking worker now waits through the store's fenced revoke port; the
   deterministic composition test proves observer loss cannot cancel it.
3. Read-only registry and caller-kind lookups must not take the mutation fence.
   Mutating pairing paths explicitly use mutation_registry instead.
4. A hold must retain the canonical registry lifetime lock. The independent
   isolated probe initially reopened the same synthetic registry after dropping
   the store while retaining a hold (`true`). Against the corrected rebuilt
   auth library it reports `false`. The committed regression verifies reopening
   is Locked until the hold drops. No real credentials were used.
5. Move qualified and function-local imports to their module owners; make the
   revoke regression prove the physical writer reached its wait.

The final source review reported no remaining admission/fence findings. This is
not approval of an unimplemented sender, audit/outbox recovery or registration
flow. The worker must actually use the dispatch lifetime check and bounded hold
deadline; this checkpoint does not enforce those absent calls.

## Browsable state

Nessa Docs `c0bc0627611b100ab71a96cc6fe2fb65f1a014d8` validated 121 state
documents with zero issues. The new admission chart rendered in installed
Chromium 151, with zero page errors, and was visually inspected.
[`render.json`](render.json) records the exact source checkpoint;
[`diagram-1.png`](diagram-1.png) is the inspected rendering.
