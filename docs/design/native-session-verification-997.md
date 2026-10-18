# Native session verification clocks (#997)

The native refusal regression verifies the actual paired gateway, receiver epoch
change, physical retained cache and pinned status recheck. Its refusal assertion
requires the page and subsequent refused request to arrive before the read budget
expires. Host scheduling cannot establish that ordering. The same operation may
legitimately save catalogue progress and return an incomplete read instead.

Composition accepts the existing protocol `Clock` port. `NativeSession::open`
continues to supply the production monotonic clock; `open_with_clock` lets a host
provide that same elapsed-time dependency. Auth's wall clock, policy, credentials,
socket stop behavior and five-second read budget are unchanged.

The regression holds logical time still. A separate wall-time watchdog stops
the operation and fails the test if it stalls; expiry is never accepted as a
successful refusal. The budget regression advances logical time only after the
actual source reports entering its first record head, after catalogue persistence.
Its phase marker is an empty file under that test's disposable synthetic root.

| Row | Ordered evidence/events | Required outcome | Verification |
| --- | --- | --- | --- |
| N1 | Active pinned status; confirmed partial page; epoch change; next read refused | Exact Refused, own pinned recheck refused, no purge, partial history retained | Existing native refusal regression with controlled clock |
| N2 | Catalogue saved; first record head held; logical budget expires | Incomplete progress, no refusal recheck, no purge, catalogue retained | Native budget regression |
| N3 | Operation does not finish before independent wall watchdog | Signal stop, join watchdog, fail test | Private watchdog negative probe |
| N4 | N1 session closes; gateway stops; independent offline reopen | Same retained progress and facts, stale passive projection | Existing native refusal regression |

The first head hold uses a separate fixture mode. It does not change the
preparing retries or follow/watch behavior owned by #965. A controlled delay
already reproduces the original assertion's counterexample on main and the
backup candidate. That evidence does not identify the cause of the earlier
Windows run; focused Windows comparison remains separate diagnostic evidence.
