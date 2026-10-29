# Auth/native time precision in owner cursor and reset

Source `69351af72cbd73179f7f8765424251ecf4a20b99` corrects one production
comparison. Auth records issue seconds; the native registration retains its
original millisecond observation. The existing registration contract requires
Auth seconds to equal `observed_at_ms / 1000`, not milliseconds to equal seconds
multiplied by1000. Exact native milliseconds remain unchanged in registration
identity, reset receipts, capture generation and CAS. No authority, permission,
state, lock, retention or timeout contract changes.

## Preserved failure

[Run40252725334](https://github.com/nessalabs/nessa-agent/actions/runs/40252725334)
on0d0b12f32 failed stress step036 with `forbidden`, after35 steps passed.
[Original selected lines](ci-0d0b-stress-failure.txt) preserve that outcome and
the separate scripted8/8, usage-limit16/16, drag8/8 and chaos2/2 passes.

An isolated gateway probe used the same panel principal throughout. Authentication,
issuance, creation and share succeeded; cursor inspection refused. The existing
owner-query fixture was strengthened to use2345ms instead of a whole second.
The [old guard control](subsecond-old-guard.txt) failed with `forbidden`, exit101.
The correction accepts canonical2seconds and original native2345ms while retaining
the exact native timestamp assertion. The public reset/replay/signed-recipient
test also now uses2345ms; existing refusal and no-I/O assertions remain intact.

## Restored evidence

- [Complete webhook selection](subsecond-restored-controls.txt):295/295 under
  process-local umask022,19.15s, including all three retention operations.
- Required seven-package all-target Clippy passed with `-D warnings`.
- [Independent affected review](subsecond-review.md): no findings; independently
  executed six owner-query, four owner-reset and six timestamp/ownership/ABA
  controls. This does not reopen or reset whole-feature review rounds1–3.
- [Actual built gateway probe](subsecond-gateway-probe.txt): real local issuance,
  creation, scripted canonical turn, share, cursor and one observed Head succeeded;
  independent native rows proved25 queued/0 attempted and owned cleanup completed.
  Only synthetic disposable credentials, signed-out scripted agents and loopback
  were used. This probe has no browser or real recipient.

The first corrected probe queried immediately after creating an empty conversation
and returned `invalid_request` before a native cursor existed. It was not a pass.
The completed probe follows the stress contract: commit a scripted setup turn,
then use the existing15-second native-cursor installation/capture budgets. It
does not retry `forbidden` or change credential/grant authority.

The built-gateway probe is source/queue/no-attempt evidence; positive signed TLS
recipient delivery is covered separately by the complete product suite. Corrected
browser execution and exact published-head platform acceptance remain pending.
Raised webhook3 and webhook-specific WebKit are still outside the existing PR
stress configuration. No old result is relabeled as a corrected browser pass.

## Prior published run completed

The0d0b12f32 run completed without cancellation or rerun. Ten substantive jobs
passed; conversation stress and its required-check aggregate failed. The
[selected platform log lines](ci-0d0b-platform.txt) record:

| Platform/server suite | Passed | Failed | Ignored |
| --- | ---: | ---: | ---: |
| Ubuntu | 4125 | 0 | 11 |
| Mac | 4122 | 0 | 13 |
| Windows server | 3395 | 0 | 9 |

All three retention controls and the former cancellation-history failures passed
on each platform. Actual signed TLS/retry and failed-publication outcome controls
passed on Mac and Windows. Windows non-server qualification, required Clippy and
native enrollment110/110 passed. Ubuntu's native desktop window smoke passed
with owned process cleanup. Frontend passed5324/5324 tests across408 files and
built in17.02s. Gateway contract, SDK domain coverage and release compilation passed.

This qualifies the earlier fixture repairs at0d0b12f32. It does not qualify the
later69351af precision comparison or turn the failed default browser run into a
pass. Those need new published-head CI/browser evidence.
