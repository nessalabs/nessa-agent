# Fresh assembled-source review

This is a new integration review, separate from the older feature's bounded
review loop. Base `3494d2aa8`, source `589bfa083` for round 1 and `e213c1ccb` for round 2.
All reviews were read-only against the actual checkout and coding standards.

## Round 1

Backend reviewer: no findings after config/vault separation, shared lock and
caller loss, truthful requested/outcome audit, Cedar/channel identity and
membership, allocation origin and TTL, composition and backup-export review.
Existing compiled executable independently passed 27 workspace, 3 parser, 3 domain,
1 identity and 31 actual Cedar tests. Rust sources matched its f7 build.

UI/protocol reviewer: one P2. `runtimeCall` captured channelId before spreading
params, losing non-enumerable own scope or rereading an accessor. Both wrong-scope
requests were reproduced; rejecting the response happened after the effect.
Decoder review found no matching repeated-accessor response defect.

Disposition: fixed e213c1ccb. One captured own scope is explicitly projected into
request and response binding; other own wire fields remain for server validation.
Inherited scope still refuses before request. Read/write non-enumerable and
changing-accessor regressions fail 4/10 on old code and on intentional source
revert; restored client 10/UI 6 pass 16/16. An uncommitted destructuring/rest attempt
still read a getter twice and was discarded before the final own-key projection.

## Round 2

A fresh reviewer found no actionable findings at any priority at e213c1ccb.
Independently passed nine client probes (both methods/non-enumerable/changing
accessor, mode getter changing original target, inherited scope/no effect,
throwing getter/redaction/no effect, unknown-field preservation and global read),
client/UI 16, protocol-generation 47 plus drift, and workspace 27. Checked scoped
identity through dispatch/response, settings publication/audits, Cedar/membership,
provider TTL, UI revision fences and pane-clipping counterfactual evidence.

The later actual parent `31237ca42` was inspected: only relay gap decoding/tests/docs,
with no lifetime/UI/client/composition or worker fingerprint change. No semantic
restack requirement was found; final synthetic-merge CI still must run its tests.

## Exclusions

Neither clean review establishes deployment or final CI completion. Platform
recompilation, full broad/performance completion and WebKit GitHub keyboard
classification remain separate evidence. The original restart-report failure1181
is not resolved by this review. No Cargo build, source mutation, raw gateway-log
inspection or credential access was done by reviewers.
