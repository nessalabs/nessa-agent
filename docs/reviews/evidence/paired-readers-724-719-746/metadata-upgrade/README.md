# Preserving metadata upgrade and loaded-macOS verification (#724, #746)

The owner explicitly authorized the cursor-preserving metadata6→7 conversion in
code and isolated synthetic tests. No live user database was opened or migrated.
No permissions, credentials, grant rules, Cedar source, public cursor numbering,
network/security settings or live process managers changed. The old head is
frozen, not reset. Previously exposed revision gaps and pre-upgrade floors remain;
future historical replacements no longer advance former receivers' heads.

The [read-grant design](../../../../design/read-grants.md#cursor-preserving-metadata-upgrade-724)
records transaction ordering, rollback and privacy invariants. The explicit
predecessor conversion leaves every pre-existing table row, grant journal
sequence, request receipt, attribution and catalogue incarnation unchanged.
The frozen version-6 fixture is byte-for-byte the schema at35f9d50.

## Verified regressions

- Local-database: existing12 open tests and10 new upgrade tests passed, including
  simultaneous openers, unsupported/unversioned refusal, target validation,
  typed error/panic rollback, and isolated subprocess exit before/after commit
  without Rust destructors. The recovered file is a complete predecessor or
  complete target and can reopen/upgrade.
- Five metadata regressions passed: old mid-page and completed cursor continuity,
  unchanged original table rows and receipt replay, all historical receivers,
  organization/owner isolation, directory-kind exclusion, restart without floor
  recapture, future retarget quietness, legitimate visible changes, invalid-cell
  rollback/retry and projected-head bounds. Immediate and no-op-separated
  revoke/regrant, credential replacement and no-op grant are covered.
- Loaded check unit tests:7 passed. The existing macOS job will reuse its package
  build and run the same synthetic two-device fixture10 times under4 CPU workers,
  retaining every failure and per-run log. Historical Linux median9.1s suggests
  1–3 warm minutes; a10-minute CI step caps cost without changing product timeouts.
  **This file does not claim loaded macOS acceptance.** That requires the new
  hosted head's actual job and retained artifact results.
- Root script tests:228 passed,2 skipped (230 total), before the final additional
  loader predicate/unit edge refinement; the affected loader7 tests passed after
  that refinement. Broader final-head results and independent implementation
  review belong on the PR after the candidate is frozen.

## Fail-before / pass-after probes

[migration-probes.json](migration-probes.json) records12 verified one-rule
mutations: error propagation, resulting-version validation, predecessor integrity,
downgrade refusal, historical retarget privacy, saved-cursor floor use, active
predecessor filtering, exact legacy-head capture, owner counter bound, scope key,
all historical receivers and conversation-kind selection.

[load-and-version-probes.json](load-and-version-probes.json) records12 more:
unversioned predecessor refusal, early-failure retention, incomplete ledger,
zero matched tests, worker release, repetition cost, partial-ledger persistence,
spawn failure, macOS-only guard, CI time budget, failed-run artifact retention
and reuse of the same package selection.

Each record includes the exact mutation, command, failing test output, restored
passing output, full-log hashes and source manifest. No compilation failure was
counted. Every modified/untracked source hash and the full starting diff were
restored after each mutation. A first active-predecessor probe exposed that a
no-op-only fixture did not test immediate regrant; that run is not counted. The
fixture was strengthened and the full12-probe migration round was rerun.

These are pre-commit source snapshots on base35f9d50, not self-referential final
commit claims. The second snapshot adds an unversioned-file test and refines only
the load helper after the first snapshot; production Rust is identical across
them. Final combined-tree checks/reviews and exact-head hosted CI remain separate.

The four [Cedar input hashes](unchanged-policy-sha256.json) still match the
intended main base and prior reviewed patch exactly. The earlier93410 evidence
in the parent directory remains historical; it is not relabelled as this upgrade.
