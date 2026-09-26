# Packaged runtime staging profile

This report records one macOS engineering measurement of packaged gateway runtime
staging. It establishes where this workload spent time under the conditions below.
It does not change the durability contract, implement an optimization, or establish
acceptance for issue 109.

## Result

Per-file sync was a material leaf cost in every instrumented copy. It accounted
for 86.26–90.78% of measured clone elapsed time and 81.62–84.35% of measured
forced byte-copy elapsed time. The single phase run likewise recorded 24.012 s
in file sync within a 27.498 s copy/sync/attribute phase.

The repeated runs varied too much to rank clone against byte copy. Collector-on
runs were sometimes faster than their paired collector-off runs, so the signed
collector delta may be negative and cannot be treated as precise overhead.

## Provenance and conditions

| Fact | Recorded value |
| --- | --- |
| Source checkout | `/Users/nessa/Documents/NessaLabs/nessa-agent-codex-ws3-staging-20260922` |
| Source head | `edea8fc4a2b6aea885aecc561a2713bad67fceb9` |
| Base | `71b77cad055006d433f22b0ae0a63247b2277623` |
| Test binary | `/Users/nessa/Documents/NessaLabs/nessa-agent-codex-ws3-staging-20260922/src-tauri/target/debug/deps/nessa_app-ca8c54fe6552558b` |
| Package version | `0.1.0` |
| Build | Cargo `dev`/test profile, unoptimized, debug info and debug assertions enabled; no `custom-protocol` feature |
| Build concurrency | `CARGO_BUILD_JOBS=2` |
| Build output | The checkout's real `src-tauri/target`; it was not redirected to or shared with another checkout |
| Host | Apple M5, arm64, 24 GiB RAM |
| OS | macOS 26.6, build 25G72 |
| Runtime source | `/Applications/Nessa.app/Contents/Resources/runtime` |
| Runtime fingerprint | `67892aae0c62de9cf6fc71346fb3b01a737dfd9e853f1eeab8518bc3f1d9512a` |
| Source inventory | 6,337 files, 685 directories, 0 links, 393,574,698 bytes |
| Filesystem placement | Source and destination device `16777234`; same-device staging |
| Phase destination | `/var/folders/1w/wp9n0wwd22qgrf0zkhb2fk6r0000gn/T/nessa-runtime-stage-1544a1afbdc29b2288a8b7081e76f11d763ef834ca67b618b2fddfbf5c878d30` |
| Comparison destination | `/var/folders/1w/wp9n0wwd22qgrf0zkhb2fk6r0000gn/T/nessa-runtime-stage-1038226568a777f42c0bc85d02ba8dbece101f9b2b8c376c459dc567b4440e25` |

Both measurement tests emitted the compiled checkout, compiled head, exact test
binary, package version, debug-assertion state, and build invocation. Those are
assertions embedded in the measured binary. Separately, the operator verified
that the checkout was clean at head
`edea8fc4a2b6aea885aecc561a2713bad67fceb9` before and after the run. The latter
is independent operator verification rather than a test-binary assertion.

## Checks and phase measurement

All recorded checks passed.

| Check | Result |
| --- | --- |
| Strict all-target Clippy | Passed with warnings denied; Cargo completed in 2.96 s |
| Staging tests | 16 passed, 0 failed, 2 ignored; 0.24 s test execution |
| Phase collector | 1 passed; 59.05 s test execution |
| Repeated comparison | 1 passed; 582.77 s test execution; 4 repetitions × 2 copy modes × collector on/off |

The single phase run recorded:

| Phase | Elapsed |
| --- | ---: |
| Copy, per-file sync, permissions, and extended attributes | 27.498 s |
| Full validation | 1.375 s |
| Exclusive publication | 0.006 s |
| Fresh stage, end to end | 28.640 s |
| Reuse validation | 0.930 s |

The fresh staging profile recorded 6,337 successful clone operations with no
fallbacks, 1.300 s aggregate clone time, 6,337 file syncs totaling 24.012 s,
6,337 permission operations totaling 0.153 s, 6,337 extended-attribute operations
totaling 0.600 s, and 687 directory syncs totaling 0.371 s. Full digest
validation passed with zero leaf errors.

## Repeated comparison samples

Elapsed values below are wall-clock seconds. `Order` is the collector order
within the mode/repetition pair. File-sync share is reported only when the
collector was enabled.

| Repetition | Mode | Collector | Order | Elapsed (s) | File sync (s) | File-sync share |
| ---: | --- | --- | ---: | ---: | ---: | ---: |
| 1 | clone | on | 1 | 28.517 | 24.599 | 86.26% |
| 1 | clone | off | 2 | 29.265 | — | — |
| 1 | byte copy | on | 1 | 32.962 | 27.447 | 83.27% |
| 1 | byte copy | off | 2 | 33.854 | — | — |
| 2 | byte copy | off | 1 | 34.439 | — | — |
| 2 | byte copy | on | 2 | 34.399 | 28.433 | 82.66% |
| 2 | clone | off | 1 | 38.628 | — | — |
| 2 | clone | on | 2 | 33.478 | 29.197 | 87.21% |
| 3 | clone | on | 1 | 34.252 | 29.666 | 86.61% |
| 3 | clone | off | 2 | 32.327 | — | — |
| 3 | byte copy | on | 1 | 34.408 | 29.022 | 84.35% |
| 3 | byte copy | off | 2 | 38.750 | — | — |
| 4 | byte copy | off | 1 | 37.318 | — | — |
| 4 | byte copy | on | 2 | 40.269 | 32.866 | 81.62% |
| 4 | clone | off | 1 | 40.241 | — | — |
| 4 | clone | on | 2 | 29.198 | 26.507 | 90.78% |

Collector-off clone samples were 29.265, 38.628, 32.327, and 40.241 s,
with a 35.4775 s median. Collector-off byte-copy samples were 33.854, 34.439,
38.750, and 37.318 s, with a 35.8785 s median.

### Instrumented leaf aggregates

Every instrumented sample processed 6,337 files and completed full digest
validation with zero leaf errors. Clone mode recorded 6,337 clones and zero
fallbacks. Forced byte-copy mode recorded 6,337 byte copies totaling exactly
393,574,698 bytes; its 6,337 clone attempts all took the forced fallback path.

| Rep. | Mode | Copy leaf (s) | Permissions (s) | Xattrs (s) | File sync (s) | Directory sync (s) |
| ---: | --- | ---: | ---: | ---: | ---: | ---: |
| 1 | clone | 1.742 | 0.201 | 0.622 | 24.599 | 0.262 |
| 1 | byte copy | 2.416 | 0.186 | 0.402 | 27.447 | 0.475 |
| 2 | byte copy | 3.124 | 0.189 | 0.376 | 28.433 | 0.340 |
| 2 | clone | 1.858 | 0.217 | 0.666 | 29.197 | 0.352 |
| 3 | clone | 1.781 | 0.195 | 0.652 | 29.666 | 0.833 |
| 3 | byte copy | 3.194 | 0.136 | 0.274 | 29.022 | 0.277 |
| 4 | byte copy | 3.176 | 0.223 | 0.511 | 32.866 | 0.963 |
| 4 | clone | 0.924 | 0.105 | 0.564 | 26.507 | 0.471 |

Leaf aggregates are sums of individually timed operations and may overlap other
wall-clock bookkeeping. They explain where observed time accumulated; they are
not an additive decomposition guaranteed to equal elapsed wall time.

## Reproduction

The measurement harness is not on `main`. It lives on the unmerged branch
[`codex/ws3-staging-20260922`](https://github.com/nessalabs/nessa-agent/tree/codex/ws3-staging-20260922),
whose head is the recorded source head. Run from that branch with the packaged runtime installed at the path
above and `CARGO_BUILD_JOBS=2` in the environment. The measurement binary
embedded these exact `build_invocation` values:

```sh
cargo test --locked -p nessa-app gateway::infrastructure::macos::staging::tests::measure_packaged_runtime_staging_phases -- --ignored --exact --nocapture --test-threads=1
cargo test --locked -p nessa-app gateway::infrastructure::macos::staging::tests::compare_packaged_runtime_clone_and_byte_copy_profiles -- --ignored --exact --nocapture --test-threads=1
```

The supporting checks were run as:

```sh
CARGO_BUILD_JOBS=2 cargo clippy --locked -p nessa-app --all-targets -- -D warnings
CARGO_BUILD_JOBS=2 cargo test --locked -p nessa-app gateway::infrastructure::macos::staging::tests
```

The complete run was originally captured in temporary logs named
`ws3-profile-edea-{clippy,tests,phases,comparison}.log` and a derived
`ws3-profile-edea-results.json`. Those temporary paths are not durable evidence;
the provenance, samples, aggregates, commands, and conclusions needed to
reconstruct and interpret the run are recorded here.

## Limits and interpretation

The source inventory and hash, plus earlier copies, warmed filesystem caches.
No cache flush was performed. The measurements used a debug test binary and a
temporary destination, not a shipping cold-start benchmark in Application
Support. Run order varied, and the machine was not isolated: during the repeated
comparison, the root coordinator initiated one additional worktree creation and
`pnpm` setup, causing concurrent filesystem activity.

These conditions explain why paired collector deltas range across positive and
negative values. They rule out a precise collector-overhead estimate and a
clone-versus-byte-copy performance ranking from these samples. They also mean
the medians should not be projected to user-visible startup latency.

The evidence is sufficient to say that per-file sync was a material measured
leaf cost. It does not justify weakening file or directory durability, skipping
full validation, or changing exclusive publication. No reproducible optimization
has been demonstrated.
