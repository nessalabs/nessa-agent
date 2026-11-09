# Independent principal-share review — round 1

Result: **NOT CLEAN** — one major behavioral finding and one import/style nit.

Exact checkout `/workspace/nessa-webhook-recovery`; intended base `8481fe1f6a3fd6fd5248ed125834a881b3554c01`; frozen reviewed head `e64af135023d1f5ac6babb1c859d3b3136ede2ce`; source checkpoint `36edb8e300f5ec8c29923dfd619d310c1ee43e41`. I verified a clean tree before and after review and made no shared source edits, commits or external writes. No review/author build is left running.

## M1 — receiver absence is not preserved by the canonical write or replay

Severity: **major**. Locations: new `webhooks/application/share.rs:130–147` and the reused native `conversation/infrastructure/store/share_grants.rs:844–902,965–970`.

The new application supplies `receiver_id: None` and acknowledges the result as a webhook principal share. The native general grant contract interprets None as keeping an existing receiver; its replay contract also treats an omitted receiver as permission to answer an earlier receiver-bearing command. Those semantics are correct for the existing paired path, but do not enforce the new path's receiver-absent contract.

Concrete native reproduction used unmodified compiled server/Auth/Cedar/grant/registration code. A synthetic, live webhook registration and its owning conversation were created through the actual fixtures/native APIs. The existing canonical `ShareGrants.change` port stored a well-formed same-principal/same-credential read row with receiver `retained-receiver`. A fresh `ShareWebhookConversation.execute` then returned:

```
Ok(GrantedShare { applied: false, policy_revision: 1 })
```

Reusing the original receiver-bearing command identity through the new webhook use case returned:

```
Ok(GrantedShare { applied: true, policy_revision: 1 })
```

The actual stored grant still had `Some("retained-receiver")`, and actual `AdmitDelivery` returned `Refused`. The probe's assertion that an acknowledged principal share must have no receiver failed. This demonstrates a false successful acknowledgement and an unusable grant, without inventing a paired binding or sending anything. It does not establish privilege escalation or that the current product-wire pairing path itself creates this integration/receiver relationship; the input is accepted native retained grant data constructed through the canonical grant port, not hand-authored SQL.

Expected: the dedicated principal/credential-owned path either rejects the contradictory receiver relationship before success/effects, preserving its original evidence, or implements an explicitly designed canonical receiver-removal transition. It must not silently inherit a receiver or accept a receiver-bearing historical receipt as a receiver-absent command.

Ordering matters: the replay acknowledgement at lines844–902 occurs before current live-row selection. Guarding only the later receiver-resolution path at965–970 does not fix replay. Preserve ordinary paired `change` None-means-keep behavior and valid receiver-absent original-request replay after a subsequent unshare; a valid replay must not recreate the old grant.

Suggested narrow disposition: correlate the dedicated operation's receiver-absent meaning inside the actual native transaction for both the original journal receipt and fresh stored row, prior to acknowledgement/mutation. Add design rows and real tests for both contradictions, unchanged original row/journal on refusal, valid absent-receiver controls, original absent-receiver replay after unshare, and unchanged ordinary paired None-means-keep behavior. This is not a request for a new generic framework or extra permission.

Probe deliverables:

- `/tmp/nessa-896-share-review-receiver-probe.rs` — isolated standalone native probe, reusing the actual fixture and registration helper code with public native APIs.
- `/tmp/nessa-896-share-review-receiver-probe.log` — actual fresh and replay acknowledgements, stored receiver and admission refusal; **0 passed/1 failed**, exit101.
- `/tmp/nessa-896-share-review-receiver-probe-compile.log` — successful standalone compilation.
- `/tmp/nessa-896-share-review-1-probe-build.log` — successful unmodified public server library build,1m14s; no source mutation. Public native rlibs were used directly, not a rewritten implementation.

## N1 — group existing module imports and use the imported type

Severity: **nit**. `tests/webhooks/infrastructure/share.rs:4–5` repeats two brace imports from `conversation::application`; lines8–11 split `webhooks::application`; line308 uses fully qualified `std::time::Duration` despite importing `Duration` at line16. CODING_STANDARDS.md's Rust imports section requires grouping types from one module and using short imported names. Merge those groups and use `Duration::from_secs(3)`. No behavioral change is needed.

## Boundaries checked

I read AGENTS.md, the coding standards' gates/review/agreement/evidence/import rules, relevant architecture/codebase maps, and typed dependency injection guidance. I inspected all sixteen changed files and traced the actual canonical grant writer/replay/ownership paths, original paired share behavior, native registration lookup, Auth hold and Cedar target/read policies.

Authority correlation is otherwise sound in the inspected paths: owner admission precedes private lookup; actual held credential/principal/organization/membership/audience/lifetime facts are shared with send/source admission; the second private lookup correlates registration identity and target; existing Cedar owns sharing and comment/drive refusal; the native credential-owned port correlates held credential and supplied principal. No receiver binding, paired scope, product session or permission is manufactured.

For physical ownership, I traced Auth → principal grant gate → metadata connection. No metadata or grant lock is retained while acquiring Auth. The internal ownership mutex is never locked. The same original Auth owner is retained by the overall native retry future and each actual blocking attempt, including waiter cancellation, rollback and panic. The existing paired path delegates with None and retains its original permit semantics. I found no reversed acquisition in this scope. The native task may remain detached after caller cancellation, as the existing store contract permits; its actual fences remain with it until physical completion. This is not an outer product command/shutdown supervisor or proof of a mounted sender's cleanup.

I inspected all seven author control logs/JSONL. They show actual failing tests rather than compile errors. Current application/store SHA256 values exactly match their restored receipts: `00d11aad28a735fbc75a8f6c95e4eb65869d9380058e2fdf4fd8d49bcc8abe56` and `d4f1aa68f0baf80af64a76ea8c427ff0a569030fd3236abf395261c5bceb58ff`. I did not rerun those author mutations or claim them as independent mutations. My additional adversarial probe above uses real native effects and found the receiver contradiction that the absent-row positive tests omit.

I checked design/evidence/state claims and exclusions. The admission Mermaid bytes are unchanged against the intended base; the actual `AccessAdmission` implementation is byte-identical to the earlier d4b6e6cb source checkpoint. Newer coherent origin/revocation decoding is disclosed; this review does not re-audit unrelated issuance/session changes. The state page's sourceRevision points to the actual source commit. Its new receiver-absent claims require M1's correction. No new renderer/browser/platform qualification is asserted.

## Independent execution

Using the supplied Rust toolchain/Cargo home on Linux:

- `cargo test --locked -p nessa-server --lib share_tests`: **11 passed**,0.13s. Log `/tmp/nessa-896-share-review-1-focused.log`.
- `cargo test --locked -p nessa-server --lib a_cancelled_grant_change_keeps_its_permit_until_the_write_finishes`: **1 passed**,0.31s. Log `/tmp/nessa-896-share-review-1-paired.log`.
- Required seven-package locked all-target Clippy `-D warnings`: **passed**,0.15s with existing cached builds. Log `/tmp/nessa-896-share-review-1-clippy.log`. Existing const-evaluation warning is unchanged.
- Standalone native receiver contradiction probe: compiled successfully, then **failed** its contract assertion as documented above.
- `git diff 8481fe1f6 HEAD --check`: passed; frozen head and clean tree rechecked.
- I inspected the author's217-webhook/94-adjacent/architecture/restored-control receipts rather than duplicating the full suites.

## Remaining limits

This is not a clean round; M1 and N1 require disposition and a fresh full review round. Native Windows qualification, macOS enrollment CI failure, outer command/startup/shutdown/source-feed/sender composition, recipient E2E and browser/stress gates remain outside this scoped review. Existing retry/dispatch/browser/project holds were preserved. No live credential, provider registration, production data, external recipient or listener was used. The probe's Auth/registration/database effects were isolated synthetic temporary fixtures only.
