# Issue896 whole-feature review — round THREE

Disposition: **changes required**. Blocker: empty. Major: empty in inspected relationships. Minor: R3.1. Nit: R3.2. This is not a clean all-priority round, issue completion, publication approval or a merge recommendation.

Frozen review checkout `/workspace/nessa-webhook-whole-review-3`, clean HEAD `8fb396b1625b13ae4c80555de5e46057ccba7980`. Intended main base `919079a7b13f3e4e8600cc7584b097641f8992c7`; implementation/test/client/script checkpoint `c790639702213d79bad32aaa148e2744ca7a9d1d`. I independently checked HEAD, clean status, and no code/test/client/script difference between that checkpoint and frozen HEAD. I also verified the warm author checkout matched frozen source before focused execution and again afterward. No source, tests, fixtures, branch, shared configuration or frozen documentation was edited by this review.

## Blocker

Empty.

## Major

No new major code/lifecycle defect confirmed in the inspected assembled paths. This is bounded review evidence, not a proof of every supported environment or an override of the remaining gates below.

## Minor

### R3.1 — original W-reset requirement still specifies a different reset transition

Location: `docs/design/access-grants.md:1008`, W-reset row. It still says **“Cursor unset for the current incarnation”**, **“The cursor is cleared in this commit”**, and **“The next W-queue starts at the first checkpoint of the current incarnation.”** Its audit after-image is also “after unset.” There is no observed-incarnation reset amendment superseding this row. This design does contain later current dispatch/reconciliation amendments at1109–1111, so a reader cannot consistently treat every W row as merely historical.

The selected owner design and actual implementation deliberately do something more precise and different: `product/webhook_reset.rs` observes a complete J scope, `outbox/source_reset/owner.rs` CASes the frozen generation/registration and stores **J at position zero**, and a following read that observes K freezes again instead of adopting K. Replay returns the immutable original J receipt. The implementation design table (`docs/design/webhook-delivery-implementation.md`, Explicit observed-incarnation owner reset) and the new state page describe this correctly.

This is a documentation/contract ownership conflict, **not** a recommendation to weaken or revert the approved implementation. Align the original W-reset row or explicitly supersede it through the design/flow owner, including its after-image and next-read/replay meaning. One current contract should describe the observed-J boundary, with no unscoped adoption or current-at-commit promise. The coordinating owner acknowledged that access-grants.md is separately owned and remains unchanged in this frozen source.

Evidence: direct source comparison and independently passing `public_reset_commits_observed_j_after_k_replaces_it_then_fresh_read_refreezes_without_k_post` plus the other three public owner-reset controls. The requirement, current implementation and current explanatory tables were traced together.

## Nit

### R3.2 — pinned formatter rejects the new client test, stopping frontend:check

Location: `packages/nessa-client/src/presentation/conversation-api.test.ts:2547–2552`.

The `const request = vi` / `.fn()` / `.mockResolvedValue({...})` chain is not the published formatter's output. Prettier3.9.6 produces a one-line `vi.fn().mockResolvedValue({...})` opening. I independently ran:

```text
/workspace/nessa-webhook-delivery-review/node_modules/.bin/prettier --version
# 3.9.6
/workspace/nessa-webhook-delivery-review/node_modules/.bin/prettier --check packages/nessa-client/src/presentation/conversation-api.test.ts
# exit1; Code style issues found in the above file
```

Log: `/tmp/nessa-896-r3-format-control.log`. The owner's separate pinned pnpm11.9 `frontend:check` result at this same frozen source also stops at `format:check`, before later commands (`/tmp/nessa-896-frozen-frontend-check.log`). The exact suggested formatting-only diff is `/tmp/nessa-896-frozen-owner-test-formatted.ts`. No semantic correction is requested and no assertion should be changed. A full frontend pipeline pass has not been established for this frozen source.

## Independently executed controls

I used the verified matching warm author checkout, with the supplied Rust toolchain and target directory and `CARGO_INCREMENTAL=0`. The first narrow command rebuilt affected test dependencies; I waited for that owned command to finish and launched no duplicate or broad suite. Session1672 completed exit0.

| Selection | Outcome | Log |
| --- | --- | --- |
| `cargo test -p nessa-server --lib 'outbox::store::tests::attempt::' -- --nocapture` | 7/7 | `/tmp/nessa-896-r3-attempt-tests.log` |
| `cargo test -p nessa-server --lib 'delivery::owner_reset::' -- --nocapture` | 4/4 | `/tmp/nessa-896-r3-owner-reset-tests.log` |
| `cargo test -p nessa-server --lib 'pinned_response_refuses_native_cleanup_but_canonical_revoke_and_unshare_commit' -- --nocapture` | 1/1 | `/tmp/nessa-896-r3-canonical-revoke-test.log` |
| `cargo test -p nessa-server --lib 'permanently_refused_registration_does_not_starve_a_healthy_public_sibling' -- --nocapture` | 1/1 | `/tmp/nessa-896-r3-sibling-test.log` |
| Prettier3.9.6 exact changed client-test check | **failed**, R3.2 | `/tmp/nessa-896-r3-format-control.log` |

These thirteen focused runtime/native controls cover pre-pin stop, a queued cleanup crossing pin installation, delayed publication/ACK consumption, lost admission observer, pre-install capacity refusal and unposted reconciliation, namespace lifetime after dropped store handles, due rotation, actual public owner reset and lost-response replay, J/K replacement, actual SDK Head/Page replacement, expiry and queued revoke, canonical unshare/revoke during response pinning, and healthy sibling progress. They do not replace the author's 295-test suite or claim newly executing its historical thirteen mutations. No source was deliberately mutated in this review.

## Inspected assembled relationships

- Native `store.rs` pin install and exclusion before/under the connection mutex, shared owner/file lock lifetime, attempt-specific admission/response/unposted paths, synchronous ACK release and the absence of a Drop release. Checked matching settlement against `journal.rs` bounded published-alternative trimming and retirement, cursor/queue capture generation and the full retained reset proof.
- Serial runtime capture/send/settlement, source/refusal classification and paced due rotation; pre-admission dispatch deadline, partial-fence cancellation versus joined physical work, pre-POST Sending acknowledgement, captured actual response before callbacks, release of Auth/grant authority before local settlement, retained pending outcome and shutdown drain ordering.
- Owner cursor/reset product dispatch, current Auth/Cedar and actual registration/target binding before I/O, default versus requested Head, no native lock across physical read, explicit timeout and retained job/capacity, immutable bounded reset receipt with CAS/generation/replay and expiry checks. Cross-checked public/client shape and declared observed-J semantics against the original W requirements.
- Registration intent selection, actual immutable issuance origin/purpose and secret consumption; current original stop/revocation versus expiry facts and cancellation/retirement credential ownership. Inspected the earlier Auth origin/revocation relationship, canonical writer ordering and relevant Cedar issuance/product-entry restrictions rather than inferring webhook identity from grants or registration presence.
- Principal record source selection/validation versus receiver read scope, canonical Head/Page incarnation classification, current grant/credential fences, share target resolution and canonical grant writer handoff. No fake paired-reader authority or new SDK writer fence was found in these paths.
- Signed HTTPS preparation, pinned addresses/no proxy/no redirect/no automatic retry, response classification and limits; inspected actual mounted root startup and joint cleanup ordering. Existing bounded DNS worker cancellation semantics were not treated as a new blanket wall-clock join guarantee.
- Browser capture/no-send implementation, private auth storage path, unique witness and fixed complete Head scope, default native polling and independent queued/zero-attempt/unstamped row checks, cleanup, synthetic configuration and reviewed helper dependency. This remains source/helper review, not browser execution or positive TLS proof from the browser step.
- Updated implementation/state ordering tables, source checkpoint metadata and existing/new diagram mappings. The access-grants reset conflict above remains open; no new diagram rendering was performed by this reviewer.

## Preserved failures and limits

The author's initial combined294/1 fixture failure and corrected295/295 result remain distinct. The successful all-target Clippy, client131/131, helper34/34, architecture108/108 and historical mutation/source-restoration receipts are author evidence; I did not independently repeat those aggregates. I inspected the current source and ran the narrow controls above. The owner's separate latest frontend preparation reports lint, root typecheck and production build passing; I did not execute those commands independently and they do not change the failed aggregate format gate.

No Windows/macOS runtime qualification, exact-head remote CI, canonical Chromium/WebKit, browser stress default/raised, chaos acceptance, or live service/provider action was executed here. Expected pinned browser executables were reported absent by preparation; no download or alternative browser route was tried. Current main's native discovery/full linking stress was preserved but not started: no public listener was opened. All focused recipient effects were disposable synthetic loopback fixtures. No real credential/account/provider registration/recipient, security-setting change, manual workflow/retry, external comment, publication, merge, budget relaxation or denial bypass occurred.

## Round disposition

This third ordinary round is **not clean**, because R3.1 and R3.2 remain open at exact frozen8fb. No fourth review or implementation patch was started. The coordinating owner must apply CODING_STANDARDS.md's bounded-round and design/ownership escalation before further review/publication; this report does not reset that count or authorize a merge. The implementation follows the approved observed-incarnation design in the inspected paths; the immediate open work is reconciling the one reset contract owner and applying the required formatting without semantic changes, plus all remaining independent qualification gates.
