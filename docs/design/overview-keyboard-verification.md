# Incremental overview keyboard verification — issue 820

The verification probe is armed before the overview opens. Its browser callback
observes the eligible mounted prefix, applies End/Down/Home and returns the focus
and tab-stop measurements from that callback. A later completed render does not
replace those measurements. The original two browser calls could observe a
partial list and then find it complete before applying the keys.

`verification/desktop/scripts/lib/overview-keyboard.mjs` owns this probe;
`focus.mjs` runs it in both engines/layouts. This changes verification ordering,
not product keyboard behavior. The qualifying condition requires the focused row
to belong to the observed column. The bounded observation reports a completed
list without an eligible prefix as unavailable. Browser observation faults and timeouts remain verifier faults,
rather than being swallowed as absent rendering.

| Case | Expected result | Evidence |
| --- | --- | --- |
| Rendering completes before the opening-key RPC returns | Capture and act on the earlier qualifying prefix | Fast-completion page-port test |
| Render is complete before any eligible prefix | Could-not-run, no key measurements | Absent-prefix test |
| Opening key fails | Original failure remains observable | Opening-failure test |
| Browser observation fails or times out | Original verifier fault remains observable; no fabricated measurements | Observation-fault test |
| Own focused rows are not mounted yet | No keyboard action | Browser callback test |
| Actual incremental overview in either engine/layout | End/Down/Home align focus and tab stop with mounted rows | Full focus browser matrix |

The substitute models the driver boundary: the page completes a render between
browser RPCs. The actual UI keyboard handler is measured by the browser matrix,
not replaced by that substitute. Reverting arm order or splitting observation
from action makes the fast-completion regression fail.
