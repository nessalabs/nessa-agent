# Cargo test execution in CI

`../cargo-test-parallel.mjs` owns the combined Cargo build, artifact discovery,
bounded executable pool, and doctest execution. Its adjacent root-level unit
tests cover discovery and injected failures. `e2e.test.mjs` uses a disposable,
dependency-free workspace to exercise real Cargo/libtest/doctest processes and
the required-check aggregate; `gateway-contract` runs it with stable Rust.

The Windows jobs build the same seven packages with the same flags. The
`server-lib` partition resolves `nessa-server` and its library target from
`cargo metadata`, matches the artifact's package ID, target name and kind,
and requires exactly one library executable. `remaining` uses the complement
of that selection and owns all doctests. Linux/macOS use `all`. Unit and real
process tests assert the partition union and intersection; workflow-contract
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
| Executable fails | Await active workers; fail without starting doctests |
| All selected executables pass in `server-lib` | Succeed; doctests belong to `remaining` |
| All selected executables pass in `all` or `remaining` | Run all selected packages' doctests; propagate their status |
| Either Windows job fails or unexpectedly skips | `Required checks passed` fails |
| Documentation-only classification explicitly says false | Both Windows jobs may skip |

This duplicates compilation to shorten the Windows critical path. The matrix remains the sole main-only
cache writer; the new job reads that same `local-auth` key without racing a save.
Dispatch's cold mode disables restoration in both jobs. Warm PR runs restore
main's cache independently; the extra runner adds billed time and a cold run
can cost substantially more. Timing comparisons must name the runs, cache mode,
queue time and other platform bottlenecks; preliminary estimates are not results.
