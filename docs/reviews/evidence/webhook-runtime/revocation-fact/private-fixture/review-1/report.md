# Independent private revocation fixture review

Reviewed clean frozen **d48ae5235dc87fd78fbcfe37d9e65eaf69aadaef** in `/workspace/nessa-webhook-revocation`, branch `codex/896-webhook-revocation-fact`, against published base **8c6e673b69055bdc1b2320e4f1b990b5c795aeba**. Fixture implementation checkpoint **3088a6b2071d2e8646b0b5d4c232528617631b43**; the later candidate commit adds supporting evidence only. Checkout remained clean and unchanged.

**Blocker 0; Major 0; Minor 0; Nit 0.** No finding in this scoped fixture correction. This does not qualify the full PR or complete issue #896.

## Checked relationships and scope

Read AGENTS, applicable coding-standard gates, required review brief, bounded loop, dimensions/agreement, audit and evidence requirements; architecture, codebase organization and typed DI. Inspected the complete incremental diff and evidence, every changed fixture setup/reopen/mutation path, relevant local auth open/error boundaries and private-storage diagnostics.

The only changed Rust file is `crates/nessa-auth/tests/revocation.rs`. Production code, policy, cryptography, endpoints, private-storage verification and feature contracts are unchanged. No assertions were removed or weakened. The author correctly distinguishes the Ubuntu/Windows fixture failures from the separate macOS native-callback timeout and unqualified Windows execution.

| Relationship | Source and verification |
| --- | --- |
| Private leaf versus shared temporary parent | `Fixture::new` creates `private` through the existing `nessa_local_storage::create_directory` owner before passing it to LocalCredentialStore. The temporary parent is not re-permissioned or treated as private. Independent typed security probe verifies this under process umask022. |
| Initial open, cold reopen and adversarial mutations share one authoritative file | The stored `private: PathBuf` supplies initial open, `reopen`, both cold-history JSON paths, and the malformed-history direct open. The negative tests still write and verify bytes at the same `private/auth.json` subsequently read by the actual Auth store. All seven actual integration tests pass under umask022, including malformed-byte preservation/original repair and accepted historical-actor recovery. |
| Resource lifetime survives cold operations and ends before directory cleanup | `store` remains the first declared field and `_root` is last, so the actual store/lock owner drops before the TempDir. `reopen` explicitly takes/drops the old store before acquiring its canonical lock again. Adding the path field does not add another store or mutable authority. |
| Existing privacy refusal stays enforced | The independent probe gets typed `LocalStoreError::Io` with `is_unsafe_file == true` from the raw0755 parent before any file appears; then verifies the child is0700, the child opens, and the parent's0755 mode remains unchanged before and after. No production security switch, relaxed validator or permission repair was introduced. |
| CI trigger is distinguished from implementation correctness | Exact base fixture, using the same current production Auth, independently fails six tests with UnsafeFile under umask022. The corrected fixture passes. Thus the regression is at the test-storage boundary; the source change does not claim to repair macOS callbacks, Windows ACL behavior or any production auth failure. |
| Organization and documentation | The test remains at its existing Auth integration boundary; no API/module/schema moves or speculative owners are added. Supporting evidence retains original failures. The README's no-diagram-change assessment is appropriate: no production transition, guard, effect or recovery contract changed, and referenced paths remain valid. No unrelated state-page checkpoint is advanced. |

## Independent execution and revert evidence

Private artifacts are under `/tmp/nessa-private-fixture-independent-review`. Copied the actual current `nessa-auth` source, policies and test file, then compiled a private actual Auth rlib against immutable `/tmp/nessa-retention-dependency-snapshot`; no Cargo/shared target writes, Auth stand-in or checkout edit. `auth-args.json`, `fact-args.json`, `run.py` and `probe.py` record the build/run inputs. Test compilation uses `-D warnings`; each executed test subprocess receives umask022 before starting, without changing the coordinator process's mask.

- `fixed-tests.txt`: **7/7** actual revocation integration tests pass under umask022.
- `revert-edit.json`, `reverted-compile.txt`, `reverted-tests.txt`: replaced only the private integration test with the exact Git-base version, verified different source hashes, successfully compiled and reproduced **1 pass/6 failures**, each UnsafeFile at initial store open. This is one meaningful compiled revert control, not a production finding.
- `restored-compile.txt`, `restored-tests.txt`: byte-restored the reviewed fixture, rebuilt and confirmed **7/7** pass.
- `probe.rs`, `probe-compile.txt`, `probe-tests.txt`: the additional independently authored shared-parent/typed-refusal/child-mode/no-repair control passes with the seven canonical tests, **8/8** total. The private test file was then byte-restored to the exact candidate, rebuilt and passed7/7 again (`final-restored-compile.txt`, `final-restored-tests.txt`).
- `source-provenance.json`: all copied Auth Rust/policy/schema bytes match the reviewed checkout after controls. Final head/clean status rechecked. Full incremental `git diff --check` passed, and the changed-file inventory proves production source unchanged.

The first private fact build omitted the direct nessa_local_storage extern introduced by this test fixture and failed E0433. Corrected the harness arguments to use the immutable actual library; that initial harness failure is disclosed in `initial-harness-compile-error.txt` and is not counted as a successful check or production finding. All final build/revert/probe stages compiled successfully.

The author's full canonical214 Auth unit+67 integration tests under umask022, required seven-package Clippy and formatting are supporting evidence inspected in the candidate, not independent full-suite/platform executions by this reviewer.

## Explicit limits

This was Linux private-source verification with installed Rust1.99.1, not Rust1.89 minimum-version execution or new canonical Cargo CI. Windows ACL/private-directory behavior, macOS native callbacks and actual new exact-head CI remain unqualified. Canonical scripted Chromium/WebKit, stress/chaos and recipient capability/revoke E2E are not waived for the wider gateway feature; this fixture-only correction changes no UI or gateway behavior and does not itself add a conversation feature.

The separately frozen W24 structural retained-progress finding and supervisor composition are excluded. No authorization, no-send guarantee, sender/replay implementation or full-feature acceptance is inferred from these tests. Parent retains merge control.

No live credentials, registration, real recipient transmission, public listener, GitHub write or network/security change occurred. No denied action was retried, and no old security-flagged report was retrieved or reconstructed. No review action was denied, pending or uncertain.
