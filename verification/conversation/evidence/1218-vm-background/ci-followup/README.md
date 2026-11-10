# CI qualification follow-up (#1218)

Source checkpoint: `b957e8bdf5225ce6aabe3e6c7bf2228022851603`, based on current
main `8fc037044854579bc06f1c344a4b04d9b91f2bf6`. Later receipt, diagram-checkpoint
and screenshot changes do not alter the tested source. The original checkout
and the other agents' branches remain untouched. Active #1283 and #1285 diffs
were inspected; their changes were not overwritten. No PR merge or deployment occurred.

## Corrections

- `a8519d77b3`: retain the real runtime settings port in the VM fixture. Its old
  unlimited-only fake could not save settings and broke the channel step.
- `e36d9b60d`: the initialized test gateway's Devices installer sends the real
  CLI build, as its conversation installers already do. The gateway itself is
  a Rust test executable and cannot serve the CLI protocol. Normal production
  construction, digest/protocol checks, audit and authorization remain intact.
- `eb45d2fe3`: await the initial Boat account read before inspecting its controls.
  Main's UI audit added assertions while the row could still say Loading.
- `b957e8bdf`: await the expected table-answer preview, rather than an already
  existing list row that can retain its previous preview. The expected content
  and the existing UI audit assertions remain unchanged.

## Current checks

| Check | Result | Receipt |
| --- | --- | --- |
| Background lifecycle/transport | 37 passed; long-lived fixture ignored | `background.log` |
| Workspaces | 86 passed | `workspaces.log` |
| SSH adapter | 130 passed; opt-in real-host test ignored | `ssh-adapter.log` |
| CLI | 31 passed | `cli.log` |
| Actual helper pipe/socket binary regressions | 2 passed | `binary-tests.log` |
| Independent focused Node tests | 28 passed | `review-node.log` |
| Architecture tests/checker | 108 passed; checker passed | `architecture-tests.log`, `architecture.log` |
| Server clippy, all targets, warnings denied | passed | `clippy.log` |
| Rustfmt, driver ESLint and Prettier | passed | `rustfmt.log`, `driver-lint.log`, `driver-format.log` |
| Fresh WebKit channel + VM | 6/6 | `current-main-webkit.json` |
| VM at three repetitions per engine | 14/14, Chromium and WebKit | `current-main-vm-raised.json` |
| Complete CI-equivalent conversation plan, Chromium (`inputFiles 0`) | 70/70 | `full-reaped.json`, `full-reaped.log` |
| Actual Linux OpenSSH/helper checks | 6/6 | `loopback-background.log` |

The browser checks use columns at 1440×900, dev mode and bundled browsers. The
driver uses its first requested layout; these are not sidebar results. The full
run disables the private phone input-files scenario exactly as CI does. WebKit
uses workspace-owned runtime libraries and
`PLAYWRIGHT_SKIP_VALIDATE_HOST_REQUIREMENTS=1`; browser launch and assertions ran.

Independent source review against main `8fc0370` found no code issues. It traced
admission/target binding, preparation cancellation, workspace holds, bounded
outcome decoding, SSH identity/verification, EOF, interrupted/restart ownership,
Unix guards, fixture ports and the two readiness waits. The reviewer independently
ran `stress-vm-background.test.mjs`, `stress-plan.test.mjs` and
`stress-conversation.test.mjs` (28 passed); browsers and real SSH were not
independently rerun by that reviewer.

## Regression controls

| Case | Before or reverted | Fixed / restored | Receipts |
| --- | --- | --- | --- |
| Runtime settings owner | Old head: channel alone 5/5, enabling VM fixture 4/6 | Corrected channel + VM passes both engines separately | `channel-control.json`, `channel-vm-before.json`, `channel-vm-webkit.json` |
| Devices build source | Full old-base run reached VM, then SSH setup failed: 21/70 | Focused SSH + VM 6/6; current full plan passes | `full-chromium-repeat.json`, `ssh-vm-after.json` |
| Initial account read | Wait reverted: 4/6, Boat / Loading… / Connect… | Fixed 6/6, byte-exact restored 6/6 | `account-delay-{fixed,reverted,restored}.json`, `account-wait-mutant.diff` |
| List preview | First current-main full run: 4/70, preview still the question; isolated old wait: 4/7 | Fixed and restored probes 7/7 | `current-main-full.json`, `preview-{fixed,reverted,restored}.json` |

`account-delay-probe.py` runs a temporary copy of the real driver, forwarding
all requests/authentication and response contents unchanged, while delaying
browser Boat status replies by three seconds. The account-wait hunk was removed
from the owned checkout and restored byte for byte before the full checks.
Its restored SHA-256 is
`c924d7adb353a0ee68eb957d261c2897f18a2741f8045a23c108e3b770b5d932`.

`table-preview-probe.py` retains one actual list response before each table turn
and presents that older snapshot to the first preview read, then reads normally.
This models projection lag for the consumer; it does not claim the gateway
returned stale data in that controlled run. No expected answer is fabricated.
`--revert-preview-wait` removes only the wait fix in the temporary driver copy
and prints its diff. The canonical driver remains unchanged while other checks run.

```sh
CARGO_INCREMENTAL=0 PLAYWRIGHT_SKIP_VALIDATE_HOST_REQUIREMENTS=1 python3 verification/conversation/evidence/1218-vm-background/ci-followup/account-delay-probe.py --only channel,vmBackground --engine chromium --layout columns --channel bundled
CARGO_INCREMENTAL=0 PLAYWRIGHT_SKIP_VALIDATE_HOST_REQUIREMENTS=1 python3 verification/conversation/evidence/1218-vm-background/ci-followup/table-preview-probe.py --only table --tables 3 --engine chromium --layout columns --channel bundled
```

An earlier old-base full run returned `pool_unavailable` (17/70). Pool-only,
pool/VM, channel/pool/VM and MCP/channel/pool controls passed, including the same
preceding-step order without the VM fixture. Full repeats passed pool, and the
current complete plan passes. The initial failure's cause remains unresolved;
this receipt does not claim a product fix for it. See `full-chromium.json` and
the `pool-*`, `channel-pool-vm.json`, `mcp-pool-vm.json` and `prior-pool-*` controls.

A combined two-engine channel run selected channels retained from the first
engine (10/12). This also reproduces without the VM fixture (9/10). Separate
engine invocations avoid the unrelated channel-name collision. A proposed
separate correction is to include the engine in the two channel names; that
change was not made here. The driver's default complete plan selects Chromium.

## Managed-container process reaping

The direct current-main full run passed through step 038, then failed tunnel
shutdown: 42/70. The stopped cloudflared PID was a zombie (`Zs`, PPID 1), not
executing; this container's PID 1 had not reaped it. The existing test checks
`kill(pid, 0)`, which still finds a zombie. See `current-main-full-bare.json`
and `container-zombie.log`. No tunnel implementation or assertion was changed.

`with-subreaper.py` uses Linux `PR_SET_CHILD_SUBREAPER` for its own command
lineage. It reaps exited orphaned descendants, never signals them while the
command runs, preserves its exit status, and reports/cleans remaining children
only after command exit. It does not adopt unrelated agents' processes. A small
control reaped one orphan; the unchanged complete plan passed 70/70 and reaped
seven, with no live-descendant cleanup failure. This is local init behavior,
not weakened process-liveness evidence or a real cloudflared service.

```sh
CARGO_INCREMENTAL=0 PLAYWRIGHT_SKIP_VALIDATE_HOST_REQUIREMENTS=1 python3 verification/conversation/evidence/1218-vm-background/ci-followup/with-subreaper.py node verification/desktop/scripts/conversation-stress.mjs --inputFiles 0 --engine chromium --layout columns --channel bundled
```

## Actual OpenSSH component evidence

`loopback-background.py` starts a disposable OpenSSH server bound only to
`127.0.0.1`, under the current account. It uses private temporary test keys,
`StrictModes yes`, an explicitly pinned host key and no user SSH config or agent.
Passwords and forwarding are disabled. Existing account SSH files are untouched.
The server is stopped, recorded processes are checked as exited, and the private
directory/keys are removed.

```sh
python3 verification/conversation/evidence/1218-vm-background/ci-followup/loopback-background.py target/debug/nessa "$HOME/.nessa-1218-ssh-tests"
```

The second argument must not exist and must be under a private account home.
StrictModes correctly refused the world-writable temporary parent and the
sandbox's root owned by another UID. Approved sandbox escalation allowed this
disposable private home path; StrictModes and host verification stayed enabled.

Exit, timeout and stop finish with stdin held open and return their actual
outcome with `cleanup_verified=true`. EOF returns cancelled with cleanup
confirmed and removes the observed command, child and grandchild. A separate
client-kill case observes all three alive, then all three gone before fallback
cleanup. A raw-shell negative control survives the same SSH loss and is then
explicitly cleaned. Linux zombies count as exited in these component probes.
Missing process identity is explicitly reported as cleanup unconfirmed.

This proves the observed Linux process tree on one host. It does not qualify
Boat, a separate VM/account, network partitions, every unobserved scope member,
the precise EOF-versus-signal cause in the client-kill case, or gateway settlement
over actual SSH. Interrupted settlement and restart are covered separately by
transport/store/fake-VM tests. The configured-real-host test remains ignored.

## CI and exclusions

[Run 42659676666](https://github.com/nessalabs/nessa-agent/actions/runs/42659676666)
at `7e05ddd` passed platform/SDK/frontend/gateway/release jobs but failed first at
channel runtime settings; later steps, including VM, were skipped.
[Run 42731427993](https://github.com/nessalabs/nessa-agent/actions/runs/42731427993)
at `a8519d77b3` tested GitHub merge `979b39d319e7ddc3399fb6313c1cda63157a7feb`
(parents current main `8fc0370` and `a8519d77b3`). It passed the listed completed
non-stress jobs but failed first at the new Boat Loading assertion. These old
runs do not qualify the newly published head; its required CI remains separate.

Real Boat is unrun: the selected environment declares no secrets or outbound
identities, no Boat connector is available, and paid provisioning is prohibited.
No real service credentials, paid resources, PR merge or deployment were used.
Native macOS/Windows, minimum-Rust and a separate-host SSH teardown were not run
locally. Do not infer them from the Linux component probes.

The Nessa Docs review advances only the background page's source checkpoint.
Among its listed owners, only the optional transcript `retried` flag changed
since `8fb9087`; background tool identity and lifecycle remain the same. The
bounded model and historical modelCheck receipt were not refreshed or rerun.
No Nessa Docs renderer was available for visual chart inspection.
