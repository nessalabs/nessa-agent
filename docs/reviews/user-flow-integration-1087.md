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

## Assembled execution results

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
