Technical review excerpt. Original full report SHA256: `8d0f213fd22b1bacc2185d4c0f188ce85d0a6161651551d856a96fa3bb3f0fad`.

# Issue896 focused CI addition — independent affected review

Disposition: no findings in this narrow affected scope. Blocker: empty. Major: empty. Minor: empty. Nit: empty. No structural defect identified in the added CI step. This is NOT a fourth whole-feature review, a review-counter reset, a waiver of prior findings, or issue completion/merge readiness.

Reviewed author checkout `/workspace/nessa-webhook-delivery-review`, HEAD `bd760f6610abcf9186cc8bf4c57641f427d5942a`, against `4914f5e7bc41982c7a62d4b0d31c859cf83ea1f0`. Tracked source is unchanged during review; the pre-existing untracked `node_modules` entry remains. Exactly one tracked file differs:11 added lines in `.github/workflows/local-auth.yml` after the existing complete default stress step. A Python byte comparison independently proved that removing precisely those11 lines restores the entire base workflow byte-for-byte. `git diff --check` passes. Existing driver/helper/protocol/runtime source and nightly stress workflow are byte-identical to the base.

## Inspected relationships

Read repository AGENTS.md and relevant CODING_STANDARDS gates, review/evidence rules, conversation stress and adding-a-CI-check rules. Inspected the actual new shell block, enclosing job, browser install/build, full-default invocation, later focused steps, chaos condition, failure artifact upload, and aggregate controls.

The addition reuses the existing Ubuntu conversation-stress job, pinned Chromium/WebKit installation and gateway/MCP build. There is no new job, build, installation, dispatch, aggregate change, permission change or enlargement of an existing timeout. The new step has its own10minute bound; the existing job remains60minutes, full default20minutes, browser install15minutes and chaos20minutes. Existing conditions, default cohorts, unrelated steps, budgets and evidence retention are unchanged, proven by complete workflow byte comparison rather than selected string matches.

The shell derives every zero flag from current KNOBS, excluding only seed and webhooks; explicit zero flags take precedence over environment/defaults. The first added invocation selects WebKit, seed1, webhooks1. The second selects Chromium and WebKit, seed1, webhooks3 per engine. `withEngines` iterates engines serially, applying the entire plan to each. These are supplemental focused cohorts, correctly labeled in the step/comments and separate evidence paths; they do not replace or claim to be a full raised or full WebKit cohort. The original full-default invocation still runs beforehand. GitHub's bash step behavior plus explicit pipefail propagates a failing driver through tee; a first-command failure prevents a misleading second success. An explicit chaos condition still permits chaos after an assertion failure once the gateway build succeeded, and excludes cancellation/unbuilt cases.

Actual unchanged driver paths inspected: conversation-stress.mjs meta/Run/startStack/main, webhook step, send/settled, linking/pool controls; lib/{stress-plan,cli,browser,run,gateway-stack,server}.mjs; scripted local-gateway startup/config/stop; native runtime_config enabled parsing.

- Each focused invocation starts a fresh disposable scripted stack with native `{enabled:false, listenAddress:0.0.0.0:<reserved-port>}`. The address is retained configuration, not an opened wildcard listener: runtime_config's native_section filters enabled=false to None. Both focused plans have linkings0 and pools0 and no other planned step, so neither path that turns native linking on executes. Temporary free-port selection, gateway HTTP and Vite use127.0.0.1. Separate invocation temp directories prevent state from the preceding full-default run enabling focused discovery. No browser/listener was actually launched by this review.
- The fixture URL is `https://localhost/synthetic`; production destination refusal remains active. The no-attempt proof reads the disposable native SQLite database read-only, correlated to credential, conversation and all six source-scope fields. It requires nonempty bounded rows through the fixed Head, all Queued, attempts0 and timestamp null, and closes the database in finally. This is capture/no-attempt coverage, not a positive recipient-delivery qualification.
- The witness turn is typed and sent using the real composer textarea/button, then checked in gateway terminal state and visible page text. The source Head is captured once after that turn commits, required to advance within the same scope over baseline, then compared against the native cursor without chasing a moving Head.
- Credential cache nuance: `stack.webhookCredential` is shared across sequential engine callbacks in one invocation, not separately minted per engine. Each engine receives a new Run, but the same disposable stack persists. Reusing its already-issued active integration credential avoids a second conflicting registration; each case uses a fresh random conversation ID, grants only read, and unshares in finally. The separate WebKit1 invocation starts its own stack/credential. This source tracing establishes the intended cache relationship; no cross-engine browser execution is claimed.
- Webhook finally attempts unshare, records cleanup failure, closes its owner client and restores the main conversation. Per-engine finally joins background/follower cleanup, closes the page/context and disposes restart observation. Top-level main closes its target; stack cleanup stops the gateway, writes gateway evidence, closes the dev server, removes temporary credential/data/scenario directories and synthetic GitHub resources when present. These are source-inspected normal/throw paths, not a newly exercised hard-timeout cleanup guarantee.
- Separate evidence directories/logs preserve the WebKit1 and both-engine3 plans, engine-labeled reports/views/screenshots and gateway/ACP/MCP logs. Existing failure artifact upload includes the entire STRESS_EVIDENCE tree. No successful-run artifact or actual browser receipt is inferred from this configuration.

## Independent execution

Node v24.19.0. No source edits, installs/downloads, native/browser launches, gateway builds, full suite, workflow dispatch, CI retry, publication or remote write occurred.

1. Actual new bash KNOBS→zero-array extraction, then actual exported `parseOptions`, `knobsFrom` and `plan` with both commands' arguments, using a Node assertion driver only (no conversation-stress startup):
   - `engines:[webkit], webhooks:1, linkings:0, pools:0, steps:[webhook]`.
   - `engines:[chromium,webkit], webhooks:3, linkings:0, pools:0, steps:[webhook,webhook,webhook]`.
   - Assertions checked every other KNOBS value is0 and seed remains1. No evidence directory or listener was created by these dry probes.
2. `node --test scripts/chaos-prerequisites.test.mjs scripts/ci-pnpm-install.test.mjs scripts/ci-result.test.mjs verification/desktop/scripts/lib/stress-plan.test.mjs`:24 passed,0failed/skipped/cancelled,139.164106ms. Covers prerequisite outcome, chaos-after-failure/cancellation, aggregate fail-closed behavior, existing platform budgets/guards, deterministic planning and default/raised webhook counts.
3. Existing pinned binary `/tmp/nessa-896-actionlint-bin/actionlint -version`:1.7.12. `/tmp/nessa-896-actionlint-bin/actionlint -color` from repository root: exit0, no diagnostics, all workflows. This review reused the provided official-release binary, did not download another copy or use an alternate compiler.
4. Exact11line-removal byte equality to base and `git diff --check`: passed. Final HEAD and untracked-only status unchanged.

Parent receipt `/tmp/nessa-896-focused-ci-controls.log` also says24/24; the commands above were independently rerun. Source and dry-plan/lint/control results do not establish actual focused browser success or10minute duration feasibility under CI load.

## Qualification limits

No focused browser case was executed by this review. Source/dry-plan/lint results do not establish browser acceptance or runtime duration. Existing whole-feature rounds and source-qualified predecessor evidence remain unchanged.
