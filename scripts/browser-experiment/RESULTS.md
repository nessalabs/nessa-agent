# Measured outcome · 2026-10-23

## Supported-runner qualification, first run

[Run 39920161218](https://github.com/nessalabs/nessa-agent/actions/runs/39920161218)
executed at exact source `5a3f9f6d9d354929aba4fd35587d17d815560fb7`.
Ubuntu 22.04.5, non-root uid 1001, Node 24.21.0, Playwright 1.64.1 and bundled
Chromium 156.0.8078.4 ran headful under Xvfb with `chromiumSandbox:true`.
No host security setting was changed. Four browser checks passed:

- Rendered sandbox status, including the verifier's positive evaluation and
  namespace/SUID plus seccomp checks.
- Toy MV3 content script, runtime messaging, service worker and storage behavior.
- Agent API and viewer pointer controls changing the same page.
- Navigation, test-download bytes and popup context sharing.

Startup measured **1465.04 ms**. The run failed after **2659.95 ms**: after the
intended unavailable navigation rejected, its delayed `chrome-error://chromewebdata/`
commit interrupted recovery to the local fixture. The verifier retained that
original error and reported test-profile cleanup confirmed. All 14 then-existing
Node tests passed on the runner. Remaining browser checks did not run: repeated
failure recovery, competing profile ownership, frame measurements, persistence
across restart, second-profile separation and disconnect fencing.

[Job-log-derived result JSON](evidence/run39920161218-results.from-job-log.json)
preserves the exact emitted browser report and provenance. The
[artifact](https://github.com/nessalabs/nessa-agent/actions/runs/39920161218/artifacts/12487056900)
contains four files (183666 compressed bytes), including results, rendered sandbox
text and a real viewer screenshot. Its GitHub download tool succeeded, but the
execution environment's attempt to fetch the returned artifact bytes failed with
`Tunnel connection failed: 403 Forbidden`. Byte retrieval is **denied**, not
completed; no alternate route was attempted. The screenshot has not been visually
inspected here.

The repair observes Chromium's new-document and lifecycle events through the existing
private Playwright pipe before navigation. If goto fails, it waits for a new root
loader matching the requested/error URL and that same loader's DOM readiness before
releasing queued commands. Same-document hash changes, child frames, the previous
loader and unrelated readiness events do not satisfy the barrier.
If settlement fails, it preserves both errors and closes local command admission;
it does not retry navigation. A controlled ordering regression proves recovery
cannot start before either barrier. The browser scenario now repeats error/recovery
five times and checks the original fixture URL and storage afterward. **18 local
Node tests pass**; the repair still requires supported-runner qualification.

## Original local environment failure (preserved)

Base: `0b8019cf45f99b285f10c41199875d474492c6d1` (main checkout before experiment).
Branch: `codex/1089-exp-browser`. Prototype files are under
`scripts/browser-experiment/`; the isolated qualification workflow is
`.github/workflows/browser-experiment.yml`.

| Check | Observed result |
| --- | --- |
| Non-root identity | uid 1000, user agent |
| Node / Playwright | v24.19.0 / 1.64.1 |
| System Chromium | 151.0.7922.173, Debian 13 |
| Current unit + real HTTP tests | 18 tests passed, 0 failed; see node-tests.txt for duration |
| Real headful launch | Blocked, `chromiumSandbox:true`, `DISPLAY=:89` |
| Failure measurement | 102.10 ms until launch refusal; this is not successful startup latency |
| Sandbox helper | `/usr/lib/chromium/chrome-sandbox`, mode 4755, owned by nobody, not root |
| User namespaces | `unshare -Ur true` failed: cannot open `/proc/self/uid_map`: read-only filesystem |
| CDP transport in launch log | `--remote-debugging-pipe`, no debugging TCP port |
| Profile cleanup | Verifier's temporary profile/download tree removed after refused launch |
| Process cleanup limit | Two diagnostic launches left four defunct Chromium/crashpad processes under PID 1; no live browser remained. Owned diagnostic Xorg was stopped. A reaping init is required on a future worker. |
| Project board | `gh api graphql` lookup returned `Forbidden`; status update was not attempted after the denied read |

The browser's decisive diagnostic was:

```text
The SUID sandbox helper binary was found, but is not configured correctly.
Rather than run without sandboxing I'm aborting now.
... /usr/lib/chromium/chrome-sandbox is owned by root and has mode 4755.
```

No sandbox fallback, root browser, alternate sandbox workaround, personal profile,
real credential, third-party extension, merge or deployment was used. npm package
installation succeeded using a writable `/tmp` cache after the default home cache
could not be created. No browser binary download was attempted. The private GitHub
GraphQL access denial was not retried using another route.

## Evidence and commands

- [Browser diagnostic JSON](evidence/browser-results.json): actual launch flags,
  elapsed failure time, fatal error and test-tree cleanup.
- [Node test output](evidence/node-tests.txt): HTTP/lifecycle tests, no browser substitute
  represented as actual Chromium.
- [Mutation probe output](evidence/mutation-probes.txt): changing queue capacity,
  queued-close fencing, origin rejection, credential format, typed input or failure
  profile-retention and navigation-settlement rules made tests fail in twelve disposable-copy probes; the
  restored copy passed. No live browser sandbox setting was changed.
- [README](README.md): runnable demo instructions, architecture, engine comparison,
  lifecycle contract, security/auth boundaries and next measurements.

```sh
npm test --prefix scripts/browser-experiment
EXPERIMENT_CHROMIUM=/usr/bin/chromium DISPLAY=:89 npm run verify --prefix scripts/browser-experiment
```

The second command exited 2 with `result: blocked` and an empty browser-test list.
That diagnostic was captured before the review fixes; sandbox launch options are
unchanged and the browser suite was not rerun after the sandbox refusal. The final
Node tests cover the subsequent cleanup and verifier repairs.
The X display was a temporary diagnostic Xorg using the preinstalled dummy driver,
not a newly installed service. It has been stopped. Unit tests are reproducible
without a display; browser verification needs a host with a functioning sandbox.

## Local limitations and checks not reached in the first runner run

The local run did not pass launch. The runner independently established the four
checks listed above and produced its own screenshot, while restart persistence,
profile separation and later assertions remain unverified. No video, frame/input
latency, browser RSS/CPU, bandwidth, WebKit/macOS/Windows, cloud deployment, real
OAuth or passkey behavior was measured.
The harness records successful startup time and 20 authenticated JPEG request
latencies/sizes on a supported host; that does not yet measure human input-to-paint
or production frame budgets. Browser-suite failures are release blockers for any
claim of a working browser integration.

Next action: push the recovery repair to the experimental branch for the same
supported-runner workflow, then retain that run's results separately from the
original failure. No privilege escalation was requested to alter this local
environment's browser sandbox.

## Review scope and dispositions

Independent reviews inspected the HTTP credential/origin/host boundaries, ordered
commands, close fencing, partial startup cleanup, verifier assertions and claims.
It independently reproduced the original five passing Node tests. Its four findings
were fixed before handoff:

- Use current Chromium `Layer 1 Sandbox` table rows and positive evaluation rather
  than obsolete labels. A pure fixture test covers namespace/SUID, absent rows,
  disabled seccomp and a negative evaluation. The rendered page remains unverified.
- Close an already constructed browser if viewer startup fails, retaining the
  primary failure and any cleanup failure. Startup and idempotent shutdown tests
  cover the failure order and later cleanup attempts.
- Require a string command discriminator before property lookup. Authenticated
  malformed JSON now receives `invalid_command`, covered by the real HTTP test.
- Retain and close an unexpectedly successful competing context before failing
  the ownership test. That real-browser scenario remains blocked.

The second review found an adjacent cleanup gap: a failed context close could erase
the original initialization error, and verifier teardown could delete the profile
before browser closure was confirmed and omit the report. Cleanup now has one shared
owner. Substitute regressions preserve nested primary/cleanup errors and an explicit
cleanup recovery handle; a real-filesystem regression proves an unconfirmed close
retains the test tree and still writes both failures. No automatic relaunch or
implicit retry of an uncertain initialization is added. The README also distinguishes
the configured connection limit from tested body limits.

The follow-up review found the same retention requirement on an unexpectedly
successful secondary context. Its handle now stays owned until close confirms;
the verifier refuses profile removal when any such resource remains unconfirmed.
A regression covers that case even when ordinary cleanup succeeds. Navigation
targets also require a string before lookup and have a malformed-target regression.

Local checks: repository Prettier configuration passed, JavaScript modules passed
`node --check`, `git diff --cached --check` passed, and actionlint 1.7.12 accepted
the isolated workflow. Existing application suites were not run because this
experiment does not change their runtime or dependency graph.

No production runtime state page changed, and no result from a previous source
revision was represented as validation of this implementation.
