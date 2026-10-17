# Unix ownership fixture qualification

The authorized fresh run [38999879392](https://github.com/nessalabs/nessa-agent/actions/runs/38999879392)
at diagnostic head `2dff21e428fd9f386c3663b1195a475c7354a2ed` published bounded,
sanitized **new** failure annotations. Both Unix jobs reported the ownership child
panicking at the fixture's `try_claim(...).unwrap()` with typed private-storage
`PermissionDenied: UnsafeFile`. The two JSON files retain those new annotations.
No historical logs or alternative historical-log route were accessed.

`tempfile::tempdir()` uses mode 0777 masked by the process umask. This cloud
executor's 077 mask hid the fixture assumption. Running the same existing Unix
probe tests under 022 reproduced the refusal: **8 passed, 4 failed** before the
fix, including the same child panic (`before-umask022.log`). This reproduces the
CI failure; it does not claim to have measured the CI runner's actual umask.

The test fixture now requests 0700 **at creation** using `tempfile::Builder`.
Production ownership and private-storage code are unchanged. A new H9 negative
case deliberately constructs an owned 0755 fixture and proves typed refusal,
unchanged permissions and no lock-file creation. No live settings or service was
operated, and no guard, assertion, package selection or native exit status was
weakened. The temporary workflow wrapper/publisher/tests are removed; workflow
bytes match `ed7f350a` again.

## Local checks

Rust1.99.1, Linux x86_64; source frozen after formatting:

- Existing production-source Unix probe, with MSRV1.85 and app-lock registry
  selections: **13 pass** under each mask 000, 022, 077; strict all-target Clippy
  with `-D warnings` passes. These tests are a subset of the full selection.
- Production-source application/domain/Unix headless selection: **158 pass**
  under each mask 000, 022, 077; the original qualified `cargo clippy --tests --
  -D warnings` selection passes. Eighteen checks are new relative to the base,
  including the inert subprocess entry; they are not additive to158.
- `results.json` preserves all mask runs and an intentionally broader partial
  `--all-targets` Clippy failure: four pre-existing non-test domain lint
  expectations are unfulfilled because the harness omits composition. That
  failure is retained in `partial-all-targets-baseline-lints.log`, not relabeled
  success. No allowances or actual CI gates were changed to resolve it.
- `source-sha256.json` records the current eight qualification files. Seven
  match the initial manifest; only the Unix fixture differs. Existing mutation
  evidence remains the earlier source qualification and is not presented as a
  new native run.

The same dependency and GUI exclusions as the parent qualification apply. Full
native/package CI still needs the final head's result. Source review of the
combined change is clean, including all H1–H14 orderings, guard preservation,
umask independence and exact scaffold removal; a final evidence-only delta
review accompanies the commit. Actual legacy-to-participating transition,
two-host/updater acceptance, #562 installed launch and #109 timing remain unrun.
The original legacy-binary risk is not solved by this test correction.
