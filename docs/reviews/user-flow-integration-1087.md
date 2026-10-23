# User-flow integration checkpoint (#1087)

This is an isolated assembly of the three reviewed flow PRs on current main.
It prepares real conflict and documentation corrections; it does not merge a
pull request, change an existing PR head, or clear browser qualification.

| Input | Exact source |
| --- | --- |
| Main, including inbound external events | `084b79aeedf157286810512c524aede59887f17b` |
| #1090 downloads/composer/rule ownership | `0d30b39e440c97523dd925ba90e187fd92da8b9d` |
| #1093 detailed flow documentation | `6b13b64cc51616abcdf167745f3c809cb8bedaa1` |
| #1094 native-hook/notification ownership | `de4f682f1f53ac081530d05347fc36e17dada2ce` |
| Assembled runtime checkpoint | `a32206a4b867e8a4515a37d803b7753dce90d534` |

The candidate branch is `codex/1087-integrated-user-flows`. Local merge commits
retain the exact reviewed histories. The parent remains the sole final merge
owner and decides how to land the reviewed integration delta into the PR stack.

## Resolved overlap

- Workspace README retains both sets of diagram targets and Browse flows links.
- Desktop checklist retains both the original ownership cases and the settings
  follow-up contracts, including their reachability/physical-effect limits.
- Shared selectors retain both complete exports: `userFlowOrdering` and
  `settingsActionOwners`.
- All five production owners are byte-identical to their reviewed inputs. Main's
  Rust crates and Cargo.lock are unchanged by the assembly.
  [Owner equality checks](../../verification/desktop/evidence/user-flow-integration/owners.json).
- The native-hook and notice-permission journey pages now explain the corrected
  admission/retirement behavior and drill down into the detailed action-owner
  page. Their three scoped checkpoints advance to the assembled runtime after
  reviewing the unchanged owners/tests. Unrelated checkpoints remain untouched.

## Browser access limitation, precisely scoped

The original `/tmp/nessa-browsers-install.log` records Playwright attempting
Chrome for Testing 153.0.8010.12, Chromium v1243, at
`https://cdn.playwright.dev/builds/cft/153.0.8010.12/linux64/chrome-linux64.zip`.
The HTTP response was **403**, body **Domain forbidden**. The installer made its
own retries to that same URL, then exited before reaching WebKit. No separate
agent retry or alternate host was used. This is a network-access limitation,
not an automatic approval-review judgment. A WebKit archive denial was not
independently observed; later WebKit launches report its executable absent.

A local metadata-only `playwright install --dry-run webkit` reports the pinned
WebKit 26.6 / v2359 primary target:
`https://cdn.playwright.dev/dbazure/download/playwright/builds/webkit/2359/webkit-debian-13.zip`.
Its installer also includes FFmpeg v1011 at
`https://cdn.playwright.dev/dbazure/download/playwright/builds/ffmpeg/1011/ffmpeg-linux.zip`.
This dry run made no download. The narrow pending operation is the official
WebKit-only install and that ancillary dependency into a writable cache; browser
runtime libraries must also be checked before launch. Network access and any
required system-dependency installation remain for parent adjudication. No
mirror, host override, security-setting change, download or dispatch retry is
performed by this integration work.

The separate manual-dispatch blocker was CLI's workflow lookup:
`GET https://api.github.com/repos/nessalabs/nessa-agent/actions/workflows/canonical-browser-verification.yml`
returned `Forbidden` before any dispatch POST. No manual run was started.
This is distinct from the browser CDN limitation and the later ordinary CI
browser-dependency download timeout.

## Prior execution and current gates

The parent reported #1093 CI run 39924618451 and #1094 run 39928148832 all green
at their respective heads. The successful #1093 stress job 120319054693 checked
merge tree `f2fe99687cbce242c2b1b9012b887ed8a5667a63`, whose differences from
#1090's source head are only docs/evidence. Its four canonical scripted groups
passed in Chromium and WebKit (4/4, 16/16, 15/15, 8/8); usage-limit 16/16, selected
drag 8/8, conversation stress 42/42 and chaos 2/2 passed. It did not invoke either
custom script and predates #1094's runtime changes.

#1090 run 39924552205 retained a browser runtime-dependency installation timeout;
the downstream chaos attempt lacked the never-built gateway binary. Those are
setup failures, not demonstrated gateway/chaos defects. Parent reported a
same-job retry accepted after terminal-state verification; its outcome remains
separate from this candidate. No no-op commit, new CI trigger or denied-dispatch
substitute was introduced.

Local #1094 private-dependency repair had already passed smoke 17/17, selected
settings 6/6, settings tests 354/354 and TypeScript on its unchanged head. It
materialized byte-identical pinned fonts within that worktree, without broadening
Vite access. Earlier borrowed-font failures remain historical evidence. Those
results are not new execution evidence for this assembled source.

The assembled candidate still needs its own checks and independent exact-head
review. Both custom scripts require Chromium and WebKit acceptance; a Chromium
pass alone does not satisfy that gate. Native harness effects, visible OS prompts
and ordinary production source rebinding remain unverified. No duplicate
physical-effect or notification authorization-bypass claim is made.

## Initial assembled execution results (historical checkpoint)

At runtime assembly `a32206a4b867e8a4515a37d803b7753dce90d534`, full frontend:
**5,154 passed / 2 failed, 5,156 tests in 390 files**. The two failures remain the
unchanged local-credential-source and local-gateway-endpoint private-file
acceptance fixtures. This sandbox reports `/` and `/tmp` owned by uid 65534;
security assertions were preserved. TypeScript, changed-file lint, the actual
architecture scanner, and **409/409 verification/provenance tests** pass.

Commit `77910882a16e18ffa33a0cbf54aa3370b0ec2978` then adds the settings-owner
script beside the original ordering script in the existing manual browser job.
It preserves the manual-only trigger, read-only permissions, unmasked exits,
shared provenance/artifact directory and always-upload step. **8/8 provenance
tests**, YAML/shell assertions and formatting pass. No dispatch occurred.

On that unchanged runtime plus the manual-job addition, integrated Chromium
checks hold **7/7 ordering** and **6/6 settings**. These include both retained
selector exports and both production fixtures. WebKit was not run on the
assembled candidate; its absent executable and the download hold remain.
No canonical, stress, chaos, full desktop or native suite was rerun locally for
this candidate. Earlier CI results above retain their exact source/tree scope.

Nessa Docs validates **131 pages / zero issues**. Chromium rendered **32 diagrams
on all 19 changed state pages**, opened **52 detail bindings** and followed
**18 drill-down paths**, with zero page errors. All **224 relative Markdown links**
in those state pages resolve.

[Catalog](../../verification/desktop/evidence/user-flow-integration/catalog.json) ·
[Viewer results](../../verification/desktop/evidence/user-flow-integration/statecharts.json) ·
[Ordering](../../verification/desktop/evidence/user-flow-integration/ordering-chromium.json) ·
[Settings](../../verification/desktop/evidence/user-flow-integration/settings-chromium.json).

These are implementation checks, not independent review or exact-head hosted CI.
The candidate must receive both and complete custom two-engine qualification
before the parent considers merging. The expected landing order remains #1090,
then #1093, then #1094, with these actual conflict/documentation corrections
applied at the appropriate boundary; the isolated candidate does not choose a
replacement PR or mutate those heads.

The production frontend build also passed, retaining the existing large-chunk
and static/dynamic private-file import warnings. Dependency setup used pnpm
11.9.0, the frozen lockfile and a private vendor/dependency layout in this
worktree; no Vite access setting or assertion was changed.


## Main d029d1cf integration

Independent review of frozen head `e5d4dc3af03abab99acabc67d9052a7c41ef8dc2`
was clean: the reviewer reported 69/69 targeted tests, 409/409 verification,
131 pages and 224 links, with the two baseline private-file failures reproduced
unchanged. Parent then authorized substantive integration of main
`d029d1cf126585e5e2b0146d92d37e7585ad67ab` into #1097.

Merge commit `110dacd432341d4801c39c554f774f52b82b89b0` preserves that main's
Rust, protocol and client sources without edits. Its shared transcript selector
updates and both custom exports coexist. The UI vendor was refreshed through
the repository's `ui:types` command to
`8cc1dee2409ac6334377effcd2cf0ca998e5a62f`; a frozen install alone reported up to
date while leaving the old vendor revision, which the read-only UI check caught.
No tracked dependency configuration changed. The five flow owners remain the
reviewed implementations. This mainline integration changes no state-transition
owner contract, so unrelated diagram checkpoints remain at their scoped reviews.

The newly authorized WebKit-only operation, after the earlier Chromium denial,
requested the official v2359 primary URL and immediately received HTTP 403,
body `Domain forbidden`. The installer was stopped at that denial; no alternate
host was attempted and FFmpeg was not reached. This is now a directly observed
WebKit network-access denial, distinct from the original Chromium archive and
GitHub workflow lookup denials. No download or dispatch was retried for this
mainline update. Both custom WebKit qualifications remain open.

The two private-file tests reproduce at baseline main 084b79ae and frozen e5d4dc3a:
2 failed / 37 passed across the two files, with identical source/test blobs. A
read through the unchanged helper records the first ancestor `/`, owner UID
65534, while process UID is 1000, and returns NessaPrivateFileUnsafeError. This
supports the sandbox-ownership explanation without altering its security rule.
Fresh logs remain separate while the current head is qualified.

Fresh checks at runtime `110dacd4`, with the new UI pin, passed TypeScript,
changed-file lint, the actual architecture scanner, and **434/434 verification,
provenance and scripted-scenario tests**. Both protocol/schema regeneration
checks passed, including **58 product-contract tests**. The full frontend gave
**5,167 passed / 2 failed, 5,169 tests in 392 files**; the same two private-file
fixtures remain the only failures. The production build passed with its existing
chunk/import warnings. The new-head gateway built successfully in **10m15s**,
retaining the existing lease-protocol constant-evaluation warning.

Integrated Chromium holds **7/7 ordering**, **6/6 settings action owners** and
**6/6 selected settings-page cases**. The first smoke run, overlapping cold Rust
compilation and the full frontend workers, held **15/17** with two overview
keyboard-focus failures. Those failures are retained. A separate unmodified-main
d029d1cf worktree with the same UI pin/browser held **17/17**. One matched candidate
repeat after the frontend workers finished held **17/17**. No focus source,
assertion, timeout or console filter changed. The evidence does not establish a
causal integration defect or prove that the initial focus result is harmless.

[New-base browser records](../../verification/desktop/evidence/user-flow-integration-main-d029/smoke-during-builds.json) ·
[Main baseline](../../verification/desktop/evidence/user-flow-integration-main-d029/main-baseline-smoke.json) ·
[Matched candidate repeat](../../verification/desktop/evidence/user-flow-integration-main-d029/smoke-after-frontend.json) ·
[Ordering](../../verification/desktop/evidence/user-flow-integration-main-d029/ordering-chromium.json) ·
[Settings](../../verification/desktop/evidence/user-flow-integration-main-d029/settings-chromium.json).

The state diagrams themselves are unchanged by this mainline update. The catalog
again accepts **131 pages / zero issues**; the earlier 32-diagram, 52-detail,
18-path viewer pass remains scoped to unchanged diagram/navigation content.
No whole-atlas source checkpoint advance is implied.

Rust protocol projection/tool-part tests also passed: **85/85**, with 97 unrelated
cases filtered out. The first signed-out default Chromium stress run passed its
new tool-waterfall case, then failed the long-history reopen budget: 1,472 ms for
30 turns versus 67 ms for one turn, exceeding the allowed additional 1,100 ms.
It held **15/43** results, with later plan steps explicitly not run after that
failure. [Retained first stress result](../../verification/desktop/evidence/user-flow-integration-main-d029/stress-during-projection.json).
This run overlapped local Rust test compilation. One repeat after all compilation
ended uses the same source, plan, assertions and budget; its result is recorded
separately. The first failure is not erased or attributed to a source defect
without evidence.

After local compilation ended, the same signed-out Chromium stress plan held
**43/43**. Its long-history reopen measured **444 ms** against **37 ms** for one
turn, within the unchanged 1,100 ms additional budget. The new waterfall case
and all later steps passed. [Repeat result](../../verification/desktop/evidence/user-flow-integration-main-d029/stress-after-builds.json) ·
[Folded waterfall](../../verification/desktop/evidence/user-flow-integration-main-d029/trace-chromium-folded.png) ·
[Expanded waterfall](../../verification/desktop/evidence/user-flow-integration-main-d029/trace-chromium-opened.png).
The earlier stress and smoke failures remain preserved; no source-causality or
universal performance claim follows from these two measurements.

## Browser dependency installer diagnosis and chaos prerequisite

Parent supplied old-candidate CI run 39969322169, job 120467553367. Its APT
metadata update completed quickly, then archive downloads from the runner's
configured `azure.archive.ubuntu.com` stalled/retried. It requested 119 MB of
archives; several multi-megabyte font/codec packages took minutes or were ignored
for retry. The step timed out after 15 minutes at 14:40:48 UTC, before Playwright
browser archives or browser assertions. No deterministic repository repair for
that mirror throughput is established. No mirror, installer, dependency,
security setting or timeout is changed.

A separate deterministic workflow defect was repaired in commit
`7ba1d4c0f639815b12ac30620a312e2113b02ba5`: chaos previously ran after failed setup,
although the gateway-build step had been skipped. The existing build now has an
identified outcome, and chaos requires that outcome to be success while retaining
`!cancelled()`. Chaos still runs after stress assertion failures once the gateway
was built. An installer/build failure still leaves the job and required check
failed; this guard does not manufacture browser acceptance or skip usable chaos
coverage. No new job or trigger is introduced and no old run is manually rerun.

Six semantic guard regressions exercise skipped/failed/cancelled build outcomes,
a completed build after a stress failure, cancellation, and ownership of the
readiness marker. The original workflow gives **4 failed / 2 passed**; restored
source plus existing CI aggregator/provenance tests gives **22/22 passed**.
Parsed workflow checks confirm unchanged triggers, permissions, jobs, installer
command and timeout. The earlier #1091 retry denial and manual workflow dispatch
lookup denial remain separate and binding.

[Combined checkpoint counts](../../verification/desktop/evidence/user-flow-integration-main-d029/checks.json).
Independent delta review and normal exact-head hosted CI remain pending. All
original PRs remain open. Neither the new mainline integration nor the legitimate
prerequisite fix clears the custom two-engine WebKit gate.

Final changed-state-page link scan: **225 relative Markdown links resolve** across 19 pages. The added current-main checkpoint paragraph introduces one additional review-evidence link; diagram bindings and navigation targets remain unchanged.
