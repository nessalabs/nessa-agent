# Test completion under slow startup and private storage

Refs #1060 and #1074. Reviewed base:
`1f91c7a1e253b13d9ed6d12fd3ed9c2e19f984fc` (current main at implementation and
publication preparation). This change is test infrastructure only.

## Behavior and scope

The [shared harness](../../scripts/rust-test-support/README.md) owns one direct
libtest child. It gives startup 60 seconds to publish entry, then starts a
separate 30-second execution watchdog. The child keeps the original test
assertions. Parent deadlines kill and wait, with captured output and the expired
phase retained. A zero-test successful exit is refused. Final readiness is read
after exit as well as during polling, because entry and exit can occur between
polls. The harness is not a process-tree supervisor.

#1074 now waits for actual channel results inside that bounded child. It retains
the private-file lock while preparation is held, Busy admission, original proof
identity/signature or original uncertain failure, released close receipt, thread
joins, reopening, one entropy call and zero network calls. A second test delays
the actual private preparation four seconds after gate release, for each original
result case. #1060's five physical-fault bodies are unchanged.

Prior follow-up inspection: unpublished `53e55f53803f1dfa5a908d75992a714abdec28f9`
was absent locally and GitHub returned no commit. Remote
`9b8f0f7cdd871d849baf27aa2ee3c9e7e5ead6c9` changes a different Shepherd capture
helper; none of that patch is included. Previously denied metadata and WebKit
operations were not retried.

## Local validation

Linux x86_64, Rust 1.99.1, isolated checkout-owned Cargo target:

- `cargo test -p nessa-device-client --lib`: **88 passed**, zero failures.
- `cargo test -p nessa-sdk --lib infrastructure::session_storage::ownership::tests`:
  **34 passed**, zero failures, 1,511 unrelated cases filtered.
- `cargo clippy -p nessa-sdk -p nessa-device-client --all-targets -- -D warnings`:
  passed.
- `cargo fmt --all -- --check`, direct shared-source rustfmt and
  `git diff --check`: passed.
- `node --test scripts/architecture/*.test.mjs`: **108 passed**; actual
  `node scripts/check-architecture.mjs`: passed. The first architecture attempt
  lacked Cargo in PATH; the corrected environment supplies the installed toolchain.
- Six independently executed standalone harness controls passed, including
  missed-poll readiness, six-second startup, child panic diagnostics, zero-test
  refusal and startup/execution hangs.

This is affected local qualification. Windows/native and exact-head hosted CI
remain separate gates. No manual workflow dispatch or retry is used.

## Fail-before and restored-after controls

[Device before](evidence/test-completion-1060-1074/device-before.log) restores
`received.recv_timeout(Duration::from_secs(3))` only. The delayed actual-private-
preparation test fails in 3.04 seconds with the original `Timeout`, captured from
the child; the parent also rejects its failed status. [Restored source](evidence/test-completion-1060-1074/device-restored.log)
passes in 8.09 seconds, executing both proof and uncertain-failure cases. Source
bytes were restored exactly before the passing run.

Four shared-helper mutations ran against an isolated byte-for-byte source copy,
not a concurrent edit of the package build. [Probe source](evidence/test-completion-1060-1074/helper-probes.py)
and [hashes/results](evidence/test-completion-1060-1074/controls.json) record:

| Mutation | Failing control | Exit |
| --- | --- | --- |
| Use the last in-flight readiness sample | Missed-poll terminal entry | 101 |
| Restore a five-second startup watchdog | Six-second startup | 101 |
| Discard captured stderr | Child panic diagnostic | 101 |
| Report execution expiry as startup | Hang phase correlation | 101 |

Each failed its intended running assertion. Restored source passed all six
controls, exit 0. These deliberate controls do not establish the original
Windows failures' cause.

## Independent review and documentation

Round one inspected both consumers, module paths, five SDK fault bodies,
result/readiness/status relationships and direct-child cleanup. It found a P2
fast-child readiness race. The terminal reread and deterministic missed-poll
control correct it. Fresh round two found no issue at any severity and
independently executed all six harness controls successfully. Both reviewers
excluded Windows/MSRV execution and full package qualification.

The Nessa Docs skill was read for completion review. No product source,
user-facing flow, authorization, persistence or SDK API changes, so statecharts
and their source checkpoints are unchanged. The shared test harness README and
codebase ownership map are updated instead. No exhaustive runtime or performance
claim follows from these tests.
