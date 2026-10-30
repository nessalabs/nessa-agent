# Parent Stop recovery integration

**Checkpoint status: conflicts resolved; qualification incomplete; not merge-ready.**
The memory-bounded Rust retry and complete local Stop E2E rerun are still pending
at this publication checkpoint. The unmatched shipping image guard is retained.

Latest source reviewed: `85f057b2f9947d1e54dcc596922665424dfd443a`, adding main
`833896496b5eed420f71c160c72f5388a0caa846` after the initial integration below.
The follow-up merge retains both main's `promptImages` and the parent Stop
fixture's `ScriptedUsageLimitError`. Its scenario tests passed 34/34, and the
current MCP/browser helper suite passed 788 with zero failures and one existing
skip (789 total). A fresh independent review found no issues, passed 73 tests
and an image/error/cancellation probe; see `latest-main-review.log`. Its first
scratch assertion omitted the planner's legitimate index and was corrected
without changing repository source.

The two worktrees are in the same selected environment. The original checkout
remained frozen while the latest-main merge was checked in its own Git worktree.
The complete crates/protocol trees, Cargo manifests/lock/toolchain and cargo
configuration are identical between the two source commits. Rust lint, docs and
unit/integration execution use the original checkout; frontend checks use the
latest source. This does not claim identical whole trees or current platform
qualification from historical evidence.

Initial source reviewed: `44fe82491dda5bcafe2d5cc295ab0c50112be234`, merging published
#981 `92e18448be0a865aac89a7a80ede84452242e4e2` with main
`472cf715f631d9b19552137c1cc7f309d997d38d`.

The saved environment recovered successfully. Six conflicts were resolved in the
module map, committed transcript fixture, desktop checklist, columns verifier,
stress runner and stress plan. Main's plain Needs-you rows, answers inside chat,
streaming and history paging remain. The extracted fixture now supplies main's
required `earlier: false`. Parent Stop, exact review command/ACK observation and
rendered conversation identity checks remain. No production Stop rule, shipping
image pin, protocol guard, policy, provider credential or deadline was changed by
the resolutions.

## Review

First independent review: one minor unused `readFile` import, removed; no other
findings in the integration delta. Its targeted tests passed 77/77. A fresh final
review of the committed source reported no findings and passed 43/43 independent
tests. Both reviewed stress planning, exact answer/ACK correlation, Stop
checkpoints, rendered chat identities, main's GitHub broker source admission and
credential isolation, and committed paging identity/access boundaries. Neither
review certifies all historical PR changes or provider/native execution.

## Validation

- Browser helper suite: 640 passed, zero failed, one existing skip (641 tests).
- Scripted scenario/stress construction: 34 passed, zero failures.
- Extracted browser fixture and desktop styles: 43 passed, zero failures.
- `tsc --noEmit`: passed.
- Full frontend Vitest: 5487 passed, two private-file fixture failures (5489 tests).
  Both failed again in an isolated sandbox rerun: `/` and `/tmp` are mapped to
  UID65534 while the process is UID1000. The trusted-path owner correctly
  refuses that acquisition chain. With normal filesystem ownership, both files
  passed unchanged, 39/39 tests; no validation was weakened.
- Production frontend build: passed.
- Webhook package: 44/44 passed after directing tool logs/cache into writable
  temporary paths. The first attempt could not initialize `/home/agent/.config/.wrangler`.
- Product protocol schema/fixtures/generated types: 65 tests passed.
- Repository frontend formatting and lint: passed.
- Rust formatting and session protocol generation: passed.
- Client TypeScript checking and architecture (108 tests): passed.
- Broad script suite: 480 passes, three prerequisite failures, two skips. The
  launch-order tests required missing `just`; worktree test could not find Cargo
  on this invocation's PATH. These are retained, with targeted repairs/reruns
  recorded separately. After installing the repository-pinned `just` 1.59.0 and
  supplying the prepared Rust tools on PATH, all seven launch/worktree tests
  passed unchanged, including all three prior failures.
- The original sandbox Rust compile was interrupted after the filesystem-owner
  diagnosis, with compiled dependency cache retained. Normal Rust tests were
  scheduled with normal ownership and `--no-fail-fast`; their final outcome is
  recorded below. The first full frontend pipeline stopped at its three script
  prerequisites; the remainder ran separately with the repaired tool context.
- Strict Clippy: six selected packages, all targets, `-D warnings`, passed.
- SDK API documentation: passed.
- Normal Rust tests: pending when this note was created;
  the final handoff records their actual outcome.

Earlier failed and unrun evidence in the parent directory remains historical.
In particular prior Claude 80/80 and Terra 79/79 raised runs are not relabeled as
current-source results. Custom WebKit geometry and Swift phone qualification are
not passed by these unit tests.

## Access and unrun gates

The supported environment status reports restricted, enforced policy and no
secret, runtime-variable or outbound-identity bindings. The attempted Playwright
browser install failed for `https://cdn.playwright.dev/builds/cft/153.0.8010.12/linux64/chrome-linux64.zip`
with HTTP 403 `Domain forbidden`. No alternate endpoint, network route or browser
download was attempted after that denial. The download command did not reach the WebKit installation. A later inventory
found system Chromium 151.0.7922.173, which can exercise local checks without
network access. Its Playwright launch selects `/usr/bin/chromium` through a
scratch import; repository source and assertions are unchanged. Smoke held 15/17
checks, failing overview keyboard focus in both layouts. The selected columns
geometry checks held 6/7 including visible content and retained reply/Stop in the
columns layout; sidebar Stop timed out in the existing three-second settlement
wait. These are failures, not passes or proven load effects. The initial smoke
attempt could not run while the dev server was starting. An independent local
probe saw two visible, non-inert overview rows but focus still on `body` at
+3503.6ms, then focus on the correct `retry-budget` row at +3801.2ms and still
there at +7011.8ms. The probe omits smoke's Settings sequence and supplements,
not replaces, smoke's failure. Focus/overview/filter/listener/selector code and
UI pin match main byte-for-byte; the integration's session-list gutter CSS does
not target overview nodes. No integration-specific cause or contention cause is
established; no deadline was widened. See `overview-focus-probe.json`. All are retained below.
WebKit and native Swift phone qualification remain unrun. Chromium gateway E2E
awaits the built gateway; the final handoff records the executed result. No external real-event transmission occurred.

Boat was not contacted. The earlier environment's `boat.dev` CONNECT403 denial
remains in force for this task. No alternate account, credentials, VM or snapshot
was used. Worker refresh and merge remain parent-owned.

## Precise existing-account setup request

Use the supported environment configuration workflow to restore access to
`boat.dev` and bind a private, owner-readable token file for the user's existing
Boat account. The existing opt-in verifier takes `--token-file <private path>`;
it does not define a Boat token environment-variable name. Never paste the token
into a command, comment or repository. Connect the existing Claude subscription
in that same account. The gateway's Boat client uses personal organization scope
and the dedicated `nessa` environment. Its supported credential posture passes
agent credentials only: no GitHub, arbitrary secrets, sandbox credentials,
repositories, secret files or environment contents; owner-default credential
profile, proxy AI credentials disabled. Existing incompatible environment settings
are refused rather than silently widened. This is a setup request, not evidence
that access or a binding exists.

Read-only build provenance is in `docs/adr/todo/1160-boat-workspaces.md` and
`verification/desktop/evidence/vault/phone-worker-release-receipt.json`. The alpha
image is manually published; the repository has no automatic publication recipe.
The previous runtime source is `a79f324f39c9cd58c2082206b531b438d794d438`, SDK
`33b8382d72657d176baca19797d8d146803f3d89`, Linux x86_64, stripped development
binary with dependency opt-level=0/debug=0/incremental=0, and a separate clean
runtime seed. Runtime metadata lives at `/opt/nessa/BUILD.json`. A refreshed image
needs exact integrated-source build provenance, runtime checksums and a clean
clone verification before its matching release pin can replace the old pin.

Integrated-source fingerprint calculation using the owner's ordered source list,
CR normalization, length prefixes and FNV1a still yields `91fd4c70b61e3d05`.
This is a source calculation until the compiled test/binary executes. The existing
pin remains `b6e4f48aa694ce08` / `bsh_1WtCS3oK4SWwNbdpOnBXzFPq_PwIT1aGCwnJSQHWakU`.
`shipping_worker_covers_the_compiled_lease_protocol` must fail until the matching
image is actually published and verified. Do not skip or weaken it.

After access and the matching image are restored, the repository's verifier is:

```sh
cargo build -p nessa-server --bin nessa
node scripts/workspaces/boat-e2e.mjs --token-file <private path> --evidence <private directory>
```

It provisions billable disposable VMs. Its checks include real subscription
inference, retained files across resume/restart, scoped MCP/delegation and cleanup.
Preserve owned VM IDs and require provider-confirmed absence. The parent owns that
operation and the previously approved spending scope; no such operation ran here.

## Latest source verification updates

At source85, the complete frontend suite passed **5506/5506** across428 files
with normal filesystem ownership. Current TypeScript, lint, formatting and
architecture checks passed. Current production build and client documentation
checks passed. Public protocol inputs are unchanged from the verified generator
run. Current production Chromium smoke held **17/17**, and selected production
columns geometry/retained reply/Stop checks held **7/7**, both layouts. These are
production-mode results, separate from the earlier development-mode failures;
no historical cause or full performance-budget closure is inferred.

The first current local parent-Stop E2E attempt could not run its default case
because its browser page crashed before rendering. The override case and
console held, producing2/3 held. During the same qualification period the
parallel Rust server-test compiler was SIGKILLed. Cgroup memory events increased
from0 OOM kills to3, with a16GiB limit. This supports a compilation resource
failure; the browser crash is retained without assuming its exact cause.
The six-package test command exited101 before running its normal suites.

A one-compiler-job retry runs on current source85, with default profiles, test
threads, all assertions and the worker guard retained. Browsers are kept idle
during it. The original checkout was advanced only after its frozen checks
ended, so current runtime scripts participate in the retry. The redundant
standalone gateway build was stopped; the already-built local test gateway was
retained. Its executable was copied and SHA256 matched before E2E; this is not
a published Boat artifact. The final result and browser rerun are recorded in
the handoff.
