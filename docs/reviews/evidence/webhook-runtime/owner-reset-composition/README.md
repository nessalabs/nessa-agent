# Owner reset and retained outcome composition

Source checkpoint: `c790639702213d79bad32aaa148e2744ca7a9d1d`.
Current main `919079a7b13f3e4e8600cc7584b097641f8992c7` is preserved,
including native discovery and UI pin `dac8ff435525ad91cb95365fce80bb768a58b582`.
The reviewed two-file verifier correction from `942e66bf14f0a48091a9f81699cd2b3c9d60093b`
was cherry-picked with provenance at `a2f576696bf8a847e45f6dc5a54df5ab2490ed38`;
both parent helper blobs exactly matched before integration. No whole feature branch
was transplanted. This is local qualification, not issue completion or merge readiness.

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
