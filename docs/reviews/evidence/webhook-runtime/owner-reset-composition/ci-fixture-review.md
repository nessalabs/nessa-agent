# Issue896 — bounded CI fixture correction review

Disposition: **no findings in the affected three-file delta**. Blocker: empty. Major: empty. Minor: empty. Nit: empty.

Base: `ac6225d7d0e3ef8be69d7cd31b2e689235324687`.
Reviewed head: `4be5703a83f742ef4c48d6e4e2d8a589fb047d0c` in `/workspace/nessa-webhook-delivery-review`.
The tracked author tree matched that head throughout inspection; its pre-existing untracked tooling `node_modules` link is excluded. I edited no source, fixtures or shared build artifacts and started no Cargo build, broad suite, browser or listener.

This is an affected review of recoverable CI corrections, **not a fourth whole-feature review or reset of its counter**. The original whole-feature round-three report and bounded closure remain unchanged; their SHA256 values are respectively `5ef96d4034f94ca55a898df45f6e8f5ee173c8a2f8f3efea5e1b1eb1d5e91d34` and `cdcd7367d4c8b5e69e191ebd4ef145457fcc52518f8a434ae180563797d2e2de`.

## Scope and source inspection

The entire base-to-head diff contains exactly:

1. `crates/nessa-server/tests/composition/webhooks.rs` — retain the temporary root, create its `private` child with the existing `nessa_local_storage::create_directory`, open Auth and prepare webhook storage in that child, and inspect the same actual directory in positive, absent, non-file and symlink controls.
2. `crates/nessa-server/tests/webhooks/infrastructure/runtime.rs` — create the same protected child for Auth, outbox and audit, while returning the retained parent TempDir with the runtime.
3. `verification/desktop/scripts/conversation-stress.mjs` — select `connectClient(..., "panel")` instead of the separate bootstrap-owner token for this webhook step, with an explanatory comment. Nothing else in the step changed.

No production, protocol, SDK/client, Cedar policy, permission, budget or configuration implementation changed. The Rust assertions and timeouts retain their original meaning; path expressions now follow the actual private storage root. The script's selection budget, one exact-identity click, strict unique composer observation, fixed complete Head scope, native no-attempt proof, unshare and cleanup are unchanged. Scope receipt: `/tmp/nessa-896-ci-fixture-review-scope.json`.

### Private-root boundary

I traced the setup to the actual local-storage owner. On Unix, `create_directory` creates mode0700 and verifies current ownership/no group-or-other permissions; on Windows it creates and verifies the existing private ACL. This is the same production helper, not a permissive workaround or a fixture chmod that weakens the adapter. A process-local022 umask therefore no longer makes the temp container's default mode the credential/outbox root. The parent TempDir remains alive for every descendant operation. Auth, SQLite, audit, file enumeration and the negative outbox/symlink targets all refer to the protected child. Invalid seeds still compare before/after contents of the actual target directory; opening seeds still checks the actual Auth file remains absent. The outside symlink marker assertion remains unchanged.

### Conversation-owner boundary

I traced `Run.client = stack.client`, `startGatewayStack(..., as: "panel")`, `connectClient`, `panelCredential`, local Auth initialization/provisioning and `LocalCredentialStore::provision_surface` together. The existing default `OWNER_GRANTS` includes `credential.manage` as well as conversation read/write. The actual panel is provisioned as the distinct `surface:nessa-panel` principal; it is not the bootstrap human principal. Both `run.client` and the corrected setup connection resolve the same provisioned panel token, so creation, issue/registration, owner-only share/cursor/Head and unshare use the conversation's actual owner.

The change does not make a foreign conversation visible or add authority. It corrects which already-authorized actor the fixture uses. The webhook integration remains its own read-only member credential under unchanged Cedar and current registration/source admission; it is not used as the product owner or as a paired reader. Read-only/default diagnostic behavior and transport/address policy are untouched. Cleanup still attempts unshare under the owning principal and closes the separate connection even after failure.

## Evidence and falsification

- The preserved published CI failure log `/tmp/nessa-896-ac6225-ci-failures.txt` shows seven UnsafeFile setup failures (five composition/two runtime), plus the separate stress conversation-not-found failure. The coordinator reported reproducing all seven with the old binary at process-local022; I did not rerun the old binary.
- I inspected `/tmp/nessa-896-ci-private-fixture-restored.log`: corrected composition tests **6/6 passed**, including absence/no effects, malformed seed bounds, maximum seed reopen, non-file refusal and symlink refusal. This was the coordinator's existing owned build/session; I launched no duplicate.
- I inspected the actual terminal `/tmp/nessa-896-ci-webhooks-umask022.log`: **295/295 passed**, finished18.96s, under the coordinator's process-local022 selection. Both formerly failing runtime tests explicitly passed, as did all three actual retention controls, public product/recipient and cleanup controls. The intentional synthetic worker panic is followed by its passing supervision assertion, not an unhandled suite failure. This is observed coordinator execution, not a separately executed reviewer full suite.
- Independently ran `node --check verification/desktop/scripts/conversation-stress.mjs`: passed.
- Independently ran the existing gateway-stack, stress-plan and strict-selection helper controls: **24/24 passed**, `/tmp/nessa-896-ci-fixture-review-helpers.log`.
- Independently ran pinned Prettier on the changed script: passed, `/tmp/nessa-896-ci-fixture-review-format.log`.
- Independently checked the exact three-file scope, absence of production changes, `git diff --check`, and that the complete stress script differs only by the panel selection and its comment. No assertions/budgets or no-send checks were removed. The same-token ownership trace and the corrected negative/positive storage paths were checked to falsify accidental policy bypass or wrong-directory false positives.

## Limits and remaining gates

I did not execute the corrected real browser step: browser acceptance remains pending normal exact-head CI. Helper/source checks do not establish a browser pass or positive TLS delivery from this step. I did not inspect current remote Windows/macOS status; the coordinator reported those jobs still running at admission. Preserve the original failed run40249699947 and old-source failures, and distinguish them from any new-source run after publication. The295 passing local webhook tests do not establish full server/platform or browser/stress qualification.

No denied browser download, rerun/dispatch, alternate browser route, public listener, network/security-setting change, real credential/account/provider registration, real recipient, publication or merge was attempted. No additional correction is requested within this bounded delta; final publication/qualification and guarded merge decisions remain with the coordinating owner.
