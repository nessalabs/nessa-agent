# Boat lifetime verification

Current source: `49080c0477f23e752bb7dd90dc80e093a9d12558`, based on PR #1158 after #1166 merged.
Its base merge tree is identical to the originally reviewed #1166 head
`33b0fc6534cc30507061665cbc7d3f04cce48e66`. macOS arm64, production browser
builds. Provider/backend evidence remains attributed to
`471fa51e1f92fad5e17ddd20bc5dee407de1cffb`; the later changes repair CI staging
and row hit testing, then restrict an unused getter to its Unix consumer.
Current browser receipts are from `29ef4e5985a2858bd0839de5e67050a5078d2a21`;
the later platform gate leaves Unix behavior unchanged. This records local
verification, not a deployed release or
green CI. `source.json` distinguishes the checkpoints.

## Real provider

`scripts/workspaces/boat-e2e.mjs` passed twice, including the final corrected
backend source, using the existing Boat account's connected
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

The original `boat-runtime-settings.mjs` matrix passed **5/5**: the four engine/layout combinations
and the reported console check. Each combination saved a finite global default,
read inherited channel settings, saved explicit unlimited, then reset the channel
to inheritance. Malformed/null/contradictory commands left the default unchanged.
The check also measured label clearance, click reachability and keyboard focus
return. JSON receipts and actual fixture screenshots are included.

After broad verification exposed a decorative click regression, the structural
row correction passed the production settings matrix **4/4** (`browser-matrix-current.json`)
and production row matrix **4/4** (`rows-current.json`). A pointer-rule revert
failed both decorative count and glyph click checks (`rows-revert.json`), then
the exact source was restored. An independent reviewer confirmed six actual
click/focus interactions across both engines (`review-row-clicks.json`). The
full repeated shared-controls group held **32/32**, and smoke held **32/32**.
`channel-current.png` and `default-current.png` show the corrected source.

The original shared helper also passed raised conversation stress in separate engine
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
reproduced on the unchanged base and tracked separately in #1172. The original
three-round source review closed clean at `471fa51e1f92fad5e17ddd20bc5dee407de1cffb`.
Broad verification then exposed the row interaction and isolated-cut omission;
the structural diagnosis, fixes and fresh clean review are recorded in `review.md`.
The current source passed the 20-test bare-Node CI harness, actual isolated
65-test protocol cut replay, app typecheck, ESLint, formatting, actionlint and
12 client/control tests. The Windows CI dead-code failure is retained in `windows-gate.json`. Its getter
now compiles only for its Unix consumer; portable parser tests retain the other
helpers. The local CI-package all-target Clippy selection and three parser tests
pass. Windows still requires the next-head CI result. No provider code or worker
image changed afterward.

The first broad run failed. Known tooltip/pane-focus/share failures are #1162,
linked-device focus is #1172, reduced-motion swaps are #1173. Additional unchanged-base
reproductions of committed-conversation, approval-deadline and turn-outcomes
fixture failures are #1175. `baseline-fixtures.json` records the exact base and
outcomes. Matching home-shape and 1000x700 safe-area transition failures reproduce on the
unchanged base (#1176, `baseline-layout.json`). Other columns/approval-list
failures still require isolation; they are not assumed to be baseline. Two gateway checks initially could not run
because their binary was not configured. The repeated full run sets the explicit
fresh gateway binary. Its settled outcome and current-head CI belong in PR #1174;
these local feature checks do not make that broad suite green.
