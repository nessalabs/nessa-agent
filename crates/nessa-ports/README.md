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
| Newer revision, same observations | Advance revision without an event, unless `Ports` recovers eligibility |
| Equal revision, same observations | Accept replay without an event |
| Newer revision, changed observations | Replace and report changed |
| Older revision or different workspace/generation | Reject, preserving state |
| Equal revision with different observations | Reject, preserving state |
| Duplicate listener identity | Reject the entire snapshot, preserving state |

## Module map

- `src/lib.rs`: validated values and the single inventory owner; only `std`.
- `tests/inventory.rs`: host-facing contract tests with synthetic data.

`src/forwards.rs` owns pure forward lease accounting; provider effects and
resource handles remain with the caller. `tests/forwards.rs` checks the state
contract using synthetic endpoints.

## Forward lease design

The host authenticates and authorizes an explicit request before calling this
library. It routes observations and all completion/cleanup reports to one
`Ports` owner for the lifetime of a workspace identity. A listener identity is
opaque, compared as its exact supplied UTF-8 bytes, and is not a label. This
crate does not normalize or limit identity length or inventory size. The host
must bound the bytes and number of observations before passing untrusted input;
those allocations and the caller's resource handles are outside this crate's
capacity bound.

The capacity includes pending opens and resources awaiting confirmed release.
The caller owns the provider handle and must deliver each opening outcome to
this owner, including after cancellation; on a release instruction it must
release the handle and report confirmation. Dropping this owner or failing to
report completion/cleanup leaves that obligation with the caller. No result from
this state machine authorizes an operation or verifies a real socket.

| State | Input | Result / next state |
| --- | --- | --- |
| absent | explicit request with current listener, capacity available | opening; reserve slot |
| opening | valid loopback completion while listener current | active |
| opening | failure | absent; release slot |
| opening | close, listener gone/changed, or workspace replaced | closing opening; keep slot |
| closing opening | successful completion (including invalid endpoint) | release instruction; closing resource |
| closing opening | failure | absent; release slot |
| active | close, listener gone/changed, or workspace replaced | release instruction; closing resource |
| active or closing resource | provider confirms resource lost | absent; release slot |
| closing resource | caller confirms release | absent; release slot |
| closing resource | repeated close or late completion | no second opening or new release instruction |
| any | stale generation, unknown lease, wrong state | typed refusal; state unchanged |

Snapshot discovery alone creates no reservations. A listener reappearing does
not revive an invalidated lease; an explicit new request is required. A host
must use monotonically increasing generations and must not create competing
owners for the same workspace incarnation. A lease ID includes the workspace
and generation and is scoped to that owner. Callers must route outcomes to the
owner that issued it. A rejected completion does not transfer ownership of
its resource; the caller must release that resource independently.

`close_all` seals the workspace against further snapshots and requests. A later,
strictly newer `replace_generation` starts a fresh incarnation; pending cleanup
from the previous one remains counted. Provider loss is a confirmed observation,
not an instruction to retry, and frees an acquired resource's slot.

When a current-generation snapshot has duplicate identities or contradicts an
accepted revision, it is ambiguous. The owner marks inventory ineligible,
returns cleanup instructions for every lease, and requires a strictly newer,
unambiguous revision before accepting another request. Older or wrong-workspace
updates do not invalidate a newer accepted view. A rejected ambiguous snapshot
returns both its typed error and cleanup instructions; the host must process
those instructions even though the snapshot was rejected.

After ambiguity, `Ports::apply` reports `Changed` when a newer valid snapshot
restores eligibility, even when its observations match the last accepted
snapshot. Later identical polls report `Unchanged`. Every accepted snapshot can
advance the revision, including an `Unchanged` result: callers must read the
current `ports.inventory().revision()` before an explicit lease request. The
revision is freshness evidence, not authorization, and the request still checks
it against the owner’s current state.
