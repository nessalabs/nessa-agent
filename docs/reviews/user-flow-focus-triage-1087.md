# Separate system-Chromium focus triage (#1087)

This is an open verification finding, not a defect attributed to the pending
user-flow fixes. The independent audit first reported an overview-focused
15/17 result. This broader local quick run is a different check selection and
must not be substituted for that result.

## Checkout and environment

The run started on `11e2746f7df088a1bd5ee3b2e579ad8a40b5cbd1` with the
permission follow-up uncommitted. That runtime source became
`4d718da047acbcf200c10feab62cb45b097ee939`. The diagnostic crossed a normal
branch checkout and fast-forward, which could trigger Vite reload; it is not
frozen-tree verification. Linux, pinned
Nessa UI `d550fac7a59b8f9b6c9d6f1e7dced07ca38ee35b`, Vite development,
installed `/usr/bin/chromium`, columns layout, `--quick`, `--mash 0`.
A temporary launcher preloader supplied that installed executable to the existing
shared browser launcher. It changed no application source or assertions.

```sh
node --import /tmp/nessa-system-chromium.mjs \
  verification/desktop/scripts/focus.mjs \
  --engine chromium --layout columns --mash 0 --quick \
  --out /tmp/nessa-focus-triage.json
```

The preloader only imports the checkout's Playwright `chromium`, retains its
original `launch` method, and supplies `executablePath: '/usr/bin/chromium'`
with the channel unset. No browser download or alternate network route is used.

Run: 2026-10-23 07:37:55–07:40:11 UTC. The first part overlapped the final
Vitest run (07:35:17 plus 183.41 seconds). This was a quick diagnostic under
contention and possible reload, not a browser handoff gate, frozen-tree
comparison or performance measurement. A second quick run is being captured
with the source checkout frozen at `4d718da047acbcf200c10feab62cb45b097ee939`.

## Observed result

**16/21 held; 2 assertion failures and 3 could-not-run results.**

| Check | Observation |
| --- | --- |
| `focus-panes` | After new-session-beside, caret remained in pane 1 while pane 2 was focused. After split-down, caret remained in pane 4 while pane 6 was focused. |
| `focus-bound-keys` | Ordinary split command stopped working after the bound-key exercise. |
| `focus-overview` | Could not run: overview list did not finish drawing. |
| `focus-retired-layout` | Could not run: pane movement queued 5 frames; expected one settlement. |
| `focus-regroup-peek` | Could not run: Escape in the reply pill left the overview. |

The detailed result is retained in
[the diagnostic JSON](../../verification/desktop/evidence/user-flow-ordering/focus-triage.json).
No assertion was weakened, no focus owner was changed, and this run is not a
pass. Default/raised stress and the full Chromium/WebKit desktop gate remain
unrun here.

## Scope and disposition

The diff from source baseline `0b8019cf45f99b285f10c41199875d474492c6d1`
to `4d718da047acbcf200c10feab62cb45b097ee939` contains no implementation
changes under `src/desktop/workspace` or `src/desktop/split-panes`. That is scope
evidence, not proof of causation or innocence: transitive behavior and test
environment still require investigation.

Coordinator follow-up: retain the audit's original 15/17 command and failure
names separately, reproduce the failing cases without contention against the
same baseline and head under supported engines, then assign any demonstrated
focus defect to its owner. Native macOS confirmation remains necessary for a
WebKit-only finding. This triage neither closes that work nor establishes an
exhaustive focus regression audit.

## Frozen-source diagnostic

A second run used source `4d718da047acbcf200c10feab62cb45b097ee939`
without source checkout or concurrent unit/build work, 07:45:41–07:47:59 UTC.
It finished **18/21 held: 2 assertion failures and 1 could-not-run**.
`focus-panes` and `focus-bound-keys` retained the same failures above;
`focus-retired-layout` still reported 5 queued frames instead of one.
`focus-overview` and `focus-regroup-peek` passed on this run. This remains a
quick system-Chromium diagnostic, not the full supported-engine gate or a
baseline-versus-head causation test.

[The frozen-run JSON](../../verification/desktop/evidence/user-flow-ordering/focus-frozen.json)
keeps its exact times and check details. The original audit's 15/17 result
remains separate. No focus implementation or assertion was changed.
