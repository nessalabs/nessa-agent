# Process failure diagnostics

`../process-failure.mjs` owns the shared streaming process adapter and bounded
GitHub failure annotations. The Cargo test runner consumes that adapter and
decorator through `runProcess`, the one owner of CI-only activation; the existing
desktop CI step invokes its narrow CLI around
`node scripts/check-desktop.mjs`. The checker still owns its stages, arguments,
assertions and deadlines. This wrapper does not choose tests or retry them.

On GitHub Actions, `../process-failure.mjs` retains the existing last 16,384 output
characters from each failed child and forwards the original chunks unchanged.
It reports that retained tail in escaped error annotations, each at most 3500
UTF-16 units including its header. The bounded parts split on Unicode code
point boundaries. The invocation and rejection previews each retain at most
1024 UTF-16 units; a longer preview names its original length and SHA-256. These
preview bounds do not crowd out the captured output tail or change the child's
command, argument references, exit result or rejected error.

| Event | Reporting action | Execution outcome |
| --- | --- | --- |
| Output arrives | Retain the bounded tail and forward the original chunk | Unchanged |
| Child succeeds | Emit nothing, including for failure-shaped output | Return the same result |
| Child fails | Emit correlated chunks, final part first | Return the same result |
| Child rejects | Emit correlated chunks, final part first | Rethrow the same rejection |
| Formatting or writing the report fails | Stop reporting | Preserve the child's failure |

Every part names its natural index, total and the SHA-256 of the retained
report's UTF-8 representation. Reconstruct only after receiving every index once,
with one count and hash, and verifying that hash over the ordered content. The
final part is emitted first so libtest's final failure section has priority.
Content hashes correlate report parts, not process executions: identical reports
cannot establish two distinct executions. No test output is interpreted to decide
success, retry, selection, concurrency or a deadline.

GitHub's runner caps each message at 4096 characters and retains ten errors per
step ([runner implementation](https://github.com/actions/runner/blob/main/src/Runner.Worker/ExecutionContext.cs)).
Several failed processes can therefore exhaust the step's annotation allowance.
Cancellation, missing parts and GitHub masking can also leave incomplete evidence;
a changed hash is incomplete evidence, not a reason to recover masked text. No
annotation delivery or complete process-history guarantee is made. An assertion
outside the retained tail is not captured. The full prefixed output remains in
the normal log. Reporting failures cannot turn a failed child into a success.

`../process-failure.test.mjs` simulates the 4096-character receiver, verifies complete
tail reconstruction, bounded metadata, Unicode and command escaping, and rejects
incomplete or conflicting parts. `../cargo-test-parallel/e2e.test.mjs` puts 40,000 context characters
before a real synthetic server-library assertion and recovers it from the bounded
annotations while the runner still exits 1 for libtest's exit 101. It preserves
all partition, build, harness, doctest and required-check assertions.


## CLI execution contract

```sh
node scripts/process-failure.mjs node scripts/check-desktop.mjs
```

| Event | Execution and reporting |
| --- | --- |
| CLI receives an executable and argument array | Spawn directly, without a shell, in the current directory with the inherited environment; stream UTF-8 stdout and stderr |
| Child closes with a numeric exit | Return that same exit after output drains; only failures on `GITHUB_ACTIONS=true` emit the bounded report |
| Spawn rejects | Preserve the rejection in the decorator; the CLI prints its message and exits 1 |
| Child closes without a numeric exit, including a signal | The existing adapter returns 1; no signal transparency is claimed |
| No executable is supplied | Print usage and exit 1 without spawning |
| Parent is canceled | No new cancellation, timeout or child-lifetime policy is introduced |

`../process-failure.test.mjs` includes portable actual-process CLI regressions.
The desktop checker includes them in its existing test list so they execute on
Windows as well as the frontend check. The workflow test pins this one wrapper
around the existing checker; it does not add a job or replace the checker.
