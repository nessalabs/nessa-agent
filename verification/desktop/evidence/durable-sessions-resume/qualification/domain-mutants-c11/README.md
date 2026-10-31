# Isolated actual-source domain negative controls

Source checkpoint: `6d1b413b19d2af4355fdf331778fe98fa4a683a4`.
Source SHA256: `6799f560d58ac906b7c09c452c75725453fe45b152610b6b0ed4312e67b42538`.
Test SHA256: `6726da7473a6fd09ff4278cf25c888a8f5bb04a89594d5317a5f811b3254b148`.
Identity dependency: `/workspace/scratch/durable-qualification/domain-mutants-c11/libcurrent_sdk.rlib`; SHA256 `9533c6010c6ac5e388e9fac9e81c53fe45e1b788ae0ee430f0731d30a595059a`.
Compiler: `rustc 1.99.1 (58cf8f9c3 2026-10-13)`.

The harness compiles a hash-verified copy of the actual attachment.rs and the unchanged current provider_context.rs tests with rustc --test. Only ExecutionSessionId comes from an existing SDK rlib copied into this isolated evidence directory; the artifact supplies only the immutable identity type, while admission rules are compiled directly from the hash-verified current source copy. No replacement admission rules, Cargo invocation, shared artifact writes or root source edits are used.

| Probe | Result | Expected failing regression |
| --- | --- | --- |
| baseline | test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s | none |
| watermark_bound | test result: FAILED. 8 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s | admission_boundary_cannot_skip_pending_work_or_exceed_history |
| reinstate_exact_publication_prefix | test result: FAILED. 8 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s | replacement_preserves_opening_boundary_after_terminal_prefix_grows |
| same_id_boundary | test result: FAILED. 9 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s | repeated_native_context_preserves_its_admission_boundary |
| clear_admitted | test result: FAILED. 9 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s | missing_observations_do_not_authorize_clearing_an_admitted_context |
| classify_admission | test result: FAILED. 7 passed; 3 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s | admission_watermark_distinguishes_old_history_from_new_context_work |
| restored | test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s | none |

Each mutation starts from pristine source, records its exact unified diff and mutated hash, compiles successfully, and fails its named regression. The pristine copy is restored and hash-checked after each probe. The final restored suite passes 10/10. Root source, root tests, root HEAD and the identity rlib remained unchanged.

The exact-publication-prefix mutant intentionally reintroduces the superseded constraint and must fail the C11 prefix-growth regression. The old a4 fresh-boundary removal mutant is not reused. These results qualify the copied actual domain rules in isolation. They do not qualify application namespace tracking, codecs, lifecycle orchestration, concurrency, integration, E2E, or the full crate/build selection. Compilation warnings and all run output are retained beside the manifest.

The first baseline compile using the older c80 SDK artifact failed with E0460 because its local-database dependency metadata was stale. No mutant ran in that attempt. Its compile log and script are retained as `attempt1-stale-sdk.*`. A copied existing b468 SDK artifact compiled successfully against the available dependencies; its baseline, all five mutations and restored baseline are the results above.
