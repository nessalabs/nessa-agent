# PR #1158 Daybreak defensive security review — final Codex-comment fixes

Reviewed checkout `/Users/nessa/.codex/worktrees/github-webhooks-1107/nessa-agent` as the dirty tree at local `c2ea1002a8e662f1591b13442f6bea0c553fd9a4`, against `b09cadf17b8bca97f0c3902a735ee4059d58a2e0`. Review was read-only; I did not edit the checkout, run Cargo, commit, or push.

The exact reviewed source and tests were committed as `eab6d335e064276a95c094d874174fbcbc51344b` after the review. The coordinator then confirmed `/tmp/nessa-1158-final-races.log` finished with both tests passing and reran the eight-package strict Clippy gate successfully in `/tmp/nessa-1158-final-clippy-frozen.log`. Hosted CI remains separate evidence.

## Findings

No findings at blocker, major, minor, or nit priority.

## Adversarial analysis

- G22 command admission: traced `product::github::dispatch` from the vault generation snapshot, through the product `changes` queue, the pre-effect recheck, live-selection call, post-selection recheck, and `GithubConnections` generation-scoped sign-in operations. A queued old-generation disconnect cannot reach either selection deletion or credential deletion after lock/unlock. Status/connect/cancel and the no-live-adapter path retain valid behavior.
- G23 selection ownership: traced `LiveEvents::set` through `LiveEventStore::select`, the SQLite `IMMEDIATE` transaction, deletion/enable/gap audit writes, event and binding changes, audit retention, `LiveSelectionAdmission`, `VaultSelectionAdmission`, and `Vault::within_generation`. The transaction is still uncommitted when generation is checked. A stale generation returns `VaultLocked`; dropping the transaction rolls back binding, event, and audit changes. Once admitted, the vault state mutex remains held through SQLite commit, so commit and a concurrent lock have a single order: lock-before-guard rejects and rolls back; guard-before-lock commits before the lock can publish its generation change.
- Tried to falsify the guarantee with: queued disconnect; stale explicit disable; stale disconnect cleanup; lock/unlock before the commit guard; lock requested during the guarded commit; adapter refusal without invoking the commit closure; SQLite commit failure; provider/identity/repository failure before selection; audit insertion failure; same user/repository restored with a new binding generation; unrelated/multiple binding rows; and a legitimate enable/disable with an unchanged generation. No stale path can commit, and valid current-generation paths still reach the one transaction owner.
- The injected trait cannot silently report success without committing: `selection.rs:48-58` retains the transaction in `pending` and returns `Storage` if the adapter does not invoke the closure. Production composition injects the vault-backed owner. The closure is synchronous and contains only `rusqlite::Transaction::commit`, so it cannot await or re-enter the vault.

## Agreement across fields and layers

- Authoritative unlock identity is `Vault::State::generation`; `Vault::access` enforces equality and lock state while holding the state mutex.
- Authoritative live selection and retained events are the SQLite `bindings`/`events` rows; `SqliteRelayStore::select` owns their replacement and the matching `audit`/`binding_audit` evidence in one transaction.
- Caller identity is serialized at the authenticated product boundary and carried unchanged as the audit initiator. GitHub user identity is rechecked around provider repository discovery before the transaction.
- The product dispatcher owns queued-command admission; the SQLite selection adapter owns durable commit admission. These are separate boundaries, not duplicate authorities: the former prevents later application effects, while the latter closes the in-flight transaction race.
- Restoration was checked across sign-in and selection meanings: a later vault generation owns the restored credential/selection; an old operation cannot delete it or create cleanup evidence for it. Same repository identity with a new binding generation remains preserved.

## Evidence inspected

- `/tmp/nessa-1158-inflight-selection-red.log`: revert probe failed `an_in_flight_old_unlock_cannot_remove_a_new_live_selection` (0 passed, 1 failed).
- `/tmp/nessa-1158-inflight-two-tests-rerun.log`: both in-flight store and product regressions passed (2 passed).
- `/tmp/nessa-1158-final-github-suite.log`: 77 passed.
- `/tmp/nessa-1158-final-product-vault.log`: 5 passed.
- `/tmp/nessa-1158-final-vault.log`: vault suites passed (2, 4, and 10 tests; zero failures).
- `/tmp/nessa-1158-final-workspaces.log`: 39 passed.
- `/tmp/nessa-1158-final-clippy.log`: strict selected-package Clippy completed successfully.
- `/tmp/nessa-1158-final-races.log` was still running at review close (Cargo process already owned by the coordinator); I did not start a concurrent run and do not count this incomplete log as evidence.

## Files and boundaries inspected

Inspected the actual diff from `b09cadf17`, with detailed review of `crates/nessa-server/src/product/github.rs`, GitHub relay application ports and `LiveEvents`, SQLite store/selection, vault relay adapter, composition wiring, `crates/nessa-vault/src/application/vault.rs`, the new inbox/product race tests, adjacent relay tests, G22/G23 in `docs/design/github-connect.md`, and the affected architecture/state documentation.

Explicit exclusions: no live GitHub/provider calls, no UI/browser verification, no Windows-specific execution, no performance measurement of filesystem commit while the vault mutex is held, and no independent rerun of Cargo checks. Those exclusions do not change the lifecycle/security conclusion above.
