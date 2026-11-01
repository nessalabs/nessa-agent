# Parent Stop recovery integration

**Status: local qualification complete except the shipping image guard; corrected hosted qualification pending publication; not merge-ready.**
Current corrections: `027f430cbbe630c0ebfa62fa800eeabfe9f1eaee`, including main
`f08de87e40f2897df57556ad24adad66a82bf308` (#1194). Independent reviews of the
committed integration and each correction found no issues. These are scoped
source reviews, not certification of the whole historical PR or platform gates.

The final local unit/integration selection at `027f430cb` completed all **38
binaries**: **8493 passed, 1 failed, 41 ignored**. The sole failure is the
unchanged shipping worker-image guard. All four corrected legacy report cases,
the restored admission-order regression, typed Agent replay rejection, and the
seven service cases from the earlier failed run passed in this complete rerun.
This records a new qualification result; it does not infer the old failure's
cause. The task-local subreaper reaped 23 adopted descendants and preserved the
failed command status. The rebuilt selection used standard profiles and took
13m50s; the server library passed 4506 and failed only that guard in 251.89s.

Strict Clippy for all eight CI packages and all targets passed at `027f430cb`,
with standard profiles, `CARGO_INCREMENTAL=0` and one compiler job. Fresh frontend
qualification passed **5510/5510**; MCP/browser helpers passed **789**, with one
existing skip. Their inputs are unchanged from their recorded source checkpoints.
All **74 doctests** for the eight CI packages passed separately after the guard
stopped the main runner. The canonical SDK Rustdoc check passed with `-D warnings`.
TypeScript, architecture tests and the configured architecture checker passed.
The first architecture-check invocation lacked Cargo on PATH; its failed log and
the configured successful retry are both retained.

**Earlier integrated-source failure record:** the same eight-package selection
completed all **38 binaries** at `50bef867f`:
**8482 passed, 12 failed, 41 ignored**. The server library accounts for all 12 failures:
four legacy report fixtures expected no reply instead of the real child's saved
reply, seven other service cases failed in this concurrent full run, and the
shipping worker-image guard failed as expected. All seven other cases passed
unchanged in isolated diagnostics (18 handoff tests and three exact tests).
Those diagnostic passes do not relabel the full run or establish its load cause.
The legacy fixtures now use the real completed child result and existing report
formatter; the complete corrected rerun is recorded above. The actual
production lock inversion failed its exact regression with an admission-cycle
assertion in 5.12s (exit 101); the original source was restored and verified clean.
The restored-order positive regression passed in that full selection.
The task-local subreaper reaped 22 adopted descendants and preserved the command's
failure status. Earlier bare-container failures remain separate failed evidence.

Current integrated-source production Chromium records:

- Parent Stop default/override and console checks: **4/4**.
- Focused stress: **9/9**, including two parent Stops, typed await, thread
  delegation, handoff and final settlement.
- Codex/Terra parent Stop on the rebuilt `027f430cb` binary: **4/4**.
- The first focused Terra stress preflight could not run: its default Full
  scenario requires Ask/Auto/Full, but this Linux model advertises Ask only.
  This remains unrun, not passed. The existing documented `ask-only` profile
  passed **11/11**, including Full refusal and denied Ask permission, two parent
  Stops, delegation, typed await and handoff. The capability guard is unchanged.
- Smoke across columns/sidebar layouts: **17/17**.
- Changed content, held-reply and held-Stop checks at 760x700 in both layouts:
  **7/7**.

Current parent Stop captures: [default cascade](parent-stop-chromium-default.png)
and [explicit override](parent-stop-chromium-override.png). The capture run passed
**4/4** on the rebuilt candidate.

These use the already-installed Chromium 151 and an immutable gateway binary
built from `50bef867f`. Browser runner/frontend inputs and gateway behavior are unchanged by the later
unused-import cleanup and legacy unit-fixture correction. No alternate browser
download was attempted. Hosted [run 40990262102](https://github.com/nessalabs/nessa-agent/actions/runs/40990262102) at `f09246860` passed frontend, gateway contract,
desktop release compile, SDK domain coverage and the scripted gateway aggregate
in both pinned browsers. Its three platform jobs failed strict Clippy on the
same unused import, now removed. The entire browser/stress job passed, including
the default 57/57 stress and separate focused checks. Windows server-other passed
2248 with seven ignores. The Windows conversation partition completed
1394 passes, four legacy-fixture failures and six ignores. Its repaired admission
test and typed Agent replay rejection both passed. That CI attempt completed
with eight passing jobs and five failures including its aggregate. Corrected
hosted qualification is pending publication.

The integrated source retains typed agent origin, Cedar's agent/owner boundary,
exact Stop selection and supervised report admission. Canonical person-command
chains cannot acknowledge a report through the legacy Person fallback. Legacy
collision fixtures explicitly seed old Person history; the new regression
rejects Agent-to-Person re-authorship with identical actor, identity and text.
Recursive fixtures drive grandchild creation from genuine person admission to
the child; agent seeds invoke no relay tools. Default Stop withdraws the exact
queued seed report; later genuine person input releases only its exact typed
source while the original withdrawal remains final. Override checks observe the
same captured descendant executions after the full observation window.

The shipping image guard and pin are unchanged. The ordered 43-source fingerprint
calculation at `027f430cb` remains `91fd4c70b61e3d05`, while the shipping pin is
`b6e4f48aa694ce08`. Boat was not contacted. The precise existing-account setup
request remains below. Custom WebKit geometry and native Swift phone
qualification remain unrun locally. State-page metadata, sources, links and Git
checkpoints validate. The standalone read-only Nessa Docs renderer at
`c0bc0627611b100ab71a96cc6fe2fb65f1a014d8` accepted 141 pages with zero issues.
All three charts on the two changed pages rendered in installed Chromium; full
source checkpoints and Health were checked with no browser errors. The changed
preparation chart also responds to Fit. These are scoped page checks, not a
whole-renderer/browser-platform certification. The owned renderer exited cleanly.
The pages retain their reviewed `50bef867f` checkpoint because the later correction
changes only an unused import and unit-fixture setup, not modeled behavior.

Two earlier typed-main compilation attempts were OOM-killed; neither ran tests.
Only 13 GiB of this task's generated incremental cache was removed. Source,
executables, libraries and logs were retained. Matching observed hosted CI's
nonincremental mode then compiled all 38 binaries and completed strict Clippy
without a new OOM kill. Failure excerpts, distinct checkpoints and scoped reviews
are retained beside this file.

**Historical pre-provenance checkpoint:** parent had just merged main `f08de87e40f2897df57556ad24adad66a82bf308`
(#1194). Incorporation and qualification of that new main are pending. Results
below belong to the explicitly named earlier revisions and do not qualify its
typed agent provenance/Cedar integration. Local verification corrections are
committed through `fda398c1d9548d9013b012577870bea3e01e9df3`; only the older
checkpoint `93599d3` is published at this point.

Earlier source reviewed: `85f057b2f9947d1e54dcc596922665424dfd443a`, adding main
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
- Normal Rust tests: see the final verification record below, including the
  failed CI selection and separate supplemental/diagnostic execution.

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
WebKit and native Swift phone qualification remain unrun locally. Chromium
gateway E2E was pending at that checkpoint; its executed results are recorded
below. No external real-event transmission occurred.

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

The existing provider's dedicated-environment posture is:

```json
{
  "safeForThirdParties": false,
  "passAgentsCredentials": true,
  "passGithub": false,
  "passSecrets": false,
  "passSandboxCredentials": false,
  "envContents": "",
  "secretFiles": [],
  "repositories": [],
  "credentialProfileId": null,
  "proxyAiCredentials": false
}
```

This is read from `workspaces/infrastructure/boat.rs`; no provider request or
credential inspection was performed. The selected environment still has the
original restricted network policy and no bound secrets.

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

## Source85 verification updates (before mainf08)

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

A one-compiler-job retry started on current source85, with default profiles, test
threads, all assertions and the worker guard retained. It recorded five completed
suites (795 passes) before its process disappeared without a final summary or
recoverable session result. Its overall outcome is **uncertain**, not passed;
the OOM-kill counter did not increase during that retry. The replacement uses
the repository's exact eight-package CI runner with one compiler job and its
normal two-binary concurrency. Browsers are kept idle during it. The original
checkout was advanced only after its frozen checks
ended, so current runtime scripts participate in the retry. The redundant
standalone gateway build was stopped; the already-built local test gateway was
retained. Its executable was copied and SHA256 matched before E2E; this is not
a published Boat artifact. The final result and browser rerun are recorded in
the final record below.

## Final runtime verification record

The exact eight-package CI selection compiled all **38 test binaries**, with
`CARGO_BUILD_JOBS=1` and normal filesystem ownership. The normal runner started
14 binaries before its fail-fast queue stopped: **7294 passed, 9 failed, 40
ignored** across those completed binaries. Embedded child-probe summaries are
excluded from that total by taking each binary's final summary. The runner
exited 1. Its 24 unstarted binaries are not counted as passed by this run.
See `ci-selection-rust-summary.log`, including all failure details and the raw
log checksum. No assertion, test thread default, cleanup deadline, feature
selection or shipping guard was weakened.

Seven SDK failures and one server Codex-status failure share process cleanup.
The failing tests and their direct cleanup owners match main byte-for-byte,
as checked independently. An unchanged isolated private-directory test failed
with `CleanupUncertain` and exit 101. Its newly created `sleep` process was
`Z`, parent 1, group 153391; probing that group with signal 0 still succeeded.
This container's PID 1 is `tail`, which does not reap those orphan descendants.
The same exact isolated test passed under a task-local child-reaper supervisor,
but that particular pass adopted zero descendants and alone does not prove why
it passed. Broader unchanged counterparts under that supervisor passed **13
process tests plus Opencode, forced-shutdown and server Codex-status tests**;
the supervisor reaped respectively 6, 2, 1 and 2 adopted descendants. It only
reaped its own descendants and explicitly propagated the primary command's
wait status. It did not alter PID 1, existing orphan groups, source, settings,
assertions or network policy. These are narrow environment diagnostics, not
a relabeling of the original concurrent CI selection as green. The original
orphan zombies are not falsely reported as reaped. See `cleanup-*.log` and the
two `diagnostic-*-source.txt` files.

Normal doctests for all eight selected packages passed **74/74**, separately
because the failed standard runner stops before its doctest phase. See
`ci-selection-doctests-summary.log`.

The remaining **24/24 binaries** were subsequently executed from the exact same
eight-package feature selection, unchanged, and passed **1177 tests**, with
one existing ignore. Thus all 38 binaries were executed across the failed normal
runner and supplemental completion: **8471 passed, 9 failed, 41 ignored**.
This does not change the failed runner verdict. See
`ci-selection-remaining-summary.log` and `remaining-ci-binaries-source.txt`.

Hosted run [40981242287](https://github.com/nessalabs/nessa-agent/actions/runs/40981242287)
at published head `93599d357336dad4ae86dba32ed33c28e54c8cb7` provides an
independent current-source platform result: Ubuntu's SDK library passed
**1582/1582** and server library passed **4501 with one failure**, solely
`shipping_worker_covers_the_compiled_lease_protocol`. Other unit/integration
binaries shown in that job passed; later steps were skipped after its failure.
The SDK coverage, frontend, gateway-contract and desktop release compilation
jobs passed. This does not assert a passing aggregate or completed Windows,
macOS, browser-stress or native phone qualification. Selected immutable job
excerpts are in `ci-ubuntu935-summary.log`.

The quiet Chromium parent-Stop retry passed **4/4**: default cascading,
parent-only override, and each page's console check. The local gateway SHA256
still matches its compiled original-checkout binary; see
`gateway-binary-sha256-confirmed.txt`. The first browser crash remains retained
in `source85-parent-stop-first.*`; the quiet pass does not establish that
crash's cause. These checks use system Chromium 151.0.7922.173 and isolated,
signed-out scripted-agent settings, not a real provider or shipping worker.

### Scoped verifier correction

Quiet source85 checks also held gateway-window **7/7**, scripted permission,
failure and cancel **5/5**, and resize **3/3**. MCP Apps held **8/9**: Context
answered blank after Message. Context alone held **3/3**, but the supported
Message/Context pair reproduced **3/4**. The fixture writes `pending` synchronously
when its click handler starts, so blank does not prove a context RPC refusal.
Those production/test owners match main; the recorded-turn fixture does not use
the scenario-only typed-error/version changes.

Commit `4f3d5c866bff445f37f7661a1337c3c6a85606e7` makes Context prove trusted
pointer entry through the existing sandbox readiness helper before the ordinary
click, preserving the exact `ok` assertion and deadlines. Each helper invocation
now owns its observation and UUID, so an old Message listener cannot certify
Context. A fresh review found no findings after that marker correction. Old
listeners remain until sandbox document close; they do not share observation
state, and the runner owns page cleanup.

The actual helper body passed a real-browser negative target probe: Message
entry 1, Message events attempting to certify Context 0, Context entry 1.
Reverting only the helper made that negative probe fail (actual 1, expected 0);
restoring it passed. Reverting only Context's readiness hunk reproduced **3/4**
with the blank answer. Restored final source held the pair **4/4** and complete
MCP Apps **9/9**. Relevant helpers passed **95/95**; formatting, lint and syntax
passed. All original failures, reverted hunks and probe source are retained.
This closes the verifier's local reproduction without claiming an underlying
browser/proxy cause or changing product behavior.

### Windows admission qualification

Current hosted Windows conversation testing at935 failed one parent-Stop test:
`owned_stop_export_and_submission_share_one_admission_order`, **1393 passed,
1 failed, 6 ignored**. Its aggregate five-second deadline combined admission
acquisition with disk-backed receipt completion, then labeled any timeout a
cycle. Its precise historical cause remains unresolved; Linux passes do not
clear Windows. See `ci-windows-conversation935-failure.log`.

Commits41c4/fda398 correct that test's evidence boundary: the writer alone must
acquire within the unchanged five-second cycle bound. Actual command completion,
lost-caller supervisor drain, both receipt replays and retirement then share
one absolute deadline from the fixture's existing `TURN_HOLD_HANG` ceiling
(qualified for loaded Windows in issue558). Production order and all exact
actor/receipt/single-cancellation assertions remain. A minor unbounded final
replay found in review was fixed; the fresh final review was clean. Compilation,
actual lock-order negative control, restored execution and fresh Windows
qualification are pending at this checkpoint.


## Published matching worker for source22a8218b

This is the historical Dad5 publication. Its task snapshot was retired only
after the c516 replacement passed fresh-clone qualification; the receipt below
retains the original publication facts.

The [worker publication receipt](worker-release-receipt.json) records image
`bsh_-6mHzeuFu3FWNbJWOe0IbXLXxhBcJPNOjq76Bk_nMGI` for protocol
`dad5afbc1bc5755f`, built from application source
`22a8218b0ff2c392e911e07c75994f33e4896048`. Both `nessa` and `nessa-mcp`
were freshly verified with their recorded checksums and lengths. The receipt
includes the locked PTY vendoring restoration, independently compared upstream
file hashes, limited credential-path checks and provider-confirmed cleanup of
all three publication VMs. This replaces the shipping mapping, not the protocol
matching rule. The four-VM live verifier is prepared but not yet run at this
publication checkpoint. Native and performance limits above remain distinct.

## Current worker for source66e064de

The [c516 publication receipt](c516-worker-release-receipt.json) binds image
`bsh_MF9Ppg9XWHyVD8qYw7Wv6mku_yJpX2lS-Dlj4-LZ27E` to protocol
`c516606214ce8bc2` and frozen source
`66e064de4a6177454e22873314391d8956b5da79`, integrating main `100d7aef`.
Both Linux binaries were freshly built and checked in a fresh image clone,
including checksums, lengths, libraries, MCP initialization, lockfile and
Node/supervisor/SDK provenance. Named credential/state exclusions are limited
checks, not an exhaustive secret scan or secure-erasure attestation.

All six publication VMs across Dad5 and c516 are confirmed absent. Cumulative
usage is US$0.0624; six of the approved ten VM creations are used, four remain,
and the one retained task snapshot is within the account's included allowance.
The unchanged four-VM live verifier is awaiting the pin commit at this
publication checkpoint. The shipping guard and separate native/performance
qualification requirements are unchanged. This pin changes artifact identity,
not lifecycle behavior; no unrelated state-page checkpoint is advanced.
