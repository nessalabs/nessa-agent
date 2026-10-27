# Boat lifetime verification

Final source: `471fa51e1f92fad5e17ddd20bc5dee407de1cffb`, based on PR #1166
head `33b0fc6534cc30507061665cbc7d3f04cce48e66`. macOS arm64, production browser
builds. Browser evidence is from `34672659bb87d44ad9f390e9725830bfe500e7db`;
UI implementation has not changed since that checkpoint. This records local
verification, not a deployed release or green CI.

## Real provider

`scripts/workspaces/boat-e2e.mjs` passed twice, including the final corrected
source above, using the existing Boat account's connected
Claude subscription and the unchanged prebuilt worker protocol
`b698f20053f8bb7c`. `provider-checks.json` contains selected actual provider
receipt fields:

| Transition | Actual provider result |
| --- | --- |
| Main and worker created with channel override | `ttlSeconds: 7200`, finite `archiveAfter` |
| Archived worker resumed after explicit unlimited edit | `ttlSeconds: null`, `archiveAfter: null` |
| Existing active main reconnected after that edit | Original `7200` and identical deadline |
| Channel reset before creating another worker | Inherited global `ttlSeconds: 14400` |

The same run verified distinct VMs, subscription inference, saved workspace files
after gateway restart and VM resume, one approved local fixture log, revocation
after removing/closing that connection, coordinator delegation and missing-VM
refusal without replacement or Local fallback. No real user folders were shared.

All four owned VMs are **confirmed absent** in `provider-cleanup.json`. That file
also retains one unconfirmed conversation-close result after the missing-VM
scenario; it does not claim successful live connection teardown from that result.
Provider absence was checked separately. The isolated gateway then shut down.
Temporary token files were deleted; the user's existing Boat config was preserved.
No raw credentials, configuration files or gateway logs are included here.

## Real browser

`boat-runtime-settings.mjs` passed **5/5**: the four engine/layout combinations
and the reported console check. Each combination saved a finite global default,
read inherited channel settings, saved explicit unlimited, then reset the channel
to inheritance. Malformed/null/contradictory commands left the default unchanged.
The check also measured label clearance, click reachability and keyboard focus
return. JSON receipts and actual fixture screenshots are included.

Its shared helper also passed raised conversation stress in separate engine
processes: **9/9 Chromium and 9/9 WebKit**, seed 19, channels 3, parallel 3,
followers 2, turns 2, rounds 6. These results do not claim the broader desktop suite
passed; its baseline failures are reported separately in the handoff.

## Other verification

The final CI package selection passed locally, including 3907 server tests with 15
existing ignores, and all-target Clippy. Protocol generation/checks passed 65
tests, including owner/schema mutation refusal. Focused configuration, audit,
allocation and scope tests passed. Intentional source revert probes that ignored
the channel override or removed the scope guard failed their regression tests,
then the restored workspace suite passed 25/25.

The full frontend suite had 5331 passed and one linked-device focus failure,
reproduced on the unchanged base and tracked separately in #1172. The three-round
independent source review closed clean at `471fa51e1f92fad5e17ddd20bc5dee407de1cffb`;
see `review.md` for findings, fixes and exclusions. The final corrected-source
package/provider repeat passed. Broad browser verification is still running;
its observed tooltip/pane-focus/share fixtures are tracked in #1162, linked-device
focus in #1172, and reduced-motion swaps in #1173. The drag failure was reproduced
against an isolated unchanged-base dev server (6/9 results held), not the reused
feature server. Do not call that broad suite green.
