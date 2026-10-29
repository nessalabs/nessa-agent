# CI follow-up for #1158

Hosted run [40442141482](https://github.com/nessalabs/nessa-agent/actions/runs/40442141482)
checked `6861bf2ec803a78a9464fe9647db9c7e5e37a206`. It completed with three
underlying failures and the resulting aggregate failure. All other jobs passed,
including macOS/Linux integration, Linux native desktop smoke, Windows server
library tests, release compilation, gateway contracts and SDK domain coverage.

## Related failures repaired

1. Frontend: `scripts/ci-result.test.mjs` still expected the pre-vault package
   list. `1f5236580` aligns it with the existing two CI partition commands.
   **41** aggregation/partition tests pass, including an independent review run.
2. Stress: `032 notice` reloaded the window after an earlier gateway restart.
   The genuine vault dialog intercepted a later ordinary click. `07430be40`
   waits for the required unlock dialog based on backend state, enters the
   disposable passphrase through the UI, and verifies unlocked state. The same
   handling covers launch reloads. No permission/deadline/capacity was relaxed;
   no forced click, dialog dismissal, direct unlock RPC or skipped oracle was added.
   Before: minimized seed-2 pool→notice sequence **4/7 held**, reproducing the
   intercepted click and automatic-denial aftermath. After: **13/13 held** across
   Chromium/WebKit. Restart→launch also holds **13/13** across both engines.
3. Windows desktop: new vault composition called the existing `port()` getter,
   whose platform guard omitted Windows. Removing that guard exposes the same
   validated `u16` on every platform. Constructor validation and callers are
   unchanged. Native all-target strict Clippy and formatting pass on macOS;
   actual Windows compilation is left to the hosted rerun.

Each repair received independent focused review with no findings. These are CI
integration fixes; runtime vault crypto, access, lease and audit contracts are
unchanged. Statecharts do not need new transitions or checkpoints for an assertion,
a test user's reload actions, or accessor availability. The desktop verification
checklist records the reload regression.

## Second hosted run and approval-receipt repair

Hosted rerun [40446076479](https://github.com/nessalabs/nessa-agent/actions/runs/40446076479)
at `4c977dad81dc2f2ebd13607dff241c4149427c93` confirmed the prior fixes: frontend,
all platform jobs, Windows desktop/server compilation and other checks passed.
Its only underlying failure was stress step039 GitHub: the row became signed out
after approval. The resulting aggregate failure is not a second underlying fault.

A deterministic application test independently reproduced a matching race:
completion took terminal ownership but projected SignedOut before waiting for its
audit/storage receipts. A Settings poll could then stop before the later Connected
publication. `4ff387934` captures the failing regression; `e684e77f2` adds an
internal Saving projection that remains publicly Connecting until both receipts.
The precise hosted interleaving was not observed directly; the source contract
violation was independently falsified and repaired.

The original completion still owns terminal evidence. Saving owns no second poll
or audit, and remains generation-bound. Lock/reopen and queued disconnect tests
cover both receipt boundaries, alongside existing failure/rollback and real-store
fence tests. See [independent implementation review](saving-review.md).

Verified on the repaired source:

- GitHub/relay/product tests in the eight-package CI selection: **67 passed**.
- Strict all-target Clippy in the same package selection: **passed**.
- GitHub live-event browser checks, Chromium/WebKit: **3/3 held**.
- Full default Chromium conversation stress: **48/48 held**, including039 GitHub,
  at-rest automatic-denial checks, followers and console checks.
- Architecture, formatting and 134-page documentation validation: **passed**;
  the owning connection diagram parses. Only the affected state page advances
  its source checkpoint to the reviewed implementation commit.

No public wire contract, permission, deadline, capacity or Cloudflare plan changed.
The PR stays draft pending hosted verification of the new head. Historical failures
and the separately tracked observations below are retained, not waived or closed.

## Separate observations retained

- Full local script suite after the package-list repair: **455 passed, 1 failed,
  2 skipped**. The sole failure is the previously base-reproduced macOS path
  spelling assertion [#1157](https://github.com/nessalabs/nessa-agent/issues/1157).
- Full local Chromium stress: **17/48 held**, stopping earlier at unchanged
  `015 upgrade` with a missed reply/send failure and repeated subscription-capacity
  warnings. Neither repaired reload action had run yet. Evidence is noted on
  [#1038](https://github.com/nessalabs/nessa-agent/issues/1038); the relation to that
  issue's source-closure cause is unproven. This is not a green full-stress result.
- Frontend focus [#1154](https://github.com/nessalabs/nessa-agent/issues/1154),
  previously untriaged full desktop failures and native/platform verification
  limitations from [the original report](README.md) remain recorded.

The PR stays draft. A new hosted run must confirm the fixes, particularly the
Windows-only compile boundary. Cloudflare deployment and its Free plan are unchanged.
