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
nonzero test exit, exactly one failed test, `CleanupUncertain` in the libtest
panic output and a new PPID-1 zombie; the directory test also needs retained
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

`acceptance.json` records the selected image ID/reference, binary SHA-256,
manifest directory, exact package versions and each accepted scenario's output,
exit code, process identities/states and retained directories. A partial report
is not four-scenario acceptance. Libtest's debug output is used only by this
developer acceptance probe; SDK production decisions retain typed failures.
