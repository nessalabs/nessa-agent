# Linux container process-cleanup acceptance

This opt-in harness demonstrates the existing SDK cleanup contract under a
non-reaping Linux PID 1 and under Docker `--init`. Linux deployments need an
init that reaps adopted descendants (`docker run --init`, or `tini -s --`).
The [canonical cleanup state](../../docs/state/services/sdk/runtime/stop-cancels-owned-work-and-confirms-process-cleanup.md#linux-container-acceptance-630)
owns the ordering table. There is no production subreaper change or CI job.

## Module map

- `Dockerfile` pins the Debian base and installs only the runtime prerequisites.
- `container-init.py` waits for its direct test child, then inspects `/proc` and
  container-local private directories while the supervisor remains alive.
- `check-container.mjs` owns scenario selection, acceptance validation, deadlines,
  read-only input mounts, unique container names and removal in `finally`.
- `check-container.test.mjs` tests acceptance failures and Docker orchestration
  using a substitute runner; the four real container runs remain necessary.

## Build the image

Docker, BuildKit, Node and an already compiled Linux SDK **library test** executable
are required. Preserve Docker registry credentials and proxy defaults. In a managed
cloud environment select its local socket and supply its proxy/CA configuration:

```bash
env -u DOCKER_HOST -u DOCKER_CONTEXT -u DOCKER_TLS \
  -u DOCKER_TLS_VERIFY -u DOCKER_CERT_PATH \
  docker --host=unix:///var/run/docker.sock build \
  --build-arg HTTP_PROXY --build-arg HTTPS_PROXY --build-arg NO_PROXY \
  --secret id=proxy_ca,src="$CODEX_PROXY_CERT" \
  --secret id=system_ca,src=/etc/ssl/certs/ca-certificates.crt \
  -t nessa-process-cleanup:630 scripts/process-cleanup
```

The secret CA bundles are combined only within the installing build step. TLS
verification stays enabled; the session CA is not installed in the image.
Outside that environment use your trusted public CA bundle for both secret inputs
and your configured proxy arguments. No network access is used by the tests.

## Select and run the explicit artifact

The harness never builds or chooses a binary from a glob or file timestamp. To
obtain the exact artifact, compile with Cargo JSON output (this compiles without
running the cleanup tests on the host):

```bash
cargo test -p nessa-sdk --lib --no-run --message-format=json > /tmp/sdk-artifacts.jsonl
```

Read the `compiler-artifact` whose `target.name` is `nessa_sdk`, `target.kind`
contains `lib`, `profile.test` is true and `executable` is non-null. Pass that
exact `executable` value, and the absolute crate directory used for this compile.
The fixture uses embedded `CARGO_MANIFEST_DIR`; that directory must match the
binary, even if its source is in another worktree. The harness mounts its fixture
folder read-only at that exact path. It mounts the binary read-only at
`/probe/nessa-sdk-tests`. The image must support that binary's architecture and
shared libraries (`libgcc_s`, `libm`, `libc` and the loader).

```bash
node --test scripts/process-cleanup/*.test.mjs
node scripts/process-cleanup/check-container.mjs \
  /absolute/path/to/exact/nessa_sdk-library-test \
  /absolute/compiled/crates/nessa-sdk \
  nessa-process-cleanup:630 /tmp/nessa-cleanup-evidence
```

Each of the two exact tests runs once per PID 1 configuration, in a fresh
container, with `--exact --nocapture --test-threads=1`. The negative needs a
nonzero test exit, exactly one failed test, `CleanupUncertain` as the selected test's libtest
panic cause (its panic header followed by the unwrap error) and a new PPID-1 zombie; the directory test also needs retained
`/tmp/nessa-agent-*` directories. A different failure, timeout or missing zombie
fails the harness. Both `--init` cases must pass and leave no new orphan zombie
or retained private directory. The ACP fixture's `ignore-stop` mode ignores
TERM and creates its child before emitting `running`.

The Python supervisor bounds each test at 20 seconds, waits one second for init
reaping, captures evidence, and remains alive one more second. Docker operations
have a 30-second outer deadline; removal has an independent 10-second deadline.
SIGINT/SIGTERM cancel active work and trigger forced removal. Removal failure
fails the harness. Intentionally unreaped zombies exist only in the disposable
container PID namespace. Containers have a writable private `/tmp`, no writable
host mounts, a read-only root and no network.

Supervisor proof fields must be explicit: timeout/truncation booleans, process
identities and states, retained private-directory paths and prerequisite package
versions. Initial, test and adopted-process identities must agree; incomplete or
contradictory evidence fails acceptance. Image inspection receives the same
interrupt cancellation as container creation and execution. `publishAcceptance`
checks cancellation before and after its record write; cancellation observed
during independent removal or publication leaves rejected diagnostics and fails
the harness.

Libtest output is limited to 64K characters and Docker stdout/stderr to 128 KiB each after incremental UTF-8 decoding;
malformed bytes count by their replacement-character UTF-8 cost. Both streams
use the same bounded prefix capture owner;
truncation fails acceptance. Rejected JSON, malformed output and Docker failure
diagnostics are saved before validation/removal. Evidence-write failure still
triggers removal.

`acceptance.json` records the selected image ID/reference, binary SHA-256,
manifest directory, exact package versions and each accepted scenario's output,
exit code, process identities/states and retained directories. A successful harness exit and four entries with `accepted: true` establish
acceptance. A partial report, rejected entry or nonzero harness exit does not. Libtest's debug output is used only by this
developer acceptance probe; SDK production decisions retain typed failures.

## Recorded acceptance

On 2026-10-07, the checkout based on main
`cffdabba64f821c012b2df04c2f02e5754511df3` passed all four scenarios. The image ID
was `sha256:c30a858151fd1f110f5f1639372775425d3ed5602728f97bc288d5d1df273e4a`.
Installed prerequisites were `python3-minimal=3.13.5-1`, `libgcc-s1=14.2.0-19`
and `ca-certificates=20250419`. Cargo JSON selected library test executable
`nessa_sdk-3a9a37445fc631f7`, compiled from this worktree's SDK directory, with
SHA-256 `932566e1324fb80f9e30d0512e823d04d6c97b375af3a565306e39b88dd6c28d`.
Final acceptance used the explicit preserved copy
`/tmp/nessa-630-main-base-library-test` with that same hash.

| Test | PID 1 | Test exit | New orphan zombie | Retained private directory | Acceptance |
| --- | --- | --- | --- | --- | --- |
| Private directory cleanup | Python supervisor | 101 (`CleanupUncertain`) | PID 10, PPID 1, PGID 9, state Z | `/tmp/nessa-agent-RiEcJQ` | Passed |
| Private directory cleanup | Docker init | 0 | None | None | Passed |
| TERM-resistant ACP parent and child | Python supervisor | 101 (`CleanupUncertain`) | PID 10, PPID 1, PGID 9, state Z | None | Passed |
| TERM-resistant ACP parent and child | Docker init | 0 | None | None | Passed |

The external evidence file was `/tmp/nessa-630-r3-evidence/acceptance.json`;
all four entries recorded `accepted: true`, and all disposable containers were
removed. Forty-two pure orchestration tests and the architecture checker
passed on the final correction; 74 architecture tests passed before that correction.
At pre-review head `937610ee85494b53e4591a3b3b2146118f864971`, 32 individual
acceptance/orchestration mutations caused test failures. The review correction
added 22 schema/cancellation mutations at `9364794177684a846b9071f268b5b870b198036a`
that also failed tests. The final structural correction added seven targeted
panic-cause/publication mutations at `d9ac477436605a5ac4b360d198aa7fedd830b1e0`,
which each failed tests. The output-capture correction added six targeted UTF-8
budget/decoding mutations that also failed tests. Restored files
received fresh modification times. A real-container supervisor
mutation that reaped adopted children caused the negative case to fail with
"no new adopted zombie"; restoring the direct-child-only supervisor restored
four-case acceptance.
