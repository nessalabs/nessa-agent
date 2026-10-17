# Paired-reader verification (#724, #719, #746)

Base: `839b1d50352c833c888254130456ac1355cccfa6`. Linux sandbox, Rust 1.99.1 stable, Node 24.19, pnpm 11.19. Synthetic temporary owners, principals, grants, events and stores only. No live credentials or data, schema migrations, permission expansion, system settings, deployment or restart.

The source manifest identifies the changed source and design files; the local Git history identifies the reviewed commit. GitHub publication and exact-head CI are blocked: after an environment-disconnect notification, the existing executor responded successfully but GitHub CLI returned `401 Bad credentials`. No push or PR creation was attempted; no authentication, permission or environment workaround was used. The prepared PR body is handed off separately. Evidence files themselves are excluded from that manifest.

## Behavior and fail-before evidence

`revert-probes.json` records each one-rule mutation, source hashes, command, expected failing-test result and elapsed time. Each probe required a nonzero test exit containing a test failure and no Rust compile error, restored the original file in `finally`, then compared the complete binary git diff to its starting value. Probes cover reader routing, receiver selection, cross-socket quota, callback ordering, bounded recipient work, failed scoped-head queries, publication after caller cancellation, and preparing retries.

The restored tree is tested again after all probes. These are deliberate revert probes against the patch, not a claim that the full original main suite was run with these newly added tests.

`linux-load.json` records 20/20 successful runs of `composition::read_only_online_tests::online_two_devices_follow_live_hints_and_converge`, with four owned CPU busy workers and a copied executable identified by SHA-256. Workers were terminated and waited in `finally`. This executable predates the final characterization comments, formatting and failed-query test; the notice/quota/retry behaviors under test are the same. The final restoration of unchanged Cedar fingerprint inputs occurred later. This does not establish macOS load behavior.

## Review correction before final verification

Self-review found that comment-only edits in `readers.cedar` would alter the raw-source receiver policy fingerprint and therefore pairing epochs. The entire file was restored byte-for-byte to base before restarting the final checks. `unchanged-policy-sha256.json` records the four conversation policy inputs, each compared to base. No policy source is changed by the final patch. The first final combined-build attempt was stopped before tests when this was found; its partial compile is not counted as validation.

## Broad local checks

- Final restored server library run: 3168 passed, 1 failed, 10 ignored (the earlier full run was 3167/1/10 before the failed-query regression was added). The failure is unchanged `agents::infrastructure::codex::tests::status_cleanup_removes_descendants_after_root_exit_and_after_timeout` (`CleanupUnconfirmed`), also failing on an isolated rerun. This is not a separately compiled main baseline result. All affected cohorts passed. `restored-server.json` identifies the rebuilt executable and verifies that it stayed unchanged throughout the run.
- `cargo test -p nessa-client-core -p nessa-protocol -p nessa-device-client`: 675 passed across unit, integration and doc-test executables.
- CI package-selection clippy (`nessa-local-storage`, `nessa-auth`, `nessa-server`, `nessa-protocol`, `nessa-client-core`, `nessa-device-client`, `nessa-sdk`, all targets, `-D warnings`): passed; existing allowed long-running const-evaluation warning remains.
- Architecture, generated product protocol checks, SDK/client docs generation, client/root typechecking, frontend formatting/lint, 12 ESLint-rule tests and production frontend build passed. Client docs generation reported 37 warnings.
- Script tests: initial 428 passed, 2 skipped, 2 failed because `just` was absent. After installing the local tool, all 6 launch-order tests passed, including both earlier failures.
- Frontend Vitest: 4768 passed, 2 failed. Unchanged private-credential and gateway-endpoint tests reject the sandbox's ancestor ownership: `/` and `/tmp` map to UID 65534 while the process and temporary fixture owner are UID 1000. The actual policy was preserved; no ownership or denial bypass was used. The aggregate `frontend:check` therefore did not pass.
- Signed-out scripted gateway/browser verification: all three Chromium suites passed; all three WebKit launches could not run because host libraries are missing. The aggregate verdict remains **could-not-run**, as recorded in `scripted-summary.md`.

## Final combined selection

The repository CI runner built all selected test binaries successfully. It then stopped queuing new binaries when the unchanged retained-directory executable failed 3/30 tests with `PermissionDenied: UnsafeFile`. The observed `/` and `/tmp` owners are UID 65534, while the process and fixture owner are UID 1000. `final-rust-summary.log` records the executed suites and failures. SDK infrastructure (72), application (499), domain (265), client core (418), device client (81), protocol (162 plus 6 wire tests), and all executed auth suites passed. The runner did not run its remaining queue or doctests; the final rebuilt server suite was therefore run separately in full. The same complete package selection passed clippy and formatting (`final-rust-checks.json`). No failed assertion was suppressed or policy changed to obtain these results. The final additional permit-release probe deliberately leaked the new RAII charge, failed at unsubscribe release, then restored the exact source and reran the quota regression successfully.

## Remaining scope and handoff limits

- #724 is partial: former receiver R1 still observes later R2 → R3 replacement timing through its existing journal-derived head and the now-consistent notice. The characterization regression documents this explicitly. Numeric gaps retain the owner's revision numbering. A complete fix needs durable old-head floors before narrowing replacement tracking; alpha metadata has no in-place migration support. No query-only cursor regression or schema version bump is introduced.
- #746 is a finite mitigation: attempts 5–32 preserve retained progress, successful-pass budgets and one registration; perpetual preparation still ends unavailable. The original macOS failure cause and repeated loaded macOS success remain unverified locally.
- Existing Cedar admission is unchanged, including paired list refusal and peer-gateway catalogue-watch refusal. The allowance does not reserve physical pool capacity for another principal.
- Project-board updates are blocked by the current integration (`Resource not accessible by integration`). No scope expansion was attempted. Issues and draft PR contain the handoff instead.
