# Owner reset and retained outcome composition

Original assembled source checkpoint: `c790639702213d79bad32aaa148e2744ca7a9d1d`.
Owner cursor/reset time-precision correction: `69351af72cbd73179f7f8765424251ecf4a20b99`.
The original qualification preserved main `919079a7b13f3e4e8600cc7584b097641f8992c7`,
including native discovery and UI pin `dac8ff435525ad91cb95365fce80bb768a58b582`.
The reviewed two-file verifier correction from `942e66bf14f0a48091a9f81699cd2b3c9d60093b`
was cherry-picked with provenance at `a2f576696bf8a847e45f6dc5a54df5ab2490ed38`;
both parent helper blobs exactly matched before integration. No whole feature branch
was transplanted. This is local qualification, not issue completion or merge readiness.

The earlier [current-main integration](current-main-integration.md) is
`3e7e965d6b5edbcf29ff6926d736d21090268bdb`, preserving main
`2026c2becebec94ec69f849fa529143ad4625995` and UI pin
`3118f2d8a15f68825ecef3fe0c337368b3d7f889`. Earlier UI/platform results below
retain their original source inputs.

The subsequent main `aef11ea3b3d0c96f8c96fd9509f5657c58693780` and reviewed
focused webhook CI addition are integrated at
`108ce259cdc7b2628a19329181886de21cd88d3c`; the
[focused coverage receipt](focused-browser-coverage.md) records exact CI checkout,
the successful default Chromium witness and still-unexecuted focused cohorts.

Run 40257115915 is terminal: all jobs except Windows server passed. The
[preserved Windows failure](ci-491-windows-failure.txt) records one native lifecycle
completion assertion failure (3,394 passed, one failed), not a timeout. All three
original retention controls and the late-response boundary control passed. Default
Chromium stress passed 50/50; its webhook witness captured Head 46 with 46 queued
records and zero attempts. This does not qualify the unexecuted focused WebKit
and count-three additions, or a later source/main/UI combination.

The subsequent [bounded integration closure](main-3a220-integration-closure.md)
checks `c5cd751f4a7c75c9a2e9966436eada47de87bddd`, preserving main
`3a22008c606d6867d02f4bd242ff27944726aa3f` and UI pin
`39e617600ad36fd9c7a1f41691aca239af25b495`. The test-only
[native lifecycle correction](native-lifecycle-qualification.md) has an independent
[affected review](native-lifecycle-review.md); it does not change production
budgets or establish a corrected Windows pass.
The [current frontend receipt](main-3a220-frontend-summary.txt) records 5,306/5,306
tests across 409 files and a successful build after the separately reviewed
[asynchronous focus fixture correction](linked-focus-qualification.md)
([affected review](linked-focus-review.md)). Original failed invocations remain
recorded. No new-head platform or focused browser pass is inferred from these
local checks.

The subsequent actual CI merge is
`f9e906be369d7abddbad72893143283e4d95c798`, combining published `c2ac8c4f`
with upstream main `be8f53f80b7cd1fb7d49de508351e8c068238483`. Its
[bounded source-impact review](ci-f9e-integration-review.md) has no findings.
Full default50/50, focused WebKit1 and chaos2/2 passed there; raised Chromium
case2 failed the independent SQLite probe and case3 was not run. The
[contention diagnosis and meaningful controls](sqlite-proof-contention.md) and
[affected review](sqlite-proof-review.md) preserve that original failure and
distinguish the local correction from still-required corrected browser CI.

## Behavior and ordering

The exact native delivery is pinned before Sending through consumed settlement.
Matching response and unposted reconciliation cannot reclaim their own receipt;
other native mutations may promptly refuse Unavailable while pinned. Canonical
revoke/unshare retain their own authority fences. Temporary source/preparation
refusals create no attempt and cannot starve a healthy registration.

The owner-only reset binds position zero to the complete observed incarnation J,
CASes the active registration and frozen capture generation, and retains bounded
immutable receipt evidence. Replay freshly authorizes and returns original J after
later replacement/reset/restart. It does not rewind again or promise current-at-commit.
A next read seeing K freezes without K queue/POST. Existing delivery payloads retain
their own incarnation. See the [ordering table](../../../../design/webhook-delivery-implementation.md)
and [statechart](../../../../state/services/gateway/webhook-outbox.md).

The optional owner diagnostic snapshots the native checkpoint before physical Head.
Its two observations are non-atomic. Omitted/false uses no canonical-read admission,
capacity or Head I/O; requested failures and deadlines are explicit. The retained
runtime owns physical work and capacity through completion after caller timeout.
It does not reconcile, publish, freeze/reset, advance, queue or POST.

## Qualification

- [Restored current-main webhook suite](restored-webhooks.txt): **295/295**, no retention exclusions.
  This includes actual public issuance/share/reset, canonical SDK reads and signed
  loopback recipient delivery, stable retry bytes/identity/signature, capability,
  unshare/revoke/expiry/old-epoch no-send, source replacement, recovery and owned cleanup.
- [Required seven-package all-target Clippy](required-clippy.txt): passed `-D warnings`
  for local-storage, Auth, server, protocol, client-core, device-client and SDK.
- Client owner API: **131/131**, typecheck and documentation preservation passed.
- Shared browser capture/no-send helpers: **34/34**; architecture **108/108** plus checker;
  product schema/generator checks and affected-file ESLint passed.
- [Thirteen reset mutations](reset-mutations.jsonl) all caught the intended failures
  and restored each altered source byte-for-byte before the 289-test restored suite.
  These preceded the optional diagnostic and current-main integration. The current
  [equivalence receipt](reset-rule-equivalence.json) proves nine complete production
  files unchanged plus the target-expiry derivation unchanged in the changed product
  handler. Whole-file equality for that handler is deliberately not claimed.
- The [initial combined run](initial-current-main-webhooks.txt) was **294 pass/1 fail**:
  management-only fixture issuance conflicted before exercising the query. The
  corrected fixture bootstraps that explicit grant set through the existing setup;
  no production policy changed. Initial schema-description/generator-PATH failures
  were corrected before documentation qualification; no missing field check was removed.

The browser step sends a unique witness through one fresh exact-identity UI
selection, captures one Head after commit, then polls the native cursor against
that fixed complete scope. Separate read-only native queued/zero-attempt/unstamped
rows must reach that Head. The storage path was traced to actual private
`auth/webhooks.sqlite`. Production localhost refusal remains unchanged. The count
is seeded at one by default and explicitly tested at three; these are helper
controls, **not executed browser stress acceptance**. Positive TLS delivery above
comes from the separate real loopback recipient fixtures.

## Round-three finding closure

[Round three](review-3.md) originally required changes: one minor conflicting
canonical W-reset row and one formatting nit. No blocker or major was confirmed
in the inspected assembled paths. Thirteen independent focused controls passed:
[pin7](review-3-attempt-tests.txt), [public reset4](review-3-owner-reset-tests.txt),
[canonical revoke/unshare1](review-3-canonical-revoke-test.txt), and
[healthy sibling1](review-3-sibling-test.txt).

Both exact findings are [closed at37611fac240304fffb8dd69fae101ff5f1ac664d](review-3-closure.md).
Only the single canonical W-reset row and formatter layout changed. The reviewer
verified identical complete TypeScript and transpiled-JavaScript ASTs, including
all assertions, and [pinned formatting](review-3-closure-format.txt). The
[closure receipt](review-3-closure-checks.json) records that emitted bytes differ
because line wrapping is preserved; byte identity of the formatted test is not
claimed. All production remains byte-identical. This is bounded finding closure
under round three, not a fourth full review, counter reset or relabeling of the
original report as finding-free.

## Remaining gates and exclusions

The later owner cursor/reset precision correction and its preserved browser
failure, deterministic control, independent review and actual gateway probe are
recorded in the [subsecond correlation receipt](subsecond-precision.md).

The published-head CI failures and narrowly reviewed fixture correction are
tracked in the [CI correction receipt](ci-fixture-repair.md). Original failures
remain visible; corrected real browser and platform acceptance are not inferred
from local fixture/helper passes.

Exact-head platform CI, canonical browser, default/raised stress and chaos
acceptance remain required. The complete native-filesystem `pnpm frontend:check` passed at37611, including
**5,324/5,324 unit tests**,408 desktop helpers,108 architecture tests/checker,
61 protocol controls, client docs/types, root types, formatting/lint and production
build. The actor remained unprivileged UID1000 with normal root/tmp UID0 metadata.
The earlier sandbox run passed5,322 and failed two unchanged private-file positive
fixtures because root/tmp mapped to UID65534; both passed in native context before
the complete pipeline passed. No ownership rule/assertion changed. Earlier format
and missing-just failures are also preserved. The [qualification receipt](qualification.json)
records source, toolchain, counts, failures and unrun gates. Earlier source-specific
results and the original Windows failures remain historical. Native discovery's
upstream full stress step now listens on all interfaces; it was preserved and not
started here. No real credential, account, provider registration, production data,
public listener or real recipient was used. No exactly-once remote effect is claimed.
Existing denied retry/download/manual-dispatch/project-board actions remain held.
