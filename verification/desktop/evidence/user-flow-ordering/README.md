# Pending action ordering evidence (#1087)

Base: `0b8019cf45f99b285f10c41199875d474492c6d1`.
Source head: recorded in the PR/handoff after committing. Linux, Node 24.19.0,
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
Gateway scripted E2E reported **could-not-run (exit 2)**: `cargo build -p
nessa-server` could not start because Cargo is absent. Default and raised
conversation stress, chaos, native macOS acceptance and live providers remain
unrun. Architecture scanning also cannot execute its Cargo metadata step.

The full frontend baseline's private-file acceptance failures are retained:
this sandbox reports `/` and `/tmp` owned by uid 65534, while the existing
private-file acquisition contract accepts root/current-user ancestors only.
Those tests and their security checks were not weakened.

Final source-tree checks: **129/129 targeted tests in 19 files**;
**401/401 verification-library tests**; TypeScript, changed-file ESLint,
source/fixture formatting and production build passed. Build retained its
existing large-chunk warning. Full frontend: **5130 passed, 2 failed in 390
files** (the same two unchanged private-file acceptance fixtures). Architecture:
**107 passed, 1 could not execute Cargo metadata**, reported by that test runner
as a failure. Required exact-head CI remains pending on GitHub.
