# Ordinary registry writer handoff — independent review round 1

Result: **not clean: one minor test finding**. No production correctness, authority or deadlock defect was confirmed in the scoped locking change.

## Source and scope

Reviewed clean checkout `/workspace/nessa-webhook-registry-order`, commit `d99a7520532a86406f51853b37d8e9f62c71a0fe` against immediate parent `e8ee15706c32212be56aeb83fa29edec9a1c8151`. The parent preserves published `af9ea378` and main `d3dbf580`; the unpublished product-runtime draft is absent from this checkout and excluded from review.

Owned delta: `crates/nessa-auth/src/adapters/local/registry.rs`, the new contention regression in `registry/pairing/tests.rs`, and the Ordinary registry writer handoff table in `docs/design/webhook-delivery-implementation.md`. Read AGENTS, the local review gate/standards, architecture/structure guidance and adjacent Auth/pairing/native composition code. Shared source remained unchanged.

## Finding m1 — Readiness and continued hold observations must be asserted

Location: `crates/nessa-auth/src/adapters/local/registry/pairing/tests.rs:4000–4028` (the `started` observations, `held` diagnostic and release).

The regression receives both worker-start observations but only logs the `Result`s. It also logs, without asserting, whether the original expiry is still held immediately before explicit release. It subsequently asserts the lack of an early result and eventual authoritative outcomes. Therefore a scheduling failure can be logged as two `Timeout`s while a no-result timeout still satisfies the negative assertion; eventual operations can run only after release and pass. The fixture's own ten-second Clock watchdog can likewise end its hold independently of the explicit release, without a required hold observation rejecting that ordering.

The existing actual before-fix run is meaningful: both start observations were `Ok`, the holder remained held, and cancellation returned the expected erroneous `Unavailable`. This finding does not invalidate that recorded reproduction. It concerns what future executions of the regression enforce.

Expected: collect the two start observations and the held-state observation while the fixture is held, then assert **after releasing and joining all threads** that both starts succeeded, their labels are the distinct expiry/decision operations, and the holder was still held immediately before release. Keep the RAII release, current watchdogs, no-retry behavior and final typed/state assertions. No production hook or new lock abstraction is needed.

## Production relationships checked

- Ordinary mutation acquisition is canonical registry mutex followed by **nonblocking** effect-fence acquisition. A competing ordinary writer cannot inspect the prior writer's effect fence until the canonical mutex is released. `RegistrySlot` declares the fence before the mutex guard, so Rust field drop order releases that fence before handing off the mutex.
- Asynchronous revoke still awaits its effect fence without a canonical registry mutex. Once the fence exists, `revoke_fenced` constructs a `RegistrySlot` retaining both guards through `revoke_in_slot`, persistence, canonical replacement, snapshot and revision publication. The synchronous revoke entry uses the same slot and mutation implementation. Original revocation receipt/cause/time and credential transition logic is unchanged.
- The apparent opposite lock order does not establish a waiting cycle: a synchronous mutation owning the registry mutex only tries the effect fence and returns on failure; it never waits for that fence. An async revoke owning its fence may wait for the registry mutex, but any synchronous holder that encounters it releases its mutex on refusal. No new async await occurs while a registry guard is held.
- `AccessAdmission::hold` acquires a read fence then obtains its snapshot via the published registry reader. `AccessReader::read`/credential verification use that published read path rather than the canonical mutation mutex. Current read holds still cause the synchronous effect-fence attempt to refuse; no authority fence was removed. Releasing a finished writer's fence before its canonical guard does not expose an unpublished successful mutation: persistence and snapshot publication have already run, or the operation returned its existing failure.
- Followed bootstrap, issue/provision, recovery, synchronous/async revoke, `mutation_registry`, pairing create/decision/commit/authorized commit and owner-phone authorization. Canonical admission revision, credential validity and domain transition checks remain inside their locked mutation snapshots; a queued operation with stale admission still refuses rather than borrowing a fresh authority decision.
- Native owner commands use admitted leases and `spawn_blocking`; native connection work likewise retains its physical permit in `spawn_blocking`. Local product administration wraps native issue/list/revoke store work in blocking workers. The new change adds no worker, nested blocking task, retry queue or detached cleanup. Observer loss/cancellation does not alter those existing physical owners. Arbitrary external callers that ignore the existing physical-store calling contract were not newly qualified.

## Evidence and limits

Inspected the original synthetic contention probe and logs: a real no-op expiry held inside its Clock owns the original registry mutation guard; a raw mutation attempt reports `Locked`, translated expiry/decision report `Unavailable`, and operations succeed after release. Read the new regression's actual original failure (`0 passed / 1 failed`) and corrected result (`1 passed / 0 failed`). These establish a source-backed mechanism, **not** the identity of the holder or cancellation site in historical Ubuntu/macOS CI.

Read supplied results: 215 Auth library tests passed, including actual effect-hold/revocation and registry-owner lifetime controls; exact named native enrollment test passed 1/1 on Linux; Auth all-target Clippy passed; 108 architecture tests and the architecture check passed. Confirmed the reviewed Auth files are byte-identical to those in the author-tested checkout (registry SHA256 `e0d9eef05bc58a565451644c09f5d25eee267f98e9427e4c07bcd8637263727e`; pairing tests `7916e8a10031d5a55092e5d20f99bfbfc0db1f11c533403ee0513a84d859d484`). The author's server test checkout also contains an unpublished product draft excluded here, so that run is not an exact assembled-tree qualification of this clean publication candidate.

No independent test/build, new listener, credential issuance, network effect, CI rerun or source mutation was performed during this review. Windows/macOS qualification, full exact-head CI, source-matched browser/stress and assembled webhook source/sender/recipient/lifecycle completion remain unverified. Fresh full review is required after the minor test disposition and final author checks.
