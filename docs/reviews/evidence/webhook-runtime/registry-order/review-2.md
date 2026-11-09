# Ordinary registry writer handoff — independent review round 2

Result: **clean: no findings at any priority**. Round-one m1 is closed. This is a scoped source/concurrency review, not completion of issue #896 or platform/merge qualification.

## Frozen source and scope

Reviewed clean checkout `/workspace/nessa-webhook-registry-order` at `da2146c0ee1ef731c4740284b330c26b9f26a956`. Owned source is `d99a7520532a86406f51853b37d8e9f62c71a0fe`, corrected regression `9bef5b428a6b987136544109f5bf8e286e29297f`, and the source-pinned state-page checkpoint. Immediate pre-fix parent is `e8ee15706c32212be56aeb83fa29edec9a1c8151`, integrating published `af9ea378` and main `d3dbf580`. The separate unpublished product-runtime draft is absent and excluded. Source remained unchanged during review.

Read AGENTS and the standards/local review gate. Inspected the complete production diff, regression and ordering/state documentation; followed adjacent registry bootstrap/issue/recovery/revoke, published access reads and effect holds, pairing create/decision/authorized commit/ordinary commit, owner-phone mutation, native physical-worker composition and local product administration. No independent test/build or CI run was launched and no shared build artifacts were changed.

## Findings and closure

There are no open findings. Round-one m1 required assertions for both worker starts and the original hold immediately before release. The revised regression collects both observations while held, requires their distinct `decision`/`expiry` labels, records and asserts the held state, and retains the no-early-result assertion. Assertions run after RAII release and all worker joins, preserving cleanup on diagnostic failures. The existing watchdogs and final actual expiry/cancellation/canonical-revision assertions remain intact. No test hook, retry, sleep or new lock framework was introduced.

## Agreement, authority and concurrency

- Ordinary synchronous writers now take the canonical registry mutex before a **nonblocking** effect-fence attempt. They serialize behind the preceding ordinary physical mutation rather than interpreting its active fence as a current read hold. A current read hold still causes `Locked`; no authority fence or epoch check is removed.
- `RegistrySlot` declares the effect fence before its canonical mutex guard. Field destruction releases the fence before handing off the mutex. Successful persistence, canonical replacement, published snapshot and revision publication precede that handoff; existing failure/health handling remains unchanged.
- Async revocation awaits its write fence without holding the registry mutex. It then retains both guards in `RegistrySlot` through the same `revoke_in_slot` implementation used by sync revocation. Original receipt replay, revocation time/cause, credential transition validation and durable publication are unchanged.
- The opposite acquisition orders do not form a waiting cycle: a synchronous holder never waits for an effect fence; it releases the canonical mutex on a failed try. An async writer can wait for that mutex, but does not await the fence while holding it. There is no added await inside a physical registry mutation.
- Effect-held snapshots and credential verification read the published registry rather than taking the canonical mutation mutex. Pairing admission revision/lifetime/identity/domain checks still run against the locked canonical state after serialization; a stale admitted mutation remains refused.
- Native owner/connection paths retain their existing admitted physical leases in blocking workers, including after observer loss. Local product administration likewise moves native lifecycle work off socket workers. The diff adds no detached task, nested worker, retry queue or new cancellation authority. It does not newly qualify arbitrary callers that ignore the physical-store calling contract.

## Evidence checked and remaining limits

Read the original synthetic probe (`Locked` translated to expiry/decision `Unavailable` while a real no-op expiry owns the mutation guard), the original regression failure (0 passed/1 failed), corrected pass, and strengthened regression pass (1/1). Read author receipts for all 215 Auth library tests, Auth all-target Clippy, 108 architecture tests and architecture check. These include existing real effect-hold/revocation and registry-lifetime controls.

The exact clean candidate's targeted Linux native enrollment test has now completed successfully: `peer_enroll::a_refused_or_wrong_class_invitation_saves_nothing`, 1 passed/0 failed, from `/workspace/nessa-webhook-registry-order` (`/tmp/nessa-896-pairing-order-exact-candidate-native-enrollment.log`). This is stronger source correlation than the earlier dirty-product-checkout run; neither is Windows/macOS qualification.

Verified source hashes: registry `e0d9eef05bc58a565451644c09f5d25eee267f98e9427e4c07bcd8637263727e`; pairing tests `436abb0ca64dd867bffcd2388d5d6a0ead2d3d0661fe6739808162d7d47f4863`. Independently compared the state page's linked sources with its preceding `f990b7df` checkpoint: only the registry and newly linked pairing test differ. Its `sourceRevision` names the exact corrected source commit, and Mermaid diagram bytes are unchanged.

The historical CI holder and particular cancellation site remain unknown. The reproduction establishes a source-backed contention mechanism, not retrospective identification of that event. Exact-head CI, Windows/macOS checks, source-matched browser/stress qualification, and assembled webhook source/sender/recipient/startup/shutdown/known-outcome coverage remain pending. No credentials, listeners, recipient transmissions, external state changes or merges were performed by this reviewer.
