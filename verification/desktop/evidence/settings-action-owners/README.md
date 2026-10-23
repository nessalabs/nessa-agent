# Settings action owners (#1087)

Base main: `0b8019cf45f99b285f10c41199875d474492c6d1`.
Follow-up branch: `codex/1087-settings-action-owners`.
Runtime source: `c988ee98fae2517adfd0fae33fd422deb915856f`. Linux, Node 24.19.0, pinned Nessa UI
`d550fac7a59b8f9b6c9d6f1e7dced07ca38ee35b`, system `/usr/bin/chromium`.
No production account, native harness or visible OS permission prompt was used.

The minimal four independent reproductions against the original components
fail **4/16**, with the other **12 passing**. They count duplicate dispatch and
retired-source effects at component boundaries. Fixed owners passed all 16;
expanded cases cover ownership through mandatory rereads, focus deferral,
unmount/replacement, matching acknowledgements, permission rejection/retry,
current replay and stale-read isolation. Existing assertions remain unchanged.

The browser fixture imports actual NativeHooksRows and ReviewNoticeRows. Its
Notification object is controlled; browser-adapter invocation is measured, not
OS prompt display. Provider replacement is a deliberate test ordering whose
ordinary production reachability remains unproven. Same-task native callbacks
remain qualified separately from ordinary human clicks.

```sh
node verification/desktop/scripts/settings-action-owners.mjs \
  --engine chromium,webkit --channel bundled \
  --out /tmp/settings-action-owners.json --shots /tmp/settings-action-owners
```

Local Chromium uses the supported `--chromium-executable /usr/bin/chromium`
option and a dev server for this worktree. This does not replace WebKit evidence.
The prior browser download and manual workflow lookup were denied; neither is
retried in this follow-up. No two-engine pass or merge readiness is claimed.


## Results at the runtime checkpoint

- Expanded components: **28/28 passed**, including **16 added cases**. Restoring
  only the two original owners gives **9 failed / 19 passed**. Fixed sources
  restored afterward.
- Settings plus gateway notice owner: **354/354 passed in 16 files**. The existing
  MCP `ServerRow` list-key warning remains; its source is unchanged.
- Shared verification library: **401/401 passed**.
- TypeScript, changed-file ESLint/Prettier, production build and the actual
  repository architecture scanner passed. The build retains large-chunk and
  existing static/dynamic private-file import warnings.
- Fixed Chromium: **6/6 held**. The explicit Chromium/WebKit command retains those
  six results, then cannot launch absent WebKit: **6/7 held, exit 2**
  ([JSON](browser-final.json)). This remains an incomplete browser gate.
- Original owners in Chromium: **2/6 held, exit 1**
  ([JSON](browser-original.json)); all four behavior checks fail, including the
  replacement source that never starts its initial read.

No full frontend suite, full desktop run-all, real OS prompt/native harness,
canonical scripted suite or manual hosted browser workflow was run for this
follow-up head. Existing gate17 and gate19 requirements are not replaced by these
focused tests. No Rust, SDK, transport, authorization policy or provider behavior
was changed. Independent review and exact-head CI remain pending; the parent is
the sole merge owner.

The initial attempt to use `pnpm exec` in this borrowed dependency worktree tried
an automatic install and failed before running tests (`mkdir /home/agent/.local/share/pnpm`,
ENOENT). Tests were then run through the existing installed Vitest binary. A local
workspace-client link was corrected to this checkout before TypeScript passed;
no tracked dependency configuration changed. The first architecture attempt
lacked the already-installed Rust environment; supplying its existing `/tmp`
CARGO_HOME/RUSTUP_HOME made the actual scan pass. No permission or security gate
was weakened.

## Broader Chromium diagnostics

The shared smoke script held **16/18** results (both layout behavior groups pass,
both console checks fail). The selected existing settings-page script held
**5/10** (five behavior checks pass, five console checks fail). Both exit 1.
[Smoke](smoke.json) and [settings checks](settings-page.json) retain the failures:
Vite returned 403 for Geist/Geist Mono font requests whose borrowed dependency
links resolved under the other worktree. No Vite access setting, console filter
or assertion was changed, and no resource-denial retry was attempted. These
are failed broader gates, not passing browser acceptance. A launch-only preload
selected installed Chromium for these shared scripts; it changed no browser
security option or application behavior.

## Documentation checks

Nessa Docs accepted **120 pages / zero catalog issues**. Chromium rendered
**four diagrams across all three changed state pages**, opened **seven detail
bindings**, and followed **four drill-down paths** with zero page errors.
All **33 relative Markdown links** in the changed state pages resolve.
[Catalog](catalog.json) · [Viewer checks](statecharts.json) ·
[Statechart screenshot](statecharts.png).

The follow-up map is independent of #1093 and is based on main. It adds the
pending-action owner page, linked from workspace and the existing Settings
journey. #1093's three broader settings pages remain frozen at their reviewed
checkpoints. On assembling the PRs, their descriptions of previously unguarded
pending actions must link to this newer owner; they must not be treated as
claims about the later source. No reviewed branch was overwritten here.

## Individual rule mutations

All seven ownership guards were removed one at a time after asserting the edit
landed; every mutation failed at least one component regression. The fixed file
was restored with a fresh write between probes and after the run. The rules are
synchronous native admission, ownership before/after its reread, native cleanup
retirement, synchronous Allow admission, Allow request identity and currently
bound access-source checking. [Exact mutation results](mutations.json).
