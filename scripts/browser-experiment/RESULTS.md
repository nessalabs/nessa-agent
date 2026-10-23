# Measured outcome · 2026-10-23

## Supported-runner qualification passed after repair

[Run 39922084392](https://github.com/nessalabs/nessa-agent/actions/runs/39922084392)
passed at source **`53f6db11322c12cee874654dce801fc24b90d0a6`** on Ubuntu
22.04.5/Xvfb, non-root uid 1001, Node 24.21.0 and Chromium 156.0.8078.4 with
`chromiumSandbox:true`. All **18 Node tests** and **nine browser checks** passed:

| Browser check | Result |
| --- | --- |
| Rendered sandbox status | Passed |
| Minimal MV3 content script, messaging, service worker and storage | Passed |
| Same-page agent API and viewer pointer control | Passed |
| Navigation, download bytes and popup context sharing | Passed |
| Five failed-navigation/recovery cycles, invalid-command refusal | Passed |
| Competing profile ownership refused | Passed |
| Cookie, localStorage and extension storage across restart | Passed |
| Independent second-profile cookie/storage state | Passed |
| Browser disconnection fences further commands | Passed |

| Measurement | Observed value |
| --- | --- |
| Startup, one run | 3385.98 ms |
| Authenticated loopback frame requests | 20 samples |
| Frame response median / max | 33.59 / 79.55 ms |
| Mean JPEG size, 1000 × 700 | 36283 bytes |
| Test profile tree removed after confirmed close | true |

These timings are fixture-only measurements on one worker. They exclude remote
network latency, input-to-paint measurement, CPU throttling and long-duration
loads; they establish no smoothness budget. Browser RSS/CPU was not measured.
The viewer's 250 ms post-response delay limits frame rate independently of these
request timings. The first run's 1465 ms startup and this run's 3386 ms startup
are not a controlled comparison.

[Exact report from job log](evidence/run39922084392-results.from-job-log.json)
records the source revision and successful results. The successful run's
[artifact](https://github.com/nessalabs/nessa-agent/actions/runs/39922084392/artifacts/12487856648)
contains the actual screenshot and sandbox report, expires **2026-10-30 08:05 UTC**,
and has SHA256 `98187583c2819f8103adfd98f7b95eb9df1c7a69566937165761f7ab20894e1c`.
After the earlier artifact-byte proxy denial, no byte-download retry or alternate
route was attempted. The screenshot exists in the runner artifact but was not
retrieved or visually inspected in this environment. Preserve it through an
authorized download before artifact expiry if a durable image is needed.

The recovery change passed an independent review of loader/URL correlation,
hash/stale/child-frame rejection, DOM-before-commit ordering, queue fencing and
observer cleanup. No remaining code findings were reported. The original failed
run and local sandbox refusal below are retained as separate evidence.

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
Node tests pass**; the successful qualification above verifies the repair.

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

## Remaining limits

The local run did not pass launch. The successful runner established the nine
synthetic checks above. No video, human input-to-paint latency, browser RSS/CPU,
real-network bandwidth, WebKit/macOS/Windows, cloud deployment, real OAuth or passkey
behavior was measured.
The harness records successful startup time and 20 authenticated JPEG request
latencies/sizes on a supported host; that does not yet measure human input-to-paint
or production frame budgets. Browser-suite failures are release blockers for any
claim of a working browser integration.

No privilege escalation was requested to alter this local environment's browser
sandbox. A future integration still needs authenticated full-browser display,
resource/load qualification, host lifecycle security and provider-permitted auth
flows. The page-only viewer does not include extension popups or browser chrome.

## Historical review scope and dispositions

The notes below describe findings and verification limits at the time of the
earlier reviews, before supported-runner qualification. The later successful run
at the top of this report is the current browser verification result.

Independent reviews inspected the HTTP credential/origin/host boundaries, ordered
commands, close fencing, partial startup cleanup, verifier assertions and claims.
It independently reproduced the original five passing Node tests. Its four findings
were fixed before handoff:

- Use current Chromium `Layer 1 Sandbox` table rows and positive evaluation rather
  than obsolete labels. A pure fixture test covers namespace/SUID, absent rows,
  disabled seccomp and a negative evaluation. At that review, the rendered page
  was unverified; the later qualification passed its rendered sandbox check.
- Close an already constructed browser if viewer startup fails, retaining the
  primary failure and any cleanup failure. Startup and idempotent shutdown tests
  cover the failure order and later cleanup attempts.
- Require a string command discriminator before property lookup. Authenticated
  malformed JSON now receives `invalid_command`, covered by the real HTTP test.
- Retain and close an unexpectedly successful competing context before failing
  the ownership test. At that review, the real-browser scenario was blocked;
  the later qualification passed the competing-profile ownership check.

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
