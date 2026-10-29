# Cargo test execution in CI

`../cargo-test-parallel.mjs` owns the combined Cargo build, artifact discovery,
bounded executable pool, and doctest execution. Its adjacent root-level unit
tests cover discovery and injected failures. `e2e.test.mjs` uses a disposable,
dependency-free workspace to exercise real Cargo/libtest/doctest processes and
the required-check aggregate; `gateway-contract` runs it with stable Rust.

The Windows jobs build the same eight packages with the same flags. The
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

On GitHub Actions, `diagnostics.mjs` copies the last 16,384 output characters
from each failed child into one escaped error annotation. This exposes the
executable, exit code and bounded output (including libtest's failure section)
through the checks API when full logs are unavailable. Truncation is explicit;
the full prefixed output remains in the normal log. The decorator forwards the
same command, arguments, output callbacks, result and thrown error. It adds no
retry, timeout, test filter or interpretation of whether output means success.
The real-Cargo regression asserts that a synthetic server-library assertion
appears in the annotation while the runner still exits 1 for libtest's exit 101.

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

The redistribution retains each 45-minute test-step deadline and inherited
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
