# Measured outcome · 2026-10-23

Base: `0b8019cf45f99b285f10c41199875d474492c6d1` (main checkout before experiment).
Branch: `codex/1089-exp-browser`. Prototype files are under
`scripts/browser-experiment/`; the isolated qualification workflow is
`.github/workflows/browser-experiment.yml`.

| Check | Observed result |
| --- | --- |
| Non-root identity | uid 1000, user agent |
| Node / Playwright | v24.19.0 / 1.64.1 |
| System Chromium | 151.0.7922.173, Debian 13 |
| Unit + real HTTP tests | 14 tests passed, 0 failed; see node-tests.txt for duration |
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
  and profile-retention rules made tests fail in ten disposable-copy probes; the
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

## Explicitly not measured

All browser persistence, profile separation, viewer same-context, extension,
navigation/download and browser cleanup assertions are implemented in the verifier
but **did not run past launch**. No browser screenshot or video was produced. No
browser startup success time, frame/input latency, browser RSS/CPU, bandwidth,
WebKit/macOS/Windows, cloud deployment, real OAuth or passkey behavior was measured.
The harness records successful startup time and 20 authenticated JPEG request
latencies/sizes on a supported host; that does not yet measure human input-to-paint
or production frame budgets. Browser-suite failures are release blockers for any
claim of a working browser integration.

Next action: provision an ordinary non-root sandbox-capable test desktop/worker,
then run `npm run verify` with the matching Playwright Chromium and retain its
rendered sandbox status, screenshot and measurements. No privilege escalation was
requested to alter this environment's browser sandbox.

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
