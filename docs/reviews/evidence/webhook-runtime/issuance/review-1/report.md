# Independent webhook issuance review — round 1

Reviewed head **21516a70ec72341d07d92a290cfc9ddf69dd8894**, implementation
**8e9f8c93277f4654f15632c89503d37be6d8a2e8**, against assembled baseline
**434843c502deb642378a6369ca86cc2ddd3f41b8**.

Scope: the complete 38-file diff for the auth issuing-path/receipt slice, including
12 changed code/test files, design/state documentation and preserved evidence.
The existing outbound foundation and clean persistence implementation are inherited
context, not newly reviewed feature implementation here. This is not full #896
acceptance or merge qualification. Endpoint registration, W1 coordination, source,
sender, runtime lifecycle and recipient E2E remain pending at this head.

A private exact-head archive lives at `/tmp/independent-webhook-issuance-review/checkout`.
Root's original checkout and shared Cargo target were not modified. The private
archive has exactly five appended independent tests; all other reviewed files match
Git blobs at the exact head (`identity.json`, all38 changed paths checked).
Later dirty runtime/DTO changes in the original checkout are expressly NOT covered.

## Findings

### Blocker

None.

### Major

None.

### Minor

None.

### Nit N1 — update the existing path comments for the new issuing path

Locations at the reviewed head:

- `crates/nessa-auth/src/adapters/cedar/policies.cedar:110-112` says a member may
  receive a credential on **any issuing path**. A human/agent member on the new
  webhook path is now correctly refused by `issue.webhook_integration`.
- `crates/nessa-auth/src/adapters/local/registry.rs:594-596` describes
  `issue_internal` as ProductIssue or SurfaceProvision, omitting its newly
  reachable WebhookIssue caller.

Trigger/evidence: `who_may_be_issued_a_credential_on_each_path` and
`webhook_issue_asks_cedar_before_registry_effects` pass while asserting the human
and agent webhook refusals. The exhaustive mapping in `issue_sync` now calls
`issue_internal` with WebhookIssue.

Expected: the nearby owner comments describe the webhook restriction and complete
entry paths. Actual: the implementation is correct but those owner comments retain
prior assumptions. Update the comments; no additional policy mechanism or test
weakening is needed. This is a documentation nit, not an authorization defect.

No other confirmed finding and no new independent structural constraint identified.

## Independently executed evidence

Toolchain: injected Rust1.99.1, Linux, private target directory
`/tmp/independent-webhook-issuance-review/target`; locked Cargo dependencies,
incremental disabled, dev/test debug info disabled.

1. Exact-source `cargo test -p nessa-auth --lib --locked`: **212 passed**,0failed,
   4.01s test execution (`auth-tests.txt`).
2. Five additional tests in the private copied test module: **5 passed**,0failed,
   0.01s (`focused-final.txt`), with exact bodies in `probes.rs` and
   `concurrency-final.rs`:
   - A newly issued webhook bearer authenticates through the real verifier and
     current registry. Existing Cedar authorizes conversation.read and denies
     conversation.write, credential.manage, server.read and environment.serve.
     Current expiry and later revocation refuse subsequent read authorization.
     This is the auth seam, not product socket or webhook recipient E2E.
   - Revoke then reopen then replay preserves the original credential ID and
     issuance transitions, cannot return a bearer secret, leaves registry bytes
     unchanged, and still refuses a purpose switch. The returned metadata is
     CURRENT (includes revoked_at), not an immutable issuance-time snapshot.
   - Empty/unknown grants, missing issuer, inactive membership, mismatched
     membership/principal and invalid lifetime refuse before registry writes.
     Issue/revoke request namespaces remain mutually exclusive, including cold
     replay, without clearing or replacing earlier evidence.
   - An injected test-only GrantPolicyEvaluator changes the webhook hold answer:
     the registry follows the port rather than a duplicate Rust read-only rule.
     Calls are recorded on the actual WebhookIssue path. Production Cedar still
     supplies the restrictive answer; the test substitution is deliberate.
   - Two barrier-synchronized issue calls yield one secret and one original
     receipt (same purpose), or one secret and one conflicting receipt (different
     purpose), preserving only one issuance and its evidence. The existing
     synchronous API may first return Locked under contention; the exact command
     is retried only after BOTH physical calls have joined.
3. Combined exact production source + five probes:
   `cargo test -p nessa-auth --lib --locked`: **217 passed**,0failed,3.97s
   (`combined.txt`).
4. Explicit committed-range checks passed for baseline434843..head21516 and
   implementation8e9f8c93..head21516 (`committed-ranges.json`).

The first concurrency harness incorrectly assumed the synchronous API waited
instead of returning Locked (`focused.txt`). Its first correction joined lazily
and retried before the sibling necessarily finished (`focused-corrected.txt`).
Those are independent test-harness errors, not product defects. Original failing
outputs and both intermediate test bodies remain beside the corrected deterministic
probe; they are not claimed as implementation regression failures or concealed.

## Relationships and boundaries checked

- **Authority/identity:** CredentialIssuePurpose is a server-selected enum with
  exhaustive registry mapping. Current product callers explicitly remain Product;
  no new user-controlled JSON purpose selector or paired-receiver fabrication.
  Issuer/request scope remains the existing receipt identity; the webhook purpose
  becomes part of its fingerprint. Proposed retry ID/time remain distinct from
  the original committed identity. The metadata/transition receipt is read-only
  evidence, not new issuing authority.
- **Policy/domain invariants:** the same GrantPolicyEvaluator owns recipient and
  grant decisions. Cedar's new forbids restrict kind and action; existing
  member/admin policies own role. Registry validation still owns audience,
  resource organization, membership linkage, identity availability, nonempty
  grants and credential lifetime. No hardcoded policy conclusion was added to
  Rust. Baseline matrix plus additional malformed and substituted-port tests
  cover both refusal and valid combinations.
- **Admission/concurrency:** issuance still uses the registry's existing mutation
  fence and mutex; the enum and fingerprint introduce no second lock or queue.
  Controlled contention probes cover same/different purposes and preservation of
  original ID/secret count. W1 registration/sender coordination is absent and is
  explicitly deferred, not inferred from this registry test.
- **Audit/restoration:** issuance still commits credential/verifier, receipt and
  canonical transitions together through existing persistence. Cause remains
  AdminIssue, with original issuer/correlation. Cold replay, revoke and command
  namespace probes correlate current metadata with original immutable issuance
  evidence, without inventing revocation history or replacing it. A revoked
  credential's receipt is not proof that it is currently deliverable; the future
  coordinator must consume current metadata/current authority.
- **Failure/cleanup:** new policy refusals produce typed Conflict before writes;
  no new physical resource or cleanup state is introduced. Existing failures and
  supervision were traced, not redesigned. Independent tests did not inject every
  filesystem commit/publication failure; those unchanged paths are supported by
  existing suite coverage, not claimed as newly exercised by these five probes.
- **Representation/resources:** purpose has two fixed variants; stored receipt
  remains a fixed-width SHA256 fingerprint. No secrets, endpoints, keys or bodies
  were added to DTO Debug/audit/logging. Added request data does not create a new
  unbounded collection, allocation owner, network effect or remote diagnostic.
- **Public surface/organization:** new purpose lives with the consuming auth
  lifecycle request; issuing path remains with the existing grant policy port;
  mappings and policies remain in Cedar. Fixture callers/imports updated. New
  adapter tests live under tests/adapters/local and are wired test-only. No new
  dependency, facade behavior, domain/application inversion or unrelated #1052
  implementation found.
- **Docs/evidence:** reviewed ordering table, explicit pending integration,
  sourceRevision8e9f8c93, navigation, all result/count claims against stored logs,
  mutation scripts and original failing logs. Visually inspected the preserved
  final Mermaid image; no spurious semicolon-created nodes remain. The stored
  NessaDocs validation/render reports match their stated source and show135
  documents/zero issues and Chromium151/zero page errors. I did not rerun that
  renderer; documentation rendering is not gateway browser qualification.

## Gate applicability and exclusions

The implementer's server all-target check, seven-package all-target Clippy,
architecture and six fail/restore logs were inspected, not independently rerun
by this reviewer. Independent execution here is the auth baseline/probes/combined
suite and committed-range/source identity verification. Minimum supported compiler,
macOS and Windows were not executed; no new platform-specific code was introduced.

Gateway behavior changes require canonical scripted Chromium/WebKit evidence;
feature conversation stress and chaos default+raised remain applicable. Those
remain unqualified, regardless of the current product handler selecting Product
and regardless of successful Rust tests/docs render. No browser waiver is given.
No full registration/revoke/source/sender or actual recipient E2E is claimed.
Foundation and persistence slices, endpoint/redirect/SSRF policy, physical delivery
limits, retry signatures, crash settlement and production mounting require their
own exact-head final assembled verification.

No CI retry, manual workflow lookup/dispatch, browser download, GitHub mutation,
network listener or real-recipient operation was attempted. The parent's existing
foundation CI denial remains binding and was not retried or bypassed. No tool
action was denied, pending or uncertain during this review; only ordinary local
harness/working-directory errors occurred and were resolved in the private copy.
The archive has no .git, so one attempted diff command there returned Git usage;
the actual committed-range checks were then run read-only against exact Git SHAs
in the source repository. Review result is N1 pending; no review/merge approval is
extended to the parent's later dirty composition work.
