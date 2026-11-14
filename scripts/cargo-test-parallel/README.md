# Cargo test execution in CI

`../cargo-test-parallel.mjs` owns the combined Cargo build, artifact discovery,
bounded executable pool, and doctest execution. Its adjacent root-level unit
tests cover discovery and injected failures. `e2e.test.mjs` uses a disposable,
dependency-free workspace to exercise real Cargo/libtest/doctest processes and
the required-check aggregate; `gateway-contract` runs it with stable Rust.

`--run-packages "-p a -p b"` narrows which executables run, never what is
built: the build and the doctests keep the full package arguments, so Cargo
unifies features exactly as a full run does. The Linux leg passes the crates
`scripts/ci-plan.mjs` says a pull request can reach; an empty value runs all.

The Windows jobs build the same declared package selection with the same flags
(`RUST_TEST_PACKAGES` in `../ci-plan.mjs`). The
`server-conversation` and `server-other` partitions resolve `nessa-server` and
its library target from `cargo metadata`, match the artifact's package ID,
target name and kind, and require exactly one library executable. The four seeded paired-catalogue
cases run together in the existing `remaining` job, alongside the other
executables and all doctests. The two server jobs run complementary cases
on separate Windows runners: names containing `conversation::`, and all other
ordinary names. All three jobs discover the complete server registry and validate all three actual
libtest filters before executing anything. Coverage is disjoint by artifact
identity and case name, not executable: the same server executable is invoked
with three complementary filters across the jobs. Linux/macOS use `all`, unchanged.
Unit and real process tests assert the case union and intersection; workflow-contract
tests hold the build arguments, platform coverage, guards and aggregate needs.

The shared [process failure diagnostics](../process-failure/README.md) own
streaming output and bounded failure annotations. This runner uses that owner
on GitHub Actions; command selection and execution remain here. The real-Cargo
`e2e.test.mjs` recovers an actual synthetic assertion from those annotations
without changing the failed process result.


The Windows server jobs use [default libtest capture](https://doc.rust-lang.org/book/ch11-02-running-tests.html#showing-function-output),
as Linux/macOS do. Failed-test output is printed in the final failure section;
successful-test output stays captured. Disabling capture lets later successful
output push an earlier assertion outside the retained tail. The real-Cargo
`default libtest capture` regression proves that distinction with the same
one-passed/one-failed result in both modes. Only this disposable ordering fixture
uses one test thread; the production job's thread and process concurrency, package
selection, features, 90-minute step and 95-minute job bounds are unchanged.

| Situation | Reporting limit |
| --- | --- |
| Tests finish with failures | Their output is printed in libtest's final failure section |
| A later test hangs or the step is canceled | Earlier panic details may stay captured; long-running notices and the unchanged deadline remain |
| Failure output exceeds the retained tail or annotation capacity | Assertion details can still be absent; no complete-history or delivery guarantee |

Restoring capture is a reporting-mode change. It does not diagnose or repair a
handoff failure, and a later pass in that mode does not explain an earlier failure.

| Result | Next action |
| --- | --- |
| Build fails or emits no executable | Fail before running tests |
| Split discovery is missing or ambiguous | Fail before running tests |
| Registry is malformed, duplicated, incomplete, or lacks any of the four seeded cases | Fail before running tests |
| Any actual libtest filter disagrees with its assigned cases | Fail before running tests or doctests |
| Executable fails | Await active workers; fail without starting doctests |
| All selected executables pass in either server partition | Succeed; doctests belong to `remaining` |
| All selected executables pass in `all` or `remaining` | Run all selected packages' doctests; propagate their status |
| Any Windows job fails or unexpectedly skips | `Required checks passed` fails |
| Documentation-only classification explicitly says false | All Windows jobs may skip |

This duplicates compilation to shorten the Windows critical path. The native matrix remains the sole main-only
cache writer; the two server jobs read that same `local-auth` key without racing a save.
Dispatch's cold mode disables restoration in both server jobs. Warm PR runs restore
main's cache independently; the second server runner adds billed time and a cold run
can cost substantially more. Timing comparisons must name the runs, cache mode,
queue time and other platform bottlenecks; preliminary estimates are not results.

The Windows server steps have a 90-minute deadline: the step compiles before
it tests, and a change to `Cargo.lock` misses the cache and compiles the graph
cold (about 25 minutes). Run 41947356093 was still inside server-other's tests
when a 70-minute deadline fired, with 2283 of 2349 passed. A warm run ends in
under 35 minutes. The redistribution retains the inherited
libtest thread settings. The server library's two ordinary selections use
separate runners, so their process-local fixtures and CPU budgets are isolated;
current-head CI checks for effects through shared OS resources.
All four seeded cases stay in one process; the SDK ACP artifact is
also unsplit, preserving its process-local two-slot semaphore. Run 40639717491
spent about 15 minutes compiling, then its two server processes contended on
one Windows runner for another 29 minutes. The conversation selection had one
five-second readiness fixture expire under load; the other selection was still
running when the 45-minute step deadline fired. The separate runners and the
shared creation hang ceiling address those observed failures;
current-head Windows CI must establish the actual reduction before it is claimed.
