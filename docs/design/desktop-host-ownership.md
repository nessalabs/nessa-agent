# Desktop host ownership

Issue [#943](https://github.com/nessalabs/nessa-agent/issues/943) owns this change.
This is the implemented source contract; native installed-Mac acceptance remains #562 and
startup measurement remains #109.

## Boundary

The launchd registration lock serializes transactions, not desktop owners. Two
live bundles can each classify the other's runtime fingerprint as stale and
alternate replacements, cancelling admitted turns. One application-owned lease
must precede journal recovery and service effects, including settings publication
and automatic quit. It stays with the host and its admitted workers across failed
attempts, dropped callers, and deferred staging.

On macOS the lease is a private nonblocking file lock in the existing account-home
`Library/Application Support/Nessa/gateway-locks/<service-label>` directory.
The filename differs from the transaction lock. Stage and instance select the
existing service label; bundle path, runtime fingerprint and configurable data
root do not select another lock. A competing host does not adopt the other host's
runtime or change shared settings. It reports that another copy is running and
can explicitly retry after that copy exits.

The busy-acquisition retry window is five seconds, using nonblocking lock attempts
and injected monotonic waits. This does not bound or cancel filesystem IO. This accommodates Tauri 2.11.5's actual restart order
(`process::restart_macos_app`: spawn successor, then exit), without releasing the
old host's authority while work remains active. A surviving owner still causes a
busy refusal. An acquired lease lasts through its final application/worker owner;
release does not unlink the stable lock file. The native guard explicitly unlocks
before closing, so an inherited descriptor in a forked child does not prolong a
normal release. Abrupt exit and exec inheritance have separate process tests.

The existing registration transaction lock, exact retirement target, durable
retirement cause/actor, effect plans and audit acknowledgements keep their owners.
After the prior host exits, a successor acquires the lease and performs ordinary
recovery/reconciliation; explicit upgrades are not replaced by fingerprint ranking
or a downgrade heuristic. Linux/unsupported adapters retain their current behavior.

This coordinates participating builds. A binary predating the lease, or another
same-account program ignoring it, cannot be retroactively excluded by this lock.
No live service, installed bundle, Login Items setting or user data is changed by
cloud verification.

## Legacy-host release boundary

The original #943 incident included a binary predating this lease. That exact
mixed-version scenario is not fixed by this cooperative protocol. A legacy host
can ignore `desktop-host.lock` and still recover, publish configuration, reconcile
or stop the shared service. #943 stays open until the transition procedure and
residual risk below are accepted and native verification is recorded.

There is no scoped automatic exclusion using the current legacy protocol:

- Keeping `gateway-upgrade.lock` for a desktop lifetime would make legacy
  registration wait up to its existing 120-second deadline, while legacy recovery,
  settings and stop paths remain outside that transaction lock. It also requires
  changing the current host's ordinary registration locking; it is not complete
  host exclusion. See `macos/control.rs::lock_namespace` and the `GatewayHost`
  implementation in `macos.rs`.
- A process-list snapshot can support a warning, but cannot fence a later legacy
  launch. A missing legacy process at one instant is not a migration proof.
- A different launchd label alone does not isolate shared product ports, data,
  settings and clients. A new namespace/protocol would require a separately
  designed migration rather than silently splitting users' state here.

The smallest release decision is a one-time coordinated upgrade. Before the
first participating build manages this namespace, the user/operator must let
active turns settle and gracefully quit every legacy Nessa desktop host. They
then update the installed bundle and retained developer launch paths, ensure old
copies will not be launched manually or automatically, and launch one
participating build. Ordinary audited reconciliation remains responsible for
service upgrades; this procedure does not reset data, forcibly terminate
processes, or automatically alter Login Items or provider settings. Review of old
launch paths is a release prerequisite, not an operation performed by this PR.

If that boundary cannot be established, a same-namespace upgrade cannot be
advertised as safe: hold the rollout, or explicitly restrict alpha to a
coordinated clean installation and retain this legacy-upgrade blocker. An old
copy launched afterward reintroduces the original risk. Product acceptance of
that residual risk does not constitute a technical exclusion guarantee.

Native acceptance must distinguish the legacy-to-participating transition from
two participating hosts. Verify the coordinated transition, competing
participating bundles and restart/updater overlap independently. Signed installed
launch/Login Items (#562) and startup timing (#109) are separate unrun gates.
No native acceptance or release decision is claimed by the cloud checks.

## Ordering and evidence

| Row | Ordering | Required result |
| --- | --- | --- |
| H1 | A owns; B starts with another bundle/fingerprint/data root but same label | B is busy; no B journal open, recovery, registration or retirement |
| H2 | A owns; B changes Claude directory | No durable settings or native configuration write |
| H3 | A registration fails, then retries; B starts | A keeps its lease and retries; B cannot replace it |
| H4 | Two first operations in A race | One physical acquisition and one retained lease |
| H5 | A caller disappears/panics while accepted work continues | Ownership remains with the host/worker, not its waiter |
| H6 | Deferred publication or stop worker outlives the foreground host handle | Lease remains until the admitted worker finishes |
| H7 | A releases normally, or a separate process exits abruptly; B retries | B acquires the original lock file; ordinary reconciliation remains responsible for upgrades/audit |
| H8 | Stage or instance differs | Independent existing service labels do not contend |
| H9 | Private lock open/validation fails | Typed error, no fallback acquisition or service effect |
| H10 | A child inherits a descriptor; normal release or exec occurs | No unintended child-held ownership after normal release/exec |
| H11 | Restart successor tries before predecessor exits, including release during the final polling interval | Attempt acquisition once after the final wait; release in that interval can succeed, while a persistent owner yields busy without another wait |
| H12 | Acquisition fails or the waiter panics, then A retries | No fictitious retained lease; retry can acquire |
| H13 | Joined caller awaits a journal while ownership is refused | Publish the pre-journal failure to joined waiters; none await a journal that will not open |
| H14 | The first acquisition clock read panics while a journal waiter joins | Supervise deadline creation too; publish typed acquisition failure and settle that waiter |

Cloud checks establish the portable retained-lease owner and real Unix private-file
lock/process semantics. Actual two-host macOS registration, explicit updater restart,
Login Items, signed launch and installed-window behavior still need coordinated
native acceptance; synthetic tests do not close #562/#109.

The Unix process fixtures request mode 0700 when creating their temporary service
directories. `tempfile::tempdir()` otherwise uses 0777 masked by the process umask;
a restrictive local umask must not hide an unsafe fixture on ordinary CI accounts.
H9 also verifies that an existing 0755 service leaf is refused without permission
repair or lock-file creation. Qualification runs the same assertions under umasks
000, 022 and 077; it does not change the production private-storage boundary.
