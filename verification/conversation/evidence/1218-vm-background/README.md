# VM background commands (#1218)

Work is local on `codex/1218-vm-background`, based on main
`2b42b4199c2c0692b454746249ca7b564be2b5ad`, in
`/workspace/nessa-agent-1218`. No push, PR, merge, deployment, paid Boat operation,
or real credential creation is part of this task.

## Coordination

The original checkout remains on `work` at
`49b65a9e17aa239d778818d251154eaddc2a7c94`, clean and untouched.
The implementation builds on merged Devices/idle work #1219 and VM lifetime
configuration #1168/#1174; neither is reimplemented.

Active PR diffs inspected:

| PR | Head inspected | Relationship |
| --- | --- | --- |
| #1205 | `fe52ae5a0f5db01dba51c32eadff162916291a15` | Desktop verification; no background execution hunks |
| #1266 | `f0764e15b08f5a118f8401b680415fdec9531e94` | Composer attachments; no background execution hunks |
| #1267 | `b57208b162eaf4dff1f6fb42cc5bad722324e7d4` | Shared CLI/composition/workspace files, distinct additive paths |

For #1267, this change adds a command variant/dispatch arm, retains the
`ThreadWorkspaces` handle at composition, adds an independent SSH session, and
extends the workspace runtime port. It does not change login grants, consent,
Cedar policy, credential distribution, lease frames, or lease configuration.
The CLI grant/ungrant hunks, saved-login composition and workspace lease-login
wiring remain owned by that PR. No necessary behavior/hunk overlap was found.
No excluded issue or another agent's branch was changed.

## Execution and ownership

The existing native-call authorization and durable admission run first.
The admitted lease supplies a typed local/VM target and the VM's reported cwd;
an unsupported placement or missing VM cwd cannot fall back to the gateway.
The saved conversation identity and immutable placement identify the task's
machine without a second persisted location column.

The VM runner opens only the existing workspace (`provision = false`) through
the same channel and allocation checks as an agent connection. Its SSH stream
holds the runtime's `InUse` token. The verified build installer supplies the
helper, including when an older VM image speaks the same lease protocol but
lacks the helper. The independent helper does not change the lease fingerprint
or worker-image pin.

On stdin EOF or a cancellation byte, the helper asks Shepherd to cancel and
waits for cleanup. A terminal record includes the bounded output tail and
physical exit/cleanup evidence. The gateway bounds and validates the record;
transport loss, malformed evidence, or a deadline without an outcome settles
interrupted, retaining unconfirmed cleanup. It does not replay or run locally.

The original background owner blocks idle parking through verified settlement/delivery.
The `InUse` guard separately blocks a direct workspace rest. Existing recovery
settles a saved nonterminal task interrupted without executing it.

## Regression coverage

| Contract | Regression |
| --- | --- |
| Running VM task survives repeated sweeps past idle; completion allows rest; loss and restart remain interrupted | `vm_background_stress_holds_past_idle_then_rests_and_restart_does_not_replay` |
| SSH/other remote placements and absent reported VM cwd launch nothing | `background_remote_placements_and_missing_vm_cwd_fail_before_runner_effects` |
| Stream owns `InUse` after the runtime handle is dropped | `a_background_stream_holds_the_vm_after_its_runtime_is_dropped` |
| SSH EOF cleans a real Shepherd descendant before reporting | `ssh_eof_cleans_the_shepherd_scope_before_reporting` |
| Closed input before first poll launches nothing | `a_connection_already_closed_does_not_start_a_command` |
| Cancellation remains pending until cleanup answers | `cancel_keeps_the_reader_pending_until_cleanup_answers` |
| Exit code, separate stdout/stderr, timeout and bounded output cross the stream | `bounded_output_exit_and_timeout_cross_the_stream` |
| Missing, malformed, oversized and contradictory outcomes remain interrupted | `missing_malformed_oversized_and_inconsistent_outcomes_are_interrupted` |
| Shell arguments preserve quoting, substitutions, trailing newlines and stdin | `background_arguments_survive_the_login_shell_without_interpretation_or_newline_loss` |
| Pipe/socket descriptor I/O, EOF and backpressure | `background_input_reads_a_pipe_and_its_eof`, `background_input_reads_a_socket_and_its_eof`, `background_output_waits_for_socket_backpressure` |
| Shared socket stdio preserves a large JSON-escaped output record while the reader waits | `background_run_preserves_a_large_record_on_shared_socket_stdio` |
| Actual CLI helper exits with stdin open, clears a synthetic secret and prints machine-readable outcome | `background_run_exits_with_stdin_open_and_clears_agent_secrets` |

The fake-VM stress test defaults to three repetitions. Set
`NESSA_VM_BACKGROUND_STRESS_REPEATS` to an integer from 1 through 100 for a
raised run. Its provider, workspace and process runner are substitutes; no Boat
account is consulted. The existing desktop `background` stress step continues
to own the shared MCP/tool-row/continuation UI path.

## Validation

Source commit: `9f10c41087f6602abae76168efd312e8574a4d6c`. All 31 changed source/test files matched the
[verified byte manifest](verified-sources.json) before the commit. No implementation
bytes changed during the final checks. Toolchain: rustc 1.99.1, cargo 1.99.1,
Linux x86_64, using this worktree's own target directory.

| Check | Result |
| --- | --- |
| Server `--lib background` | 36 passed |
| Fake VM stress, raised to 10 repetitions | passed |
| Server `--lib workspaces::` | 85 passed |
| Server `--lib ssh_environment::` | 128 passed, 1 opt-in real-host test ignored |
| Server `--lib cli::` | 31 passed |
| Real `background_run_binary` | 2 passed |
| Server clippy `--all-targets -- -D warnings` | passed |
| CI package selection clippy, all targets, `-D warnings` | passed |
| `cargo fmt --all -- --check` | passed |
| Architecture checker and its 108 unit tests | passed |
| Desktop stress-plan tests | 14 passed |

[Command receipts](checks.json) give the exact argument arrays, repetition
variable and exit statuses. The CI package selection was the command in
`.github/workflows/local-auth.yml`: `nessa-local-storage`, `nessa-auth`,
`nessa-server`, `nessa-protocol`, `nessa-client-core`, `nessa-device-client`,
`nessa-sdk`, `nessa-timer`, and `nessa-vault`, with all targets and warnings denied.
[Its log](ci-selection-clippy.log) confirms success. The compiler emitted the
existing long constant-evaluation notice for the lease fingerprint; both clippy
commands exited zero. None of that fingerprint's source files changed, and its
shipping-worker contract passed in the workspace suite.

The ignored case was `first_use_over_real_ssh`, which requires
`NESSA_SSH_FIRST_USE_HOSTS` and `NESSA_SSH_FIRST_USE_BUILD`. No such hosts or
credentials were supplied. Full CI execution, the minimum Rust version, other
OS targets, real Boat/SSH and the browser stress driver were not run.

### Revert evidence

[Six probe receipts](revert-probes/results.json) record the source digest at
each probe and the expected test failure. Each [mutation patch](revert-probes/)
was checked to have landed, tested separately, and restored byte for byte with
a fresh modification time. The final passing checks ran after restoration.

1. Removing VM admission restored `UnsupportedEnvironment` in the VM stress test.
2. Removing the stream's `InUse` hold let a direct rest stop the VM too early.
3. Removing EOF/stop cancellation left the reader pending instead of receiving
   cleanup evidence; the gated regression failed at its bounded wait.
4. Removing lifecycle validation accepted contradictory cleanup evidence.
5. Removing shell argument sentinels lost trailing newlines.
6. Reverting output to Tokio's buffered stdout exited before a blocked large
   record was drained; the real shared-socket CLI regression failed.

The real CLI also found a separate shutdown defect during implementation:
Tokio's blocking stdin read held the runtime open after the command ended. The
[before-fix test](stdin-before-fix.log) timed out at 20 seconds. A nonblocking
descriptor adapter fixed it, and the final pipe CLI test passed with stdin still
open. A [manual shared-socket reproduction](socket-before-fix.log) then observed
an exit-zero helper with only 8,064 bytes of a truncated JSON record. The same
readiness adapter now handles both input and output, including write
backpressure. Descriptor-level pipe/socket/EOF tests and both actual CLI cases
pass. The helper drains its supervisor before leaving.

There are no remaining failed checks on the final source. Exploratory failures
in earlier test fixtures (placement spelling, PID-file observation, a retained
live handle, stale provider notification, and duplicate synthetic native-call
identity) were corrected before the passing final run.

## Scope and limits

Output is retained and transferred with the final record; the background API
does not publish interim output. Provider-owned shells outside `background.start`
are not tracked. Real Boat/SSH connectivity, paid VM lifecycle effects,
macOS/Windows execution, and a browser VM stress run are not claimed.
No browser UI code changed. No external agent or independent review service
was contacted. Existing TLA+ files and their historical receipts were not changed
or rerun; the new transport is outside that model's stated abstraction.

Unconfirmed cleanup retains the existing failed owner for joining, so a lost
connection can keep the cloud thread from idle rest. This change preserves that
ownership rule and does not add automatic reconciliation or claim verified
release from a transport failure.
