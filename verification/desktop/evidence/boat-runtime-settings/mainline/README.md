# Mainline integration verification

Reviewed source: `e213c1ccb370ced51a9f4ea794cd58441a0717c1`.
The integration inherited parent `3494d2aa8`; later parent `31237ca42` adds only
GitHub relay retention-gap decoding/tests/docs. Fresh synthetic-merge CI must
cover that parent delta. Local review of the lifetime integration does not prove
those new relay tests ran. This is verification evidence, not a deployment or
current-head CI pass. See PR #1174 for subsequent check results.

`source.json` keeps separate checkpoints. The Rust crates at reviewed source
are byte-identical to the gateway built at `f7a2a1f73`. The pane CSS repair is
`0a3fe97cb`, its focused verifier is `589bfa083`, and the client scope repair is
`e213c1ccb`. Earlier `b698f20053f8bb7c` receipts in the parent folder are historical.

## Real Boat workflow

The new workflow passed at backend `f7a2a1f73` using the parent's matching,
credential-free prebuilt worker `066217f7701b98b9`. Selected actual receipts are
in `provider-checks.json`:

| Transition | Receipt |
| --- | --- |
| Main and worker create under finite channel override | `ttlSeconds: 7200`, finite deadline |
| Archived worker resumes after explicit unlimited choice | `ttlSeconds: null`, no deadline |
| Active main reconnects after settings edit | Original finite deadline unchanged |
| New worker after channel override reset | Inherited `ttlSeconds: 14400` |

The same run verified subscription inference, workspace files across restart and
resume, an approved temporary local log, removal/session-close revocation,
coordinator delegation and missing-VM refusal without replacement or Local
fallback. No real user folders were shared. All four identities in
`provider-owned-vms.json` have independent **confirmed absent** provider receipts
in `provider-cleanup.json`. One unconfirmed conversation close after the
missing-VM case is retained separately; it is not proof of live teardown.
Temporary token files were removed and the existing user Boat configuration was
read-only. Raw gateway logs, configuration and credentials are excluded.

## Browser and client regressions

- Settings persistence/default/override/reset, invalid requests, label geometry,
  click reachability and focus: **4/4**, both engines and layouts at `e213c1ccb`.
  Actual fixture screenshots and settings readbacks are included.
- Raised channel stress: **9/9 each engine**, separate processes, seed 19,
  channels 3, parallel 3, turns 2, followers 2, rounds 6, at `e213c1ccb`.
- Pane clipping: **12/12** across both engines, layouts and three sizes. A bounded
  overflowing focus fixture leaves each root offset/header shift zero. Restoring
  the old hidden-overflow rule only in that test page fails **all 12** cases.
  The app source was not changed for the counterfactual.
- Full canonical Chromium stress: **53/53**, including the repaired window case
  and native paired catalogue. WebKit passes that window case, then fails GitHub
  keyboard navigation: **45/53**, with later steps explicitly not run. The
  GitHub-only WebKit check also fails (3/4); #1186 tracks it without an executed
  parent reproduction or a baseline-cause claim.
- Signed-out production scripted E2E at `f7a2a1f73`: pass in both engines for
  resize, gateway window and scenarios. The dev-only MCP app check is explicitly
  not run, not passed (`scripted-f7-summary.md`).

Review found a client request-scope defect: a non-enumerable or changing-accessor
channel could dispatch a different target before late response rejection.
`e213c1ccb` captures scope once and explicitly projects it into the outgoing
request. Four new read/write regressions fail before the fix and in its
intentional source revert; restored client 10/UI 6 tests pass16/16. A fresh reviewer
also passed nine valid/adversarial probes. See `review-dispositions.md`.

## Remaining verification and limits

The exact combined package repeat passed (server 3999 with 16 existing ignores),
all-target Clippy and gateway build passed. The initial restart-report failure
is preserved in #1181; isolated and repeat passes do not establish its cause.
Broad desktop verification at `e213c1ccb` and final-head CI are still running.
The earlier broad iteration at589 was intentionally stopped after the client
finding; its four completed groups are retained outside this folder and are not
final-tree full coverage. Performance has not been remeasured after pane clipping.
No whole-desktop, whole-WebKit-stress or overall-CI pass is claimed here.

Alpha remains Local/Boat only. Placement is fixed for the chat; no live migration
or serverless wake is added. Finite limits apply to active work and change only
on create/resume. The reachable gateway and explicit MCP permissions govern local
access; there is no automatic local-folder mount or copying Nessa's GitHub login
into remote Git. Backup describes validated export, not automatic channel restore.
