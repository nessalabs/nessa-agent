# Pending action ordering evidence (#1087)

Base: `0b8019cf45f99b285f10c41199875d474492c6d1`.
Current source head: `7f434845423686cdd01a17300c47d10d46911f50`. Linux, Node 24.19.0,
Nessa UI pin `d550fac7a59b8f9b6c9d6f1e7dced07ca38ee35b`, development Vite,
installed `/usr/bin/chromium`, 1100×850 fixture viewport. No production accounts.

```sh
node verification/desktop/scripts/user-flow-ordering.mjs \
  --engine chromium --chromium-executable /usr/bin/chromium \
  --out /tmp/nessa-flow-browser-final.json \
  --shots verification/desktop/evidence/user-flow-ordering
```

Fixed run: **6/6 held** (four ordering checks plus two console records).

```text
ok    uncertain-install-busy [chromium]
ok    uncertain-install-not-confirmed [chromium]
ok    cross-tab-clear [chromium]
ok    old-refusal-new-send [chromium]
```

Both uncertain outcomes: **1 installation dispatch, 0 stale Download buttons,
1 button after a fresh check**. Cross-tab clear: B remains expanded. New B send
after old A refusal: B collapses when its own draft leaves.

![Browser ordering fixture](chromium-user-flow-ordering.png)

Revert probes used the baseline source for only the owning hook while retaining
the new tests. Downloads: **2 failed / 9 passed**. Composer: **2 failed / 1 passed**.
Restoring both baseline hooks in the browser fixture: **2/6 held**, with all
four ordering checks failing (older offers restored Download for Busy and Not
confirmed; both composer checks failed their expected expansion state). Fixed
source restored afterward; the same browser command passed 6/6.

The fixture drives production AgentDownloads/useComposer through injected
ports. It does not establish backend duplicate physical installation, provider
completion, or real native window behavior.

WebKit is **unrun**. Playwright browser installation was denied by network
policy (`403 Domain forbidden` at `cdn.playwright.dev`); no alternate host was
used. System Chromium is a separately installed executable.
The historical first gateway scripted E2E attempt reported **could-not-run (exit 2)**: `cargo build -p
nessa-server` could not start because Cargo was absent. The later successful gateway build and four Chromium groups below supersede this limitation for Chromium. Default and raised
conversation stress, chaos, native macOS acceptance and live providers remain
unrun. The initial architecture scan also could not execute its Cargo metadata step; the later scan passed all 108 tests and the actual repository check.

The full frontend baseline's private-file acceptance failures are retained:
this sandbox reports `/` and `/tmp` owned by uid 65534, while the existing
private-file acquisition contract accepts root/current-user ancestors only.
Those tests and their security checks were not weakened.

Historical first source-tree checks: **129/129 targeted tests in 19 files**;
**401/401 verification-library tests**; TypeScript, changed-file ESLint,
source/fixture formatting and production build passed. Build retained its
existing large-chunk warning. Full frontend: **5130 passed, 2 failed in 390
files** (the same two unchanged private-file acceptance fixtures). Architecture:
**107 passed, 1 could not execute Cargo metadata**, reported by that test runner
as a failure. Required exact-head CI remains pending on GitHub.

Documentation source/test checkpoints include current
`7f434845423686cdd01a17300c47d10d46911f50`, the revoke owner at
`4d718da047acbcf200c10feab62cb45b097ee939`, the unchanged startup/session/download
owners at `11e2746f7df088a1bd5ee3b2e579ad8a40b5cbd1`, and linked-device and
sharing owners reviewed at mainline
`0b8019cf45f99b285f10c41199875d474492c6d1`. Navigation-only changes do not
advance unrelated checkpoints. Renderer/skill checkout:
`nessalabs/nessa-docs@c0bc0627611b100ab71a96cc6fe2fb65f1a014d8`.

From that renderer checkout:

```sh
npm run --silent validate:docs -- --project /workspace/nessa-agent-docs
NESSA_DOCS_PROJECT=/workspace/nessa-agent-docs PORT=4313 npm start
```

Final catalog: **129 pages, 0 issues** ([JSON](catalog.json)). Installed Chromium
rendered **29 diagrams across all 17 changed state pages**, opened **45 authored
node/edge detail bindings** and followed **14 drill-down paths**, with **0 page
errors** ([per-page and navigation evidence](statecharts.json)). All **205
relative Markdown links** in changed state pages resolve to existing files.
The viewer pass covered root → journey index → startup feature → session flow →
supervisor → client recovery; gateway observation and runtime downloads;
workspace → MCP integration management, native hooks, review notice permission, linked devices and sharing; and device → conversation sharing.

![Runtime download chart in Nessa Docs](runtime-statechart.png)

The explicit `--engine chromium,webkit` ordering invocation retained all six
Chromium results and then reported missing WebKit executable:
**6/7 held, exit 2/could-not-run**. This is not a two-engine pass. The full desktop
run-all gate remains unrun. No screenshot or docs renderer check replaces the
missing WebKit, gateway stress or native-platform gates.
## Revoke-focus follow-up and composer qualification

A separate focused commit after `11e2746f7df088a1bd5ee3b2e579ad8a40b5cbd1`
fixes the third independent finding. Permission-rule revoke now retains one
identified owner through command acknowledgement and its fresh read. Background
focus/visibility checks cannot replace it. Repeated callbacks are refused by
that synchronous owner; source/view retirement invalidates late continuations.
No server authorization rule changed.

Seven permission cases plus one actual Redux-store composer case bring this
work to **41 added tests**. The actual-store counterexample sends A and switches
to B in the **same task**; separate, ordinary human-click reachability remains
unverified. Controlled hook fixtures are not proof of that reachability.

Reverting only the permission component gives **6 failed / 6 passed**; reverting
the composer hook and running the actual-store file gives **1 failed / 7
passed**. The original permission component in Chromium permits a second
frontend revoke dispatch after focus and gives **6/7 held**. Fixed source
restored: **7/7 Chromium checks held**, including **one pending revoke dispatch**,
controls disabled through ownership, and recovery enabled after settlement.
The evidence makes no duplicate-physical-effect claim.

Updated source checks: **142/142 targeted tests in 20 files**, TypeScript,
changed-file ESLint, formatting and production build passed. Full frontend:
**5138 passed / 2 failed in 390 files**, retaining the same unchanged private-file
fixture failures. The initial source commit's evidence above remains historical.
System-Chromium overview/focus findings are being triaged separately and are
not attributed to these changes.

The separate focus diagnostic and its exclusions are recorded in
[focus triage](../../../../docs/reviews/user-flow-focus-triage-1087.md). It is not
attributed to the fixes, and its quick-run results are not substituted for the
full desktop/browser gate. Source PR: https://github.com/nessalabs/nessa-agent/pull/1090.
## Exact-head CI import correction

CI run `39919232935` at `4d718da047acbcf200c10feab62cb45b097ee939`
failed the actual architecture scan in frontend, gateway-contract and all three
local-auth platform jobs. Parent supplied the failed steps, including jobs
`120300724852` and `120300724976`: the new composer test's mock imported the
conversation testing entry where that TSX consumer must use public contracts.
The correction removes that mock and exercises the public barrel. It changes
no scanner exception, assertion or runtime rule. The original failed run is
retained; it was not blindly rerun.

After correction: **23/23 affected tests**, TypeScript, changed-file ESLint and
formatting pass. Rust stable **1.99.1** was installed under `/tmp` using the
normal Rust installer, and **all 108 architecture tests plus the actual
repository scan passed**. This supersedes the earlier missing-Cargo architecture
limitation, without changing that historical result. Gateway compilation and four scripted Chromium groups subsequently passed, as recorded below. WebKit remains unavailable.

The inspector diagnostic was pointer targeting, not a missing binding: the
composite RevokeOwned state had its detail control, but its center hit the
unexplained AwaitingAck child rectangle. That center click opened no inspector;
keyboard activation of the composite control opened the correct section. The
published chart binds that section to the acknowledgement leaf instead, so its
clickable + has an unambiguous target. [Diagnostic](detail-hit-target.json).
The final published bindings were opened individually in the final 45-binding pass.

## Gateway-backed Chromium checks after toolchain installation

At source `7f434845423686cdd01a17300c47d10d46911f50`, the real gateway
built successfully with Rust 1.99.1 (one existing constant-evaluation warning).
The signed-out scripted command then ran with installed Chromium through the
same launch-only preload used above. It changed no application source or test
assertions and used no live provider account.

**All four Chromium groups passed:** resize-observers, mcp-apps-gateway,
gateway-window and scripted-scenarios (permission, failed-turn and cancellation
paths). Each WebKit launch could not run because its executable is absent.
The command's aggregate remains **could-not-run, exit 2**, not a full two-engine
pass. Default/raised conversation stress, chaos, native macOS and full desktop
run-all remain unrun.

[Runner summary](scripted-e2e-summary.md) · [Compact per-engine counts](scripted-e2e.json).
This supersedes the missing-Cargo limitation for the Chromium scripted gate,
without changing the earlier failed attempt or implying WebKit coverage.

## Settings journey review corrections

The independent documentation review identified three missing journeys. The workspace map now opens source-grounded pages for stored MCP integration management, native harness hooks and waiting-review notice permission. They describe local admission, effects, acknowledgements, failure/retry and retirement limits, without treating MCP Apps as management coverage. All three reviewed pages checkpoint source `7f434845423686cdd01a17300c47d10d46911f50`.

The existing focused settings suite passed **226/226 tests in seven files**: MCP model, gateway and rendered tab; native-hook adapter and rows; review-notice rows and gateway notice owner. This follow-up adds documentation, not new runtime tests. Live remote consent, next-start harness effects, real OS prompts, repeated native toggles and late prompt/write settlement remain explicitly unverified. The architecture enrollment summary was reconciled with the source-backed Approved → Staging → Active publication sequence; Rust activation tests were read, not newly executed in this pass.

```sh
pnpm exec vitest run \
  src/desktop/settings/model/mcp-servers.test.ts \
  src/desktop/settings/adapters/mcp-servers-gateway.test.ts \
  src/desktop/settings/ui/integrations-tab.test.tsx \
  src/desktop/settings/adapters/native-hooks-gateway.test.ts \
  src/desktop/settings/ui/native-hooks-rows.test.tsx \
  src/desktop/settings/ui/review-notice-rows.test.tsx \
  src/desktop/workspace/adapters/gateway/review-notices.test.ts
```

The focused run retained a React list-key warning from `ServerRow`; all seven
files passed. No assertion or rendered runtime source was changed to silence it.

## Manual two-engine qualification follow-up

Source commit `0d30b39e440c97523dd925ba90e187fd92da8b9d` adds the ordering
script to the existing manual `canonical-browser-verification.yml` job. It
requests Chromium and WebKit with the pinned bundled browsers, writes its JSON
and screenshots below the same provenance/artifact directory, and propagates
nonzero exit status. No always-on browser job, permissions change or runtime
change was added. Existing state-page behavior checkpoints remain at the source
commits actually reviewed.

Local validation: **8/8 provenance tests**, YAML parsing, shell syntax, manual-only
trigger/read-only permission and artifact assertions, script help and workflow
formatting passed. The new workflow still needs exact-head CI and independent
review. The parent reported ordinary CI terminal green at the preceding
`7f434845423686cdd01a17300c47d10d46911f50`; that does not supply WebKit evidence.

Dispatch attempted with `gh workflow run canonical-browser-verification.yml`,
repository `nessalabs/nessa-agent`, ref `codex/1087-user-flow-regressions`,
`source=0d30b39e440c97523dd925ba90e187fd92da8b9d`, and empty `base`.
**Denied:** CLI exited 1 during
`GET https://api.github.com/repos/nessalabs/nessa-agent/actions/workflows/canonical-browser-verification.yml`
with `Forbidden`. It did not reach the dispatch POST; no manual run was started.
No retry or alternate dispatch route was attempted. Both-engine ordering and
canonical scripted acceptance remain pending and block merge readiness.
