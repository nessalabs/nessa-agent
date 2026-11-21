# nessa-ports

A headless library for complete, advisory workspace listener snapshots. It does
not discover listeners, authorize access, or open connections. Observations can
be out of date by the time a host uses them; neither a port nor an identity is
an authorization decision.

The host supplies a workspace identity and a generation that changes on every
replacement. It must call `replace_generation` when that workspace is replaced and
route snapshots to the owner for that workspace. Listener identities must be stable within an
incarnation and must distinguish listeners even if they share a port. Revisions
are ordered by the host within an incarnation.

| Input | Result |
| --- | --- |
| First complete snapshot | Accept and report changed, even when empty |
| Newer revision, same observations | Advance revision without an event |
| Equal revision, same observations | Accept replay without an event |
| Newer revision, changed observations | Replace and report changed |
| Older revision or different workspace/generation | Reject, preserving state |
| Equal revision with different observations | Reject, preserving state |
| Duplicate listener identity | Reject the entire snapshot, preserving state |

## Module map

- `src/lib.rs`: validated values and the single inventory owner; only `std`.
- `tests/inventory.rs`: host-facing contract tests with synthetic data.

Forward lifetimes are not implemented. A later slice needs an explicit,
authorized request and a bounded lifecycle with a provider-owned effect adapter,
including cleanup after disappearance, close, loss and late completion.
