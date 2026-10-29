# Independent webhook issuance review — round 2

**Reviewed head:** `911bdf74e4abf7526d50b8a53c858c950be8c677`.
**Intended base:** `434843c502deb642378a6369ca86cc2ddd3f41b8`.
**Behavioral implementation:** `8e9f8c93277f4654f15632c89503d37be6d8a2e8`.

Result: **clean for the committed issuing-path slice**. This is not full issue896
acceptance, runtime qualification, CI approval or permission to merge.

## Findings

### Blocker

None.

### Major

None.

### Minor

None.

### Nit

None.

## Prior finding disposition

First-round N1 is **fixed**. At this exact head:

- `crates/nessa-auth/src/adapters/cedar/policies.cedar:107-112` no longer says
  members may be issued credentials on any issuing path. It names the webhook
  integration restriction and retains the separate offline admin paths.
- `crates/nessa-auth/src/adapters/local/registry.rs:594-596` names ProductIssue,
  WebhookIssue and SurfaceProvision and distinguishes addition from rotation.

Compared with prior reviewed head21516a70, the only source changes are these two
comment blocks. I verified both whole source-file differences and equality after
removing full comment lines (`closure-checks.json`). The full original report,
including its one nit and independent harness failures, is preserved under
`docs/reviews/evidence/webhook-runtime/issuance/review-1/`; nothing was silently
reclassified as a passing implementation test.

## Exact checkout and review scope

I made a private `git archive` of the reviewed head at
`/tmp/independent-webhook-issuance-final-review/checkout`. All **53 changed paths**
matched their exact Git blobs before execution (`identity.json`). The archive
contains the committed auth request/purpose, Cedar path/policies/matrix, registry
mapping/fingerprint, updated ordinary callers and fixtures, new adapter tests,
ordering design, state page/navigation, evidence and first-round report.

I reviewed the complete baseline..head diff and adjacent owning code, rather than
only the N1 patch. The parent's original worktree has later dirty registration,
intent, schema and Serde changes. **Those are excluded**: they are absent from
this archive and receive no approval from this report. The inherited transport,
admission and persistence foundation is context, not a newly reviewed complete
outbound implementation. The private archive was modified only by appending two
review-only probes to the adapter test module; all other reviewed files still
match exact head. Final identity checks are in `closure-checks.json`.

## Independently executed checks

Linux, injected Rust1.99.1, locked Cargo dependencies, incremental disabled,
dev/test debug info disabled. Cargo used only the already-completed private target
`/tmp/independent-webhook-issuance-review/target`; the parent's shared target and
original checkout were untouched.

- Exact-head baseline `cargo test -p nessa-auth --lib --locked`:
  **212 passed**, zero failures, **7.51s** test execution (`auth-baseline.txt`).
- Two additional independent tests: **2 passed**, zero failures, **0.01s**
  (`focused.txt`), full bodies in `probes.rs`:
  - Deliberate Deny and Unavailable from an injected GrantPolicyEvaluator at
    either webhook recipient or grant-holding stage return typed Conflict and
    leave canonical registry bytes unchanged. Ordinary bootstrap still succeeds
    with the real Cedar answer, making the substitution scoped to the actual
    WebhookIssue path. This checks both authorization refusal and authority
    unavailability before effects, without adding a second Rust policy owner.
  - At exact configured credential/receipt capacity, a cold matching webhook
    retry returns the original identity and transitions without another secret;
    a purpose switch returns Conflict before the capacity outcome; an actually
    new request returns Capacity. All leave canonical registry bytes unchanged.
- Combined exact production source plus these two probes:
  `cargo test -p nessa-auth --lib --locked`: **214 passed**, zero failures,
  **4.08s** (`combined.txt`).
- Explicit `git diff --check BASE..HEAD` passed against assembled baseline434843,
  prior review21516 and implementation8e9f8c93 (`committed-ranges.json`).

No execution failures or test-harness corrections occurred in this round. I
inspected, but did not independently rerun, implementer evidence for97 canonical
server webhook tests, server all-target check, required seven-package all-target
Clippy with `-D warnings`, architecture checker and six fail/restore probes with
restored controls. The first-round five extra probes/combined217 remain historical
first-round evidence, not tests I claim to have independently executed here.

## Relationships and applicable review dimensions

- **Authority and identity:** purpose is a server-selected two-variant request
  fact, with exhaustive mapping inside the existing registry to IssuePath. It
  grants no authority itself and exposes no JSON selector in product requests.
  Current socket callers explicitly select Product. The fingerprint binds webhook
  purpose to issuer/request receipt identity; proposed server ID/time remain
  distinct from the original committed credential. Cloning this input DTO does
  not mint a secret or duplicate mutable registry authority. No paired receiver
  or application installation was fabricated.
- **Domain invariants and policy:** new restrictions live in the existing Cedar
  GrantPolicyEvaluator. New forbids restrict the webhook kind/action, while
  existing member/admin rules own roles. Registry validation remains the owner
  of linkage, audience/resource identity, nonempty grants and lifetime. Rust
  maps facts and checks receipt identity; it does not compare kinds/actions to
  decide the new policy. The matrix and real adapter tests cover allowed and
  refused combinations. My injected Deny/Unavailable probes confirm the owner is
  consulted and failure closes before effects.
- **Admission, concurrency and cleanup:** no new lock, task, listener, queue,
  dispatch owner or shutdown state is introduced in this committed slice. The
  registry's existing mutation fence/mutex still owns issuance; unchanged
  concurrent/direct/revoke paths remain in the baseline suite. First-round
  controlled contention probes were inspected, not presented as my execution.
  Registration W1, caller-loss ownership and sender coordination are absent at
  this head and remain integration work. No guarantee is inferred from raw
  issuance receipt tests about those absent paths.
- **Audit, observation and restoration:** fresh issuance still commits verifier,
  metadata, receipt and original AdminIssue transition through the existing
  persistence owner before returning secret evidence. Replay returns original
  issuance transitions and currently stored credential metadata, not an
  issuance-time lifecycle snapshot: revoked metadata is feasible with the
  original issuance audit. The source read and first-round cold-revoke evidence
  make this distinction explicit. Purpose substitutions conflict without
  replacement or new effects, including at capacity and after reopen. No new
  parallel history or fabricated revocation cause is added.
- **Failure semantics:** policy refusal/unavailability retain existing typed
  Conflict semantics; resource pressure retains Capacity for genuinely new work.
  Matching receipts resolve before capacity and cannot replay bearer evidence.
  Unknown storage outcomes are not newly interpreted by this patch. I did not
  inject all unchanged file commit/publication/observer-loss failures and do not
  imply that these probes prove future registration settlement.
- **Representation and resource bounds:** purpose has two fixed variants; its
  only stored change is the existing fixed-width hash fingerprint. No additional
  retained collection, body, endpoint, key or secret Debug representation is
  introduced. Existing input/resource bounds remain their owners' contracts.
  New tests and request field add no platform-specific code or new dependency.
- **Public surface, DDD and organization:** request purpose sits with the auth
  lifecycle DTO; IssuePath stays with its existing policy port; Cedar mappings
  and policy remain under the Cedar adapter. The named adapter test file is
  wired test-only. Ordinary production/fixture callers and imports are updated.
  No domain/application inversion, facade behavior, compatibility reader,
  unrelated #1052 implementation or additional network/crypto mechanism found.
- **Documentation and evidence:** state status is mixed and sourceRevision8e9f8c93
  points to the inspected behavioral implementation. Later comment-only closure
  does not falsely imply additional behavior. Existing unrelated historical
  pages retain their checkpoints. Navigation, test/source links and diagram node
  bindings are consistent with their files. I visually inspected the preserved
  final diagram; the earlier semicolon-created nodes are absent. NessaDocs135/0
  and Chromium151/0-page-error evidence was inspected, not freshly rendered by
  this reviewer. It is accurately labeled documentation evidence. Pending
  registration/configuration, source/sender and recipient E2E are explicit.

## Remaining gates and exclusions

Canonical scripted gateway Chromium/WebKit evidence and feature conversation
stress/chaos default+raised remain applicable and **unqualified**. The ordinary
product handler still selecting Product does not waive those gates. This review
qualifies only the committed auth slice; it does not validate production mounting,
registration W1, grant/credential stop coordination, source capture/replay,
supervised sender, actual recipient effects or assembled cancellation/recovery.
It does not claim exactly-once remote effects.

Server/package Clippy/checks and docs rendering were inspected from preserved
implementer evidence, as distinguished above. macOS, Windows and minimum compiler
were not run; no CI run is claimed by this review. Foundation6c's denied retry
remains stopped. No GitHub mutation, CI retry/lookup/dispatch, alternate trigger,
browser download, live registration, credential, recipient, listener or network
configuration operation was attempted. No action in this review is denied,
pending or uncertain. Original checkout and shared target remain untouched.
