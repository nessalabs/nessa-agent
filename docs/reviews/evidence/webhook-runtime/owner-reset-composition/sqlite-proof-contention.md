# Read-only no-attempt evidence under native writer contention

Original run: [40262852364](https://github.com/nessalabs/nessa-agent/actions/runs/40262852364),
stress job 121378791425. Published head:
`c2ac8c4fff3128a182d3d9e06f99218de36c63e3`; actual checkout:
`f9e906be369d7abddbad72893143283e4d95c798`, incorporating main
`be8f53f80b7cd1fb7d49de508351e8c068238483`.

```text
03:24:26 conversation-stress: 50/50 held (full default)
03:24:39 ok 001 webhook [webkit columns] (focused count1)
03:24:50 ok 001 webhook [chromium columns] (raised count3)
03:24:52 ERROR 002 webhook [chromium columns]
         database is locked · ERR_SQLITE_ERROR
03:24:54 ERROR 003 webhook [chromium columns]
         not run: 002 webhook did not hold
03:25:00 ok 001 webhook [webkit columns]
03:25:03 ok 002 webhook [webkit columns]
03:25:06 ok 003 webhook [webkit columns]
03:25:06 conversation-stress: 10/12 held (some could not run)
03:26:37 conversation-chaos: 2/2 runs held
```

The raised report includes six bookkeeping checks: only four of six raised
webhook witnesses passed. The separate focused WebKit count1 passed. Neither
that partial success nor default50/50 clears the Chromium count3 requirement.

The independent evidence helper opens native SQLite read-only and checks actual
queued rows, complete fixed scope/Head, capacity, zero attempts and absent attempt
timestamp. Node's default busy lock-wait is zero. A concurrent native writer can
therefore refuse this diagnostic read before its snapshot exists. The correction
sets this diagnostic connection's SQLite busy wait to1,000ms. Exhaustion still
throws; no missing/locked row is accepted as no-send evidence. Every existing row
assertion and finally-close remains unchanged. No production SQLite, journal,
flush, admission, UI-selection or workflow budget changes.

[Node's SQLite API](https://nodejs.org/api/sqlite.html) documents the connection
busy timeout and zero default; the CI and local verifier use Node24. A busy-handler
wait is not a universal wall-time deadline on all filesystem/OS operations.

Meaningful controls use an owned Worker with a real exclusive write transaction.
A writer kept locked must refuse. The same writer's delayed commit then permits
the actual queued snapshot. Restoring timeout0 reproduces ERR_SQLITE_ERROR at
that required successful read; the corrected helper is byte-restored afterward.
Independent review additionally commits a Sending/attempted row before release:
the waiting reader rejects the newly committed attempt, rather than accepting
an earlier queued value. Worker startup/release observations are bounded; owned
workers are terminated/joined and connections/temp fixtures close on failures.

Author focused controls:6/6; prepared-checkout full desktop helpers:408/408;
targeted formatting/ESLint passed. Independent focused controls:6/6, plus the
late-attempt rejection probe. Review has no findings; original report SHA-256:
`30419433162b7454542a7abd958c55dabf247653f4c7386afed7448960bc444e`.

The initial expanded-helper invocation in an unprepared author checkout failed
274pass/7fail because Playwright and Vite cache dependencies were absent. Its
outcome is preserved separately and is not a qualified helper pass. The existing
prepared checkout supplied the corrected408/408 result; no browser download or
installation workaround occurred. New-source browser/platform acceptance still
requires automatic CI. Domain transitions and statechart checkpoints do not
change for this fixture-only correction.

Complete required local `pnpm frontend:check` also passed:5,306/5,306 tests
across409 files,456 script tests plus2 skips,408 desktop helpers,108 architecture
checks,61 protocol controls, docs/types, lint/format, and production build15.47s.
The prepared runtime inputs equal authored c2ac plus these exact helper bytes:

- `gateway-stack.mjs`: `8ea1f7744d1e41c4986702cf9b7669dd5ca6c1c71d0a8dbf2347cfe54fa612d2`
- `gateway-stack.test.mjs`: `58d56f039f1e96a07687f3a5ea56865dc3bfe7ba751609170f4f728fdca7f5b0`

These local5,306 tests remain distinct from actual f9e CI's5,307 frontend tests
after the additional upstream phone changes. No old CI failure is relabeled as
a corrected browser pass.

## Shared final-capture deadline

The final correction additionally shares the existing15-second final cursor
catch-up timestamp with the SQLite proof. It never starts another interval after
catch-up. Before opening, preparing and stepping, lock acquisition is capped by
min(1,000ms, remaining original time); connection-local busy PRAGMAs re-budget
each phase without changing durable configuration. A final expiry check after
validation prevents accepting late proof. An expired/missing/nonfinite deadline
refuses, including after a cursor callback finishes late. This is not a promise
to hard-abort arbitrary underlying OS or filesystem work at an exact instant.

Seven focused controls and409 prepared helpers pass. Additional controls cover
an expired40ms lock budget, expiry during validation, and execution of the actual
webhook method consuming its catch-up interval before proof: it must refuse
before opening SQLite and still unshare/close. Four separate mutations—zero
lock-wait, renewed SQL budget, removed final expiry guard and renewed caller
budget—each fail the intended control; all bytes are restored. Independent
round2 repeats7/7 and the late committed-Sending rejection probe. Its
[report](sqlite-total-deadline-review.md) is clean (original SHA-256
`466e325aa1678ce8ab2e57d8c97dec9cd67541b8d6f111246a51aa88f10f6682`).
The earlier one-second-only source/review remains a historical intermediate
candidate, not the final deadline-qualified source.

The final shared-deadline source also passed the complete local frontend gate:
5,306/5,306 tests across409 files,456 scripts plus2 skips,409 desktop helpers,
108 architecture checks,61 protocol controls, docs/types, lint/format and
production build15.89s. The round2 manifest records all four exact code hashes;
all four mutation counterexamples were restored before this full check. This
local pass still does not establish corrected Chromium/WebKit acceptance.

The [terminal c2ac ledger](ci-c2ac-terminal-ledger.md) records all ten non-stress
jobs passing, all38 selected recipient/ownership controls passing on Windows,
macOS and Ubuntu, and the retained stress/aggregate failure. Menu-bar and GitHub
focused WebKit steps were skipped after that failure and need actual execution
on corrected-source CI.
