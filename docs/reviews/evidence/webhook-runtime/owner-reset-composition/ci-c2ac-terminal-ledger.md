# c2ac terminal acceptance ledger

Run [40262852364](https://github.com/nessalabs/nessa-agent/actions/runs/40262852364) is terminal **failure**. This ledger preserves the failed source; it does not qualify the later SQLite correction.

- PR head: `c2ac8c4fff3128a182d3d9e06f99218de36c63e3`.
- Actual checkout: `f9e906be369d7abddbad72893143283e4d95c798`.
- Actual main: `be8f53f80b7cd1fb7d49de508351e8c068238483`.
- Ten non-stress substantive jobs passed; stress and required aggregate failed.

| Required job | Result | Job ID |
| --- | --- | --- |
| Workflow files are valid | success | 121378733360 |
| What this change needs | success | 121378733526 |
| Conversation stress check | failure | 121378791425 |
| Windows server library tests | success | 121378791438 |
| frontend | success | 121378791439 |
| local-auth (macos-latest) | success | 121378791462 |
| local-auth (ubuntu-latest) | success | 121378791472 |
| local-auth (windows-latest) | success | 121378791479 |
| Desktop app compiles in release profile | success | 121378791484 |
| gateway-contract | success | 121378791549 |
| SDK domain coverage | success | 121378791608 |
| Required checks passed | failure | 121387644106 |

## Actual browser coverage

| Cohort | Actual result |
| --- | --- |
| Scripted Chromium/WebKit | 8/8 |
| Usage-limit Chromium/WebKit | 16/16 |
| Drag Chromium/WebKit | 8/8 |
| Full default, seed1 | 50/50; linkings1/pools1 retained; linking case002 and pool case033 passed |
| Default Chromium webhook | Case036 passed: Head46,46 queued,0 attempted |
| Focused WebKit webhook1, seed1 | 1/1 witness;4/4 report includes bookkeeping |
| Raised Chromium webhook3, seed1 | 1 passed;case2 SQLite busy error;case3 not run |
| Raised WebKit webhook3, seed1 | 3/3 witnesses |
| Raised report | 10/12 includes6 bookkeeping;only4/6 actual raised webhook witnesses passed |
| Menu-bar and GitHub focused WebKit | Both skipped after focused-step failure; no pass claimed |
| Chaos | 2/2 runs held |

## Platform and recipient invariants

Windows server:3,395passed/0failed/9ignored; macOS server:4,123passed/0failed/14ignored; Ubuntu server:4,125passed/0failed/11ignored. The corrected native lifecycle and all four full-retention controls passed on every server platform. Windows remaining native enrollment:111/111. Ubuntu native-window smoke passed with explicit gateway/driver/app/provider cleanup.

All38 selected controls below were individually matched to successful test lines in each of the three complete server logs, rather than inferred from job color:

| Boundary | Passed on each server platform |
| --- | --- |
| authentication | 3/3 |
| capability | 2/2 |
| recipient | 3/3 |
| replay | 7/7 |
| revocation | 5/5 |
| recovery cleanup | 5/5 |
| retention | 4/4 |
| fixture repairs | 2/2 |
| transient fairness | 2/2 |
| cancellation ownership | 5/5 |

Recipient controls include the actual public issuance/share path, only-granted-chat TLS POST, exact signed bytes and golden MAC vector, stable retry identity/body/signature, capability refusal with no cursor/delivery/POST, unshare/revoke/expiry/regrant and preparation races, observed-J reset replay after replacement/restart with ABA defense, known/unknown response recovery, retained cancellation ownership, physical shutdown joins, and unchanged retention caps. Browser localhost capture is separate no-attempt evidence; it is not the positive TLS recipient proof.

Frontend on actual f9e:5,307/5,307 in409files,build17.44s. Gateway/client/protocol:1,037 tests. SDK domain coverage:100% of reported regions/functions/lines. Ignored opt-in/platform probes are not represented as executed.

## Independent review closure

Whole-feature rounds1–3 and the exact round-three finding closure remain preserved. Subsequent precision/fixture/CI/main reviews are bounded affected reviews, not a new whole-feature round or counter reset. Actual f9e impact review is clean (original report SHA256`c35122ebfc1fb5fa4444ec74be788dcd4f5907d27ce8c6098edc28f8bbf73a46`). Original Windows lifecycle, Cargo-PATH, focus and unprepared-helper failures remain separately recorded.

Corrected-source automatic CI must execute the complete default and both focused webhook runs successfully, including Chromium cases2/3 and both previously skipped dependent WebKit steps. Pending/skipped work and this terminal failed run do not establish final acceptance. No merge is performed by this ledger.
