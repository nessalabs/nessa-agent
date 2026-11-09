# Cloud reconstruction, 2026-10-20

This is a reconstruction, not recovery of the missing final commit. GitHub and
`git ls-remote` identify PR #965's source as
`nessalabs/nessa-agent:codex/724-719-746-paired-readers` at
`46762c7d2bb14bb1030286eb4b5833f0d6105089`. The separately published candidate
`codex/719-724-746-paired-subscriptions` is recoverable at
`5f525d0b28c38dc89352744dbf817e7f365c3d3a`. GitHub reports no commit for
`768f7da7746a7115b5116a1cbb90e78d5b4f62a2`, and its named branch is absent.

The recovered candidate was merged with main
`6a278df920562700a25df832d0361ed7c036b20f`, retaining main's current contracts.
Only the combined imports in `conversation/application/ports.rs` conflicted.
Commit `4a3120e2d3eed25de87c14a2c2ab49b64ad4cb56` adds the reconstructed native
catalogue check and the follower-probe correction. `source-sha256.json` records
that immutable source, independently of this evidence-only commit.

The real `NativeDeviceSession` opens its protected product child using the
fixture's actual enrolled device, pin and credential. List, observation and
subscription results are checked against an independent set of granted active
identities. Hidden rows exceed the list bound. Replacement, revoke/restore,
archive/unarchive and reconnect exercise the actual metadata store, native
listener and product owner. A separate socket stress fixture drives mock-agent
turns, omitted rows, cursor pagination and seeded histories. The native fixture
is a separate gateway from the window stress gateway; its canonical step runs
while the window's background work remains active. It is not browser-native
credential enrollment or a native UI measurement.

The follower capacity probe retains the identities of its original view and
event subscriptions and observes termination of acquired temporary views. It
abandons an observation if real capacity changed. Stable over-admission still
fails. This changes neither the product capacity nor existing replay, event
comparison, EOF/hash, cancellation, deadlines or CI annotation checks.

## Verification

Linux cloud executor; Rust 1.99.1, Node 24.19.0, pnpm 11.9.0, locked Cargo/pnpm
inputs and the exact `nessa-ui-revision` were used. `CARGO_INCREMENTAL=0` and
zero debug information kept the build within available capacity. More than
24 GB remained free after qualification. The former workspace was not accessed.

| Check | Result |
| --- | --- |
| Native catalogue exact test, default cycle | 1 passed |
| Native catalogue through the stress subprocess helper, four cycles | 1 passed; all 4 cycles completed |
| Native helper compile preflight followed by its exact runtime check | Passed |
| Read-only online composition cohort | 37 passed, 1 existing generated-Swift host probe ignored |
| Device-client library | 81 passed |
| Product socket subscriptions, including seeded catalogue stress and raised load | 107 passed, 1 existing measurement ignored |
| Metadata-store cohort | 80 passed |
| Verification library, including follower and subprocess-failure regressions | 397 passed |
| Combined CI package-selection Clippy, all targets, `-D warnings` | Passed |
| Rust formatting; selected JavaScript ESLint and Prettier | Passed |
| Architecture; generated product protocol and parity checks | Passed |
| Full server library | **3399 passed, 1 failed, 10 ignored** |

The full server failure is
`agents::infrastructure::codex::tests::status_cleanup_removes_descendants_after_root_exit_and_after_timeout`:
`CleanupUnconfirmed` at the first unwrap. Both the fixture and implementation
are unchanged from current main, with SHA256 respectively
`7f3803848a667a6a8d86271cc8714e0ff5999cdc19d00bdc0c0f0220c7557740` and
`a5b99eb5bebf8e9e788d25833fba56d9150073f688548e158478d619f9e7a0b1`.
These also match the historical unchanged-base proof linked in PR #965.
Current main was not independently rebuilt here. This is a failed aggregate
gate, not a diagnosis or waiver. Container PID 1 is `tail`; the repository's
process-cleanup acceptance documentation describes the separate init/reaping
requirement, which was not changed in this task.

The follower correction's deliberate revert probe restores the committed old
probe: five adversarial tests fail; all seven pass after restoration. Native
filter fail-before/restored evidence is recorded separately in `checks.json`.
Raw command outputs are preserved under `/workspace/scratch/965-cloud-recovery/`.
Zero-matched test filters were discarded and replaced by actual nonempty runs;
none are counted in this table. Compile failures during fixture construction
are also excluded from test results.

## Remaining gates and publication

No independent review is inherited from the unavailable `768f7da7` commit. Its
owner reports review 1 at `b21b474c` with the acquired-view finding and review 2
at `768f7da7` clean, but cannot certify the exhaustive earlier ledger or unused
allowance. One explicit reconstructed-candidate review/correction cycle has
been requested through the coordinating thread. No additional review has been
started while that authorization is pending.

The canonical browser download for Chromium revision 1243 returned HTTP 403,
`Domain forbidden`, at the configured Playwright CDN. The installer retried
internally; no separate retry or alternate-host workaround was attempted.
WebKit installation was never reached. Playwright 1.63.0's published manifest
specifies Chromium 1243 / 153.0.8010.12 and WebKit 2359 / 26.6 on this Linux host
(macOS 14 has the package's explicit WebKit revision override).

`browser-acceptance.md` identifies the exact commands for a separately approved
executor with the pinned browsers already installed. None has run here.
The earlier intermittent MCP context failure remains unexplained. #746's
preparation/retry cause is also unresolved: `PREPARING_ATTEMPTS` remains four;
no speculative 32-attempt mitigation or compatibility migration was added.
Linux convergence and historical macOS results do not diagnose that cause.

Git connector reads and a dry-run push succeeded. CLI authentication is invalid;
the configured connector exposes no manual workflow-dispatch action. Source is
committed locally and bundled; a dry-run is not publication to PR #965. The
intended source branch must be updated explicitly, its exact remote SHA checked,
and current-head CI/review completed before any merge. Historical successful CI
at `46762c7d` is not current-candidate verification.

An attempted draft conversion was rejected by automatic approval review for lack
of specific authorization. The coordinator instructed that PR state stay
unchanged; no retry is needed or planned. No merge, deployment or live migration
has occurred.
