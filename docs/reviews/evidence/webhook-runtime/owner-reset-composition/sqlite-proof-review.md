# SQLite no-attempt diagnostic lock handling — independent affected review

Technical publication excerpt; original report SHA-256: `30419433162b7454542a7abd958c55dabf247653f4c7386afed7448960bc444e`.

Verdict: clean in this bounded affected scope. Blocker: none. Major: none. Minor: none. Nit: none. This is not a new whole-feature round, review-counter reset, or qualification of unexecuted browser/platform cases.

## Frozen bytes and scope

Author checkout `/workspace/nessa-webhook-delivery-review`, HEAD `c2ac8c4fff3128a182d3d9e06f99218de36c63e3`, plus exactly two dirty files:

| File | SHA256 |
| --- | --- |
| verification/desktop/scripts/lib/gateway-stack.mjs | 8ea1f7744d1e41c4986702cf9b7669dd5ca6c1c71d0a8dbf2347cfe54fa612d2 |
| verification/desktop/scripts/lib/gateway-stack.test.mjs | 58d56f039f1e96a07687f3a5ea56865dc3bfe7ba751609170f4f728fdca7f5b0 |

Hashes matched before/after review and matched the prepared execution checkout `/workspace/nessa-webhook-browser-prep`. Pre-existing untracked node_modules is excluded. No author/prepared source, historical report, branch, build artifact or CI setting was edited by this reviewer. A separate scratch-only adversarial probe was written under /tmp, importing the frozen prepared helper; only this new review report and that scratch probe were authored.

Read AGENTS/CODING_STANDARDS applicable review, agreement, resource/cleanup and evidence rules; inspected full two-file diff, helper/query/connection ownership and original/new assertions. A mechanical comparison proved that reverting only the connection timeout/comment restores the original helper file exactly. Removing only the Worker imports/block and async marker restores the original test file exactly. Thus all original row-count, fixed-Head, credential/conversation/scope, phase, attempts and timestamp assertions remain byte-unchanged. git diff --check passes.

## Relationships and adversarial checks

The change is only `{readOnly:true, timeout:1000}` on the independent native diagnostic connection. It adds a bounded SQLite busy-lock wait instead of interpreting immediate contention as failed evidence. Node's version-pinned [v24.19.0 SQLite API](https://github.com/nodejs/node/blob/v24.19.0/doc/api/sqlite.md) specifies read-only opening and a connection busy timeout in milliseconds, default0. This is lock acquisition waiting, not a new whole-operation/performance deadline or a guarantee against arbitrary filesystem stalls.

No production connection, SQL journal/transaction policy, permissions, delivery state machine, browser click or15second UI selection/catch-up budget changes. The helper continues to open read-only, acquire actual rows through one SELECT/all snapshot, validate every returned row and close the connection in finally on success, lock failure or validation rejection. Lock failure still throws; it never becomes missing evidence, cached success or queued/no-attempt proof. The SQL remains scoped to exact registration credential and checkpoint conversation, compares all six scope fields and checks nonempty capacity≤1024 and fixed Head coverage. Every selected row must remain Queued/attempts0/timestamp null.

The added fixture uses a separate Worker owning its own real connection and BEGIN EXCLUSIVE transaction. Parent readiness and release observations each use an abort-bounded2second message promise. While no release is requested, the helper must throw database-locked after its configured wait. Release is then requested before the second synchronous helper call; the Worker's independent thread waits200ms and commits/closes while the parent is blocked, avoiding a same-event-loop release deadlock. The second call must obtain and validate the real snapshot. finally awaits worker.terminate; the outer finally closes its original DB and removes the temporary fixture. Constructor/startup/worker-message failures flow into cleanup; no provider/browser/gateway is launched by these controls. These bounds are fixture/SQLite cooperation, not a universal hard-kill guarantee for uninterruptible OS work.

Independently challenged the late-release case beyond the new positive control: scratch probe `/tmp/nessa-896-sqlite-proof-late-release-probe.mjs` imports the actual prepared helper, starts a real exclusive Worker transaction that changes the row from Queued to Sending/attempts1, requests release after200ms, then invokes the synchronous helper. The helper waited and rejected `attempt admission`; an independent subsequent read confirmed attempts1 really committed. The worker was terminated/joined, DB closed and disposable fixture removed. Thus delayed acquisition does not return a prior queued projection or weaken post-wait state validation. No attempt/POST was performed; the Sending row was a synthetic SQLite fact in a disposable test database.

## Executed evidence

Node v24.19.0, prepared checkout with exact matching two-file hashes:

- `node --test verification/desktop/scripts/lib/gateway-stack.test.mjs` (session94769):6passed,0failed/skipped/cancelled; total1393.776085ms; real-lock control1263.710714ms. This includes permanent lock refusal, delayed writer release and all original invalid/missing/changed-state cases.
- `node /tmp/nessa-896-sqlite-proof-late-release-probe.mjs`: exit0; newly committed Sending/attempts1 rejected after actual lock release.
- Exact base-restoration comparisons, final hashes and git diff --check: pass.

Read author receipts, without repeating their suite/mutation: `/tmp/nessa-896-sqlite-proof-zero-wait-control.log` shows the timeout0 counterfactual fails the actual delayed-release positive call with database-locked/ERR_SQLITE_ERROR; `/tmp/nessa-896-sqlite-proof-prepared-helper-controls.log` records408/408 passing prepared helper controls. The earlier unprepared274pass/7fail missing-Playwright/Vite-cache attempt is not counted as qualification. Targeted lint/format results are author receipts, not independently rerun here. No full helper suite, Cargo build, browser cohort, CI rerun/dispatch or publication was performed by this reviewer.

## Qualification limits

Supplied CI40262852364 context: full-default50/50 and focused WebKit1 passed; raised Chromium2 failed with database-locked, Chromium3 did not run, Chromium1 and all three WebKit witnesses passed. That is4/6 raised witnesses, not12; report10/12 includes six bookkeeping entries. This local repair does not turn that failed remote run into a pass. Corrected-source future CI and remaining platform outcomes are still required; no current-head full browser/raised-cohort or platform success is asserted. Existing whole-feature reviews and integration closures remain unchanged.
