# Product command/configuration review — round 1

Result: **not clean; publication remains gated**. No authority bypass was confirmed in the reviewed command/share paths. This review does not qualify delivery completion.

## Frozen source and scope

Checkout `/workspace/nessa-webhook-recovery`, branch `codex/896-webhook-recovery`, HEAD `45713603035dfe17e056b58e9c7652a7c8dd0568` plus the seven dirty/untracked configuration/product fixture files in the brief. Published comparison base `af9ea378b3825b5e0f9027efd00dfa970120bcee`; preserved main `d3dbf580283e5d642e10239d44e7ecc8e5248739` UI/verifier changes excluded from owned review. Snapshot `/tmp/nessa-896-product-runtime-current.patch`, SHA256 `022bd2b9f0a124f76cd87132d00d2c2c3cdb6cd90243c04fae1074bc9c0c18f4`. Source was not edited during this review.

Reviewed new concrete runtime, product state/share routing, registration transition result, intent enumeration, private configuration preparation and local-auth/root wiring, new product fixture, and the product runtime design table. Followed adjacent existing Auth issuance/revocation, registration stop/install/cancellation/retirement, SQL physical owner and publication paths. Read AGENTS and the standards review gate; no broad suite or CI rerun was launched.

Claim assessed: real product credential.issue to a configured webhook principal; secret-free actual issuance result; principal-specific conversation.share; retained command tasks and startup command reconciliation. Source/send loop, known-outcome recovery and recipient E2E remain explicitly unmounted. Those omissions are not reported as unexpected regressions.

## Findings

### M1 — Production shutdown never invokes the new admitted-command owner

Location: `crates/nessa-server/src/composition/root.rs:250` and the cleanup closure at `root.rs:533–607`; new owner at `webhooks/infrastructure/runtime.rs:262`.

Reachable ordering: configured product admits credential.issue or credential.revoke; the caller disappears while its retained task waits on actual Auth/SQLite work; host stop closes sockets and drains existing subsystems. The new runtime is only retained as `_webhooks`; neither this cleanup nor the shutdown report calls/observes its shutdown. The host can report confirmed cleanup and release the Tokio runtime without joining this admitted command owner. Holding an Arc does not observe the physical command outcome. The isolated product fixture explicitly calls `runtime.shutdown()` at `tests/webhooks/infrastructure/product.rs:372`, so its successful shutdown does not exercise the production omission.

Expected: after product admission closes, invoke/retain/join this owner in the host cleanup and include its actual success/failure/unknown outcome separately; failures must prevent confirmed cleanup. Preserve dependency ordering for future source/send holds. This is a declared incomplete wiring gap, but it must close before publishing the callable command scope as lifecycle-qualified.

### M2 — Required new-rule adversarial coverage has not been supplied

Location: `tests/webhooks/infrastructure/product.rs:387–452`, `webhooks/infrastructure/runtime.rs:132–327`, `composition/webhooks.rs:38–93`.

The only new test is one successful command/share/unshare/revoke sequence. It does establish the real WebSocket boundary, secret suppression, native principal grant with no receiver, second-chat grant absence, and refusal of a valid webhook bearer as a product session. It does not exercise the newly introduced command supervisor under observer cancellation, issuance contention/capacity, first panic, shutdown observer cancellation/retry, or cold pending-intent recovery, nor ordinary credential behavior through this wrapper. It bypasses `composition::webhooks::prepare`, so duplicate/bounded seed validation, invalid input/no file effects, no-config/no-database behavior, persisted-config removal and restoration are untested. The supplied brief explicitly marks new-rule mutation evidence open.

Expected: deterministic focused tests for these introduced boundaries and one-rule fail/restore controls, preserving the actual native effects and authoritative outcomes. The repository's before-handoff and effect-bearing supervision evidence requirements remain open. This is an evidence gate, not a claim that a particular untested race is already broken.

### m1 — Three introduced Clippy errors fail the required compiler gate

Location: `product/read_access.rs:646` (`policy: policy`), `webhooks/infrastructure/runtime.rs:289` and `:295` (`clone` on Copy Result).

Actual check: `cargo clippy --locked -p nessa-server --all-targets -- -D warnings`, supplied full log `/tmp/nessa-896-product-runtime-clippy.log`, fails these three errors for lib and lib-test. Use field shorthand and Copy values. The existing lease constant-evaluation warning is separate and was not introduced here.

### n1 — Existing with_admin documentation attached to the wrong builder

Location: `product/state.rs:369–377`.

The previous `with_admin` doc sentence now attaches to `with_webhooks`, leaving the ordinary admin builder undocumented and mixing two descriptions. Move its existing sentence back to `with_admin` and keep the new builder's own description.

### n2 — New fully qualified type annotation violates the import convention

Location: `composition/local_auth.rs:126` (`Option<Arc<crate::webhooks::infrastructure::WebhookRuntime>>`).

Import `WebhookRuntime` in the owning module and use its short name, following CODING_STANDARDS Rust imports and type names.

## Agreement/lifecycle checks and rejected hypotheses

- Configuration diagnostics redact endpoint/MAC; raw seeds use the existing bounded private config-file loader. Destination parsing precedes outbox opening and does not resolve DNS or send.
- Issuance uses one retained global gate for configured and ordinary modes. Native intents bind issuer/request, actual selected identities, original times and private configuration; the existing reserve/resolve owner handles repeated server-selected IDs.
- Share classification happens only after request shape and OwnerAccess admission; actual immutable Auth origin selects the webhook branch. Its existing reviewed share operation independently checks current registration/hold/grant/Cedar ownership. The paired fallback remains distinct.
- Revocation consumes actual canonical revocation before observing a held original stop. A possible ordinary-revoke regression was ruled out: native stop_credential deliberately acknowledges absence of a registration.
- A possible false-success shutdown after audit publication failure was also ruled out: intents()/registration() call SqlOutbox.run_checked, which performs journal.finish before their read. An owed publication that still fails makes the drain fail; actual later publication can resolve it. Do not turn this rejected hypothesis into a new receipt abstraction.
- Credential-before-grant lock ordering in the existing share/source primitives is preserved. The new command owner uses short command-state locks and the issuance gate; it does not hold a command-state guard across await. The cleanup observer keeps its real JoinHandle in retained shared state across observer cancellation.

## Checks and limits

Read supplied successful real-product test log `/tmp/nessa-896-product-route-test-corrected.log`: 1 passed, 0 failed. Read corrected architecture test/check logs: 108 passed and architecture check passed. Initial architecture cargo-ENOENT attempt and original fixture compile errors remain historical evidence. Clippy failure is recorded above.

No independent execution of tests, Windows/macOS qualification, native enrollment diagnosis, host-process shutdown probe, first-panic/interleaving probe, configuration prepare probe, recipient POST or browser test was performed by this reviewer. No source files, credentials, listeners or external state were changed. Fresh full review is required after all finding dispositions and author checks/controls.
