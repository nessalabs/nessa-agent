# Original issuance and secret-free registration correction

Candidate source `946bbc8647b681fbd34440a116ecb3fb8efe5a6f`
(implementation `a3f5578e1f626cefe031fa04d6d61c6df4571a10`), correction base
`97d5c0991310cf7e10729e3f1cb3c2371be06460`. Registration remains unmounted.
This is a new parent-adjudicated correction, not recovery or retry of the flagged
[historical review delivery](../registration/review-2/README.md).

Auth owns original issuance. Current coherent access snapshots carry that fact;
registration compares command, actor, lifecycle and actual journal identity under
its existing hold. WebhookIssue has exhaustive domain/DTO and Cedar mappings.
Cedar decides both bearer and resumed-session entry from original cause. No
parallel permission table or application installation is introduced.

RegisteredWebhook contains registration identity and secret-free credential
metadata. Initial and replay acknowledgements share it; auth's generated bearer
is consumed internally. Ordinary credential.issue keeps its one-time secret.
Historical AdminIssue remains unchanged and requires explicit registration
recovery. Already-issued uninstalled-intent cleanup needs the pending lifecycle
coordinator; the private intent cap bounds retention meanwhile.

## Behavior and recovery

[Origin construction tests](../../../../../crates/nessa-auth/tests/origin.rs)
cover malformed/cross-credential facts, valid revocation records, actual webhook
command shape, exhaustive cause mapping and original fact retention after revoke.
[Cedar tests](../../../../../crates/nessa-auth/src/adapters/cedar/evaluator_tests.rs)
check every original cause and stale or disabled snapshots. Application unit tests
prove both entry paths propagate policy denial and unavailability.
[Registration regressions](../../../../../crates/nessa-server/tests/webhooks/infrastructure/outbox/registration_origin.rs)
exercise real proof verification followed by bearer/resumed entry refusal, cold
restoration, retirement, historical actors, typed ambiguity recovery and absence
of a substituted synthetic bearer marker from acknowledgement/storage/audit.
[Boundary tests](../../../../../crates/nessa-server/tests/webhooks/infrastructure/outbox/registration_failures.rs)
cover actual actor/command substitution, coherent future-origin replies and
journal sequence/revision substitution, with no installation on refusal.

Failed registration does not roll back an already committed auth issuance. The
original auth fact and private intent survive for explicit recovery. Canonical
lifecycle coordination and private-intent cleanup are not claimed in this slice.

## Qualification

At candidate source `946bbc86`, [auth](auth.txt) passed 214 unit tests and 60 integration
tests, and [product/socket](product.txt) passed 312, with one existing ignored test.
The [canonical webhook suite](webhooks.txt) passed 130 at implementation `a3f5578e`.
[Source checkpoints](source-checkpoints.json) show that only the separate auth
origin integration-test file differs after that implementation; server production
and tests are identical. The complete source is committed before review.

Required seven-package all-target locked [Clippy with -D warnings](clippy.txt),
[architecture checker](architecture.txt), [architecture tests](architecture-tests.txt),
[fmt](fmt.txt) and diff checks passed. The existing lease
constant-evaluation notice is preserved. [Isolated registration](controls/controls-baseline.txt)
passed 30. Six [guard-removal controls](controls/results.json) were caught and
restored, with [byte-identical restoration](controls/source-match.json).
The private harness compiles actual copied registration/outbox modules and uses
Cargo-built dependencies; it is not a product mount or recipient E2E. Its
[arguments](controls/args.json) and [driver](controls/run-controls.py) retain the
specific local environment paths, not a portable new build system.

The [broader initial server run](broad-server-initial.txt) passed 3,876, failed eight and
ignored ten, with two excluded public-bind fixtures. Seven affected setup failures were
corrected: new synthetic credentials now have matching original fixture facts,
and the denial policy double records and admits real Cedar session entry before
refusing the tested operation. Existing behavioral assertions remain. All 312
product tests then passed. The eighth failure is unrelated unchanged Codex process
cleanup, returning CleanupUnconfirmed; it also [failed in isolation](cleanup-isolated.txt).
No unrelated process-lifecycle repair or full-suite green claim is made.

NessaDocs [validated 135 pages with zero issues](docs-validation.json). Four
[diagrams rendered](render.json) without page errors in system Chromium 151;
the changed registration diagram was visually checked. Renderer revision is
c0bc0627611b100ab71a96cc6fe2fb65f1a014d8. This does not qualify gateway browsers.
Fresh independent review of this new correction is pending.

The broad server run excludes two existing composition phone-endpoint fixtures
that bind wildcard interfaces, honoring the explicit task ban on public listeners.
No assertion or fixture is weakened. Canonical Chromium/WebKit, stress/chaos and
recipient capability/revoke E2E remain required. Documentation rendering does not
qualify them.

Copied text logs normalize trailing whitespace only. Earlier failures remain preserved: server fixture partial-move compilation;
policy test using private AuthContext construction (moved into adapter tests);
architecture refusal of application tests constructing Cedar (replaced with an
injected policy double). An early isolated harness linked the older server-feature
auth artifact and did not exercise the new receipt validator; that stale artifact
is not an implementation regression or passing gate. Its genuine reply-metadata
refusal classification regression was fixed by checking metadata before acquiring
its named credential hold. Initial documentation editing used the renderer cwd;
no project file was written until rerunning against the absolute project root.

Full sender/source/W1 composition, recipient E2E and exact-head CI remain pending.
No live credential, production recipient or provider registration is used.
No exactly-once remote-effects claim.
