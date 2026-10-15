# Privacy tabs verification (#905)

Recorded October 14, 2026 on macOS. Product/owner-script source:
`6936cf2fc6141b475090a85c27f68c7a5db0e59c`, main base
`abdaed37c1f0f89bdf819ae9debcc702e2705ad0`.
Later evidence commits add these records and images only.

## Scope and results

| Check | Environment | Result |
| --- | --- | --- |
| Settings owner script, all cases | Dev, Chromium and WebKit | 23/23 |
| Settings supported production cases | Prod, Chromium and WebKit | 17/17 |
| Full smoke | Prod, both engines, columns and sidebar | 33/33 |
| Settings safe area/sidebar return | Prod, both engines/layouts at owner-script sizes | 19/19 |
| One-hunk Access reinsertion | Dev, both engines | Masthead fails twice at y739 |
| Restored masthead | Dev, both engines | 3/3, Access bottom y587 |

At 1000×700 both engines measure the common Access tab's last row at y587,
below the existing 700px fold ceiling. Hook search lands on Native hooks with
Claude's row at y179–240 and Codex's at y240–300, in view. The production
arrival control records two 220ms animations against the existing 300ms
contract, and no animations under reduced motion. These are animation-duration
checks; they make no claim of improved frame performance.

The development run includes actual gateway-fixture on, unset and failed hook
states. The production Rollup inputs contain desktop/index, not those fixture
HTML entries, so production deliberately selects `masthead,condense,arrive,search,
review-notices,remembered-rules,native-hooks,focus-ring`. The earlier unfiltered
production attempt at `c7dc28080` holds 17/23 with six fixture cases unable to run;
its raw record is preserved as `unsupported-production-fixtures.json`.

The first dev attempt at `6936cf2fc` used prepared font links outside this
checkout's Vite filesystem allowlist: 22 functional cases held but 24 console
results failed with 403/cancelled font requests. `initial-font-environment-failure.json`
preserves it. Copying the prepared fonts into the checkout's ignored vendor
folder, using local font links and restarting Vite fixes that verification
setup. No product, budget, console allowlist or Vite configuration changed.

## Commands

The owned dev server was port 1495. A fresh Vite production build went to
`/tmp/nessa-905-prod-final`, previewed on 1496. Both servers are retired.

```sh
node_modules/.bin/vitest run --config vitest.config.ts src/desktop/settings
node_modules/.bin/tsc --noEmit
node_modules/.bin/eslint src/desktop/settings verification/desktop/scripts/settings-page.mjs verification/desktop/fixtures/native-hooks-on/main.tsx
node verification/desktop/scripts/settings-page.mjs --url http://127.0.0.1:1495/desktop.html --mode dev --engine chromium,webkit --out /tmp/nessa-905-settings-dev-final.json
node_modules/.bin/vite build --outDir /tmp/nessa-905-prod-final --emptyOutDir
node verification/desktop/scripts/settings-page.mjs --url http://127.0.0.1:1496/desktop.html --mode prod --engine chromium,webkit --only masthead,condense,arrive,search,review-notices,remembered-rules,native-hooks,focus-ring --out /tmp/nessa-905-settings-prod-final.json
node verification/desktop/scripts/smoke.mjs --url http://127.0.0.1:1496/desktop.html --mode prod --engine chromium,webkit --out /tmp/nessa-905-smoke-prod.json
node verification/desktop/scripts/safe-area.mjs --url http://127.0.0.1:1496/desktop.html --mode prod --engine chromium,webkit --only settings,settings-sidebar-return --out /tmp/nessa-905-safe-area-prod.json
```

The browser reinsertion/restoration uses the same Settings command with
`--only masthead`. Source-only edits were performed one at a time in the
owned checkout, with restoration before the next check. Scoped formatting
checks cover every edited TypeScript/script file; `git diff --check` passes.

## Reproduction and probes

`baseline-settings-masthead.json` is the unmodified owner script/product on
main `102cff8c4383b07e1fc34c695dabefa3c6d11c72`: both engines fail at y766.
The Settings source/verifier compare identical between that base and the PR's
main base `abdaed37c`. Source history identifies the native-hook and later
remembered-rule additions, but the first failing historical commit is unmeasured.

`reinsert-probe-source.diff` reinstates only `<NativeHooksRows />` on Access;
`reinsert-probe.json` shows both engines fail the unchanged fold contract at
1000×700, y739. The lower value than historical y766 reflects the already
removed redundant group heading. The exact final source was restored, touched
by writing it back, and the same masthead script passes in
`restored-masthead.json`.

Three independent unit probes also fail when hook catalogue entries move back
to Access, the component is reinserted on Access, or its redundant group heading
returns. Restored source `09b3f538a` passes all 312 Settings tests. TypeScript,
scoped ESLint and formatting pass; later source commits update comments and
screenshot filenames only. Fresh full source review at `6936cf2fc` has no findings.

## Screenshots and limits

The four `privacy-*` screenshots show the actual 1000×700 dev Settings surface
at 1× in each engine. Current state crops are in
`../settings-native-hooks/native-hooks-{resting,on,unset,failed}-{chromium,webkit}.png`;
resting crops are from the final production run and the other states from the
final dev fixtures, at the owner script's 2× scale. All twelve were visually
inspected for tab placement, complete warning/setup/failure copy and visible
controls. Historical `access-*` crops remain unchanged.

Combined #902+#905 full functional matrix, scripted end-to-end run, CI and
external review remain pending. The previously observed #902 MCP teardown
failure's original cause is unproved; these scoped checks do not diagnose it.
Native WKWebView, arbitrary gateway rule-list sizes and a historical first-bad
commit were not verified here.
