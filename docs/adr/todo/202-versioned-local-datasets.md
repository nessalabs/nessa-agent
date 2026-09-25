# 202. Local datasets are versioned, classed, and migrated forward only

## Purpose

Every store the gateway and the desktop host keep on disk states the version of
its shape and a class. The class decides what happens when the store is older,
newer, or damaged: the gateway refuses to start, one record is refused, or the
store is discarded and rebuilt. This record also says how migrations ship once
Nessa leaves alpha. It generalizes the refusal in
[196](196-conversation-metadata-database.md) and changes nothing that 196
decided about conversation metadata's tables.

- **Date:** 2026-09-25
- **Status:** proposed — draft, awaiting the owner's answers under
  [Open questions](#open-questions); nothing is implemented
- **Issue:** [#202](https://github.com/nessalabs/nessa-agent/issues/202)

## Context

Each store settled on its own reaction to data it cannot read, and the
reactions disagree:

- **Conversation metadata.** `nessa-local-database` refuses any `user_version`
  but its own (`OpenError::Version`). Composition turns that into
  `RunError::Agent` (`composition/local_auth.rs`), and `restart()` classes that
  as `Worthwhile`. The result is that launchd relaunches a gateway that will
  never read that file, and nothing is written to
  `logs/gateway-startup-failure.json`. Gate 7 is broken today: the failure is
  loud in the log and silent to the person.
- **The credential registry** carries `schemaVersion` and refuses as
  `InvalidRegistry`: exit 28, `Pointless`, recorded, with a refusal audit. That
  is the honest path, but only this one store has it.
- **Stores that ignore and rebuild.** Warm-up records, `installed.json` and
  `shortcuts.json` treat what they cannot read as absent and rebuild it.
- **Stores that refuse one record.** Session journals refuse one conversation
  (`ConversationStateUnreadable`), and a bad attachment hold refuses one request.
- **The browser-session journal** fails the whole gateway as `Authentication`,
  which is retryable, so it too is a relaunch loop.
- **Most stores have no marker at all**, so they cannot tell older, newer and
  damaged apart.

"One current contract" forbids a reader for an old shape unless one is
explicitly requested. The owner asked for this record (#202): datasets are
versioned, the gateway does not fail wholesale over a store it can live
without, and the stores it cannot live without are named.

## Inventory

`N` is the gateway namespace (`<base>[/<stage>][/instances/<instance>]`,
`env/paths.rs`), and `H` is the host's config root (`src-tauri/src/local_data.rs`).
Both follow [0005](../done/0005-stage-scoped-local-data.md).

### Datasets this policy governs

| # | Store | Path, format | Marker today | Truth or derived | Class | Why that class |
| --- | --- | --- | --- | --- | --- | --- |
| D1 | Credential registry (`nessa-auth`) | `N/auth/credentials.v1.json`, JSON | `schemaVersion` 2 | Truth | **Critical** | A reset reopens owner bootstrap: it grants access. It holds revocations and the credential transition audit. "Never silently reset a registry." |
| D2 | Browser-session journal | `N/auth/browser-sessions.jsonl`, JSONL | none | Truth | **Critical** (Q1) | Authorizes cookie sessions and carries their transition evidence |
| D3 | Conversation metadata | `N/conversations/metadata.sqlite3`, SQLite | `user_version` 1 | Truth: `conversations`, `deletions`, and `summaries.archived`. Derived: summary title, preview, updated | **Critical** (Q2) | Ownership authorizes every conversation call. A lost tombstone resurrects a deleted conversation. The archived flag is a user decision held nowhere else. |
| D4 | Session journals (`nessa-sdk`) | `N/conversations/sessions/s-<id>.jsonl`, JSONL per conversation | none | Truth | **Record-scoped** | The only copy of a conversation's history, and 182 blocks deletion on a damaged one. One conversation's failure is that conversation's. |
| D5 | Attachment blobs and holds | `N/attachments/{blobs,holds}/…`, binary and JSON | none | Truth | **Record-scoped** | `uploadedBy` and hold state have no other copy. The root itself stays a startup requirement (Q6). |
| D6 | Audit sinks: execution, creation, file-link, deletion, attachment, warm-up, registry refusal, retirement, agent install, delivery and reclamation journals, MCP process audit (`N/process-audit`), host credential-save audit and reconciliation journal (`H/…`) | one JSON file per record | none | Truth (evidence) | **Critical, never migrated** | Evidence is kept as written. "Reject impossible histories without repairing or erasing the original data." |
| D7 | Managed-runtime coordination: `reclamation.json`, use markers, locks | `N/agents/<agent>/…`, JSON | none | Truth | **Record-scoped** (the install operation) | A misread could reclaim a runtime that is in use. It fails the install or launch, never the gateway. |
| D8 | Summary title and preview, if split from D3 (Q2) | — | — | Derived from D4 | **Degradable** | Rebuildable by rereading journals |
| D9 | Agent warm-up records | `N/conversations/warm-up/<hash>.json` | none | Cache | **Degradable** | Rebuilt by warming again. Already ignored when unreadable. |
| D10 | Managed runtime `installed.json` and binaries | `N/agents/<agent>/…` | none (the release version is data) | Cache, checked against the pinned digest | **Degradable** | Already treated as "not installed" and reinstalled |
| D11 | Host `settings.json` | `H/settings.json`, JSON | none | Truth (user choices and service selection) | **Critical to the host** | Writing defaults over it would lose choices silently. Its two readers already disagree: `load` falls back to defaults, `load_service` refuses. |
| D12 | Host `shortcuts.json` | `H/shortcuts.json` | `version` 1 | Cache of the server's bindings ([0004](../done/0004-server-owned-keybindings.md)) | **Degradable** | Already falls back and leaves the file alone |
| D13 | Browser `localStorage` (tabs, surface) | per origin | none | Cache | **Degradable** | Already ignored when unreadable. Per viewer, and never read by the gateway. |

### Outside this policy, and why

- **Inter-process files.** The endpoint record, `gateway-startup-failure.json`,
  and `gateway-upgrade/{request,result}.json` are contracts between two running
  processes that can be different builds during an upgrade. They are versioned
  with the host–gateway protocol, not as datasets. `result.json` refusing
  startup as `Agent` is the same relaunch-loop defect, fixed by the typed
  refusal below.
- **Secrets.** `owner.token`, the surface tokens, and the keychain entries are
  opaque values. A change to a token's format is an authentication protocol
  change.
- **Operator configuration.** `config.json` is written by a person, never by
  Nessa, so there is nothing to migrate. An invalid file refuses startup, and it
  moves to the typed, recorded refusal as well.
- **Derived artifacts and diagnostics.** Host-staged runtimes, the plist, and
  `gateway.log` are rebuilt from the bundle and checked by fingerprint, or they
  are only a log.

## Decision

**Every dataset declares `{ name, version, class }` beside its shape.** For
SQLite the version is `user_version`. For a single JSON file it is a top-level
`schemaVersion`, as D1 already has. For a JSONL journal it is a header record on
the first line. For a directory of records it is a `schemaVersion` on each
record, so each record can be judged alone. The version is one integer that
starts at 1. It is bumped only when the on-disk shape changes, never because
code changed ("One current contract", second paragraph). A store that has no
marker today is at version 1 as it is written today. The migration that
introduces a marker is the only code that knows "unmarked" (Q4).

**The class is the only thing that decides the outcome.** Composition maps an
open outcome to behaviour through one function keyed on class (gate 13). Stores
do not decide per store.

- **Critical.** The gateway (for the host's stores, the host) refuses to start.
  It refuses as `RunError::Dataset { dataset, refusal }`, with a new exit reason
  `datasetRefused` in `protocol/defaults/gateway-exit-codes.json`. A refusal
  from anything but a lock is `Pointless` and is recorded in
  `gateway-startup-failure.json`, so the host can say which dataset refused and
  why. The file is left exactly as found.
- **Record-scoped.** That record's operations fail with a typed wire error:
  written by a newer Nessa, too old, or unreadable. Every other record, and the
  gateway, carry on. The record is left exactly as found.
- **Degradable.** Any version other than the current one, or an unreadable file,
  is discarded and rebuilt. Degradable datasets never ship migrations, because
  rebuilding is their migration. The discard is recorded in the dataset audit
  before anything is deleted, with the dataset, the version found, the cause,
  and the build that discarded it. While the rebuild runs, the capability
  answers "unavailable" explicitly, never with an empty result (gate 7).
  Discarded rather than set aside: a copy set aside would hold conversation
  titles that a permanent delete must reach
  ([182](182-conversation-deletion.md), 196 § Erasure), and nothing would read
  it (Q5).

**Migrations ship with the build, forward only, one version at a time.** They
live in the owning context's `infrastructure/migrations/`, one step per target
version. For SQLite each step is `NNNN.sql` ending in its
`PRAGMA user_version = NNNN;`, and a fresh file is created by applying every
step from empty. The steps are therefore the one definition of the schema, not
a second one beside `schema.sql` (gate 13). For JSON stores a step is a function
over the stored document. A migration is the only code in the build that reads
an old shape. After it runs, nothing else does.

A migration runs:

- at open, under the store's existing exclusion (SQLite's `IMMEDIATE`
  transaction, the registry and journal locks);
- all or nothing: one transaction, or write-new-then-atomic-replace;
- between an intent record and an outcome record in `N/audit/datasets/`. If that
  sink is unusable, nothing is migrated and the class decides the outcome.

There is no down-migration and no backup copy. A backup would keep erased
conversations readable, and a newer file is refused rather than guessed at.

**The support floor.** Each build supports opening every version from the floor
to the current one. **During alpha the floor is the current version**, so no
build ships migrations. This is 196's decision, now stated for every dataset.
When Nessa leaves alpha the owner sets the floor (Q3), and raising it is
recorded as a decision because it deletes migrations.

## Opening a dataset (gate 15)

"Refuse" means for critical datasets the gateway refuses to start
(`datasetRefused`, `Pointless`, recorded); for record-scoped datasets the record
is refused with a typed error. Tests are written from each row when the policy
is implemented. Their names are given in the implementing PR, not invented here.

| Row | Found | Critical | Record-scoped | Degradable |
| --- | --- | --- | --- | --- |
| O1 | No file | Created at the current version, except where absence has its own meaning: D1's is "not initialized", unchanged | The record does not exist (`NotFound`) | Created empty |
| O2 | Empty file (SQLite: no tables and version 0) | Created at current | Created at current | Created at current |
| O3 | The current version | Opened | Opened | Opened |
| O4 | At or above the floor, below current | Migrated, then O3. On failure, M2. | Migrated under the record's lease, then O3 | Discarded and rebuilt |
| O5 | Below the floor | Refused: `TooOld { found, floor }` | Refused: `TooOld` | Discarded and rebuilt |
| O6 | Above current (a newer build wrote it) | Refused: `Newer { found, current }`. Untouched. | Refused: `Newer`. Untouched. | Discarded and rebuilt |
| O7 | Marker unreadable, negative, or 0 with tables. Content unparseable. | Refused: `Unreadable` | Refused: `Unreadable` (today's `Corrupt`) | Discarded and rebuilt |
| O8 | Directory or file not private, or not this user's | Refused: `Unsafe` | Refused: `Unsafe` | Capability off, file **not** deleted: an unsafe directory is not ours to delete in |
| O9 | Held by another opener | Waits, or `Busy` where the store already says so. The registry's `Locked` stays `Worthwhile`. | `Busy` (the journal lease) | Waits, or the capability answers "unavailable" |

### Migrating and discarding

| Row | Ordering | Result |
| --- | --- | --- |
| M1 | Intent recorded, migration committed, outcome recorded | Opens at current |
| M2 | A step fails (SQL error, a record the step refuses, disk full) | Rolled back: the store is still at the version found. Outcome recorded as failed. Then O5's column for its class. |
| M3 | The process dies mid-migration | SQLite's rollback journal, or the unpublished temporary file, leaves the old version intact. The next open is O4 again. The earlier intent has no outcome, which reads "not confirmed". |
| M4 | The process dies after commit, before the outcome record | The next open is O3. The intent stays without an outcome and is not repaired (gate 16): the store's version answers what happened. |
| M5 | The dataset audit is unusable before a migration or discard | Nothing is migrated or deleted. Critical and record-scoped: refused. Degradable: capability off. |
| M6 | Two openers at once (a gateway and `nessa install-agent`) | The store's exclusion orders them. The second finds O3. |
| M7 | A build older than the data is started after a migration (downgrade) | O6 |
| M8 | Discarded, then the process dies before the rebuild | The next open is O1 |

## Alternatives considered

- **Keep refusing everything that is not current (196 as is).** It is honest
  per store, but a stale warm-up record or projection would cost the whole
  gateway, which is what the owner asked to stop. It also leaves the relaunch
  loop in place.
- **In-place readers of old shapes** (serde defaults, `Option` for new fields).
  Every old shape stays in the domain forever, which is the dual path "One
  current contract" forbids. Migrations confine it to one place that runs once.
- **Down-migrations.** They double the code, and a projection would silently
  lose columns. Refusing a newer file is simpler and says what happened.
- **Moving degradable stores into a quarantine directory.** Nothing reads it,
  and erasure would have to reach it. The audit record plus the log carry the
  diagnosis (Q5).
- **One gateway-wide dataset version.** Every context's change would bump every
  store, and one stale cache would refuse them all. A version per dataset
  follows context ownership.
- **Degradation per context** (for example, attachments unavailable while chat
  works). This adds a fourth outcome with its own health and wire states.
  Deferred until the owner asks (gate 16, Q6).

## Consequences

- The relaunch loop on an unreadable or other-version store ends. The host can
  say which dataset refused and why.
- A cache or projection never costs the gateway. Its rebuild is visible as
  "unavailable", not as empty.
- Each shape change carries a migration, a fixture of the old version checked
  in by the release that wrote it, and a test that migrates the fixture and
  reads it. That work is accepted, and it starts only once the floor is below
  current.
- Adding a marker to the stores that have none is itself a shape change
  (D2, D4–D7, D9, D11). It lands as each store next changes, or all at once
  (Q4).
- What to watch: a critical dataset refusing in the field more than rarely. That
  would mean something classed critical should have been record-scoped or split
  (Q2).

## Open questions

These are for the owner. Nothing is built until they are answered.

1. **Browser sessions (D2).** Should D2 stay critical, or become degradable and
   fail closed? Discarding D2 only removes access: everyone signs in again with
   a token, and the discard is audited. The transition evidence it holds would
   then be lost unless it moves to an audit sink first.
2. **Split D3.** Should the archived flag move into an authoritative table, and
   title, preview and updated move into a projection (D8) rebuilt from the
   journals? That would let a list degrade instead of the gateway refusing.
   Both are 196's tables, so this would be a schema change in its own record.
3. **The floor after alpha.** Should it be every version since 1.0, or the last
   N releases?
4. **Markers for today's unmarked stores.** Should "unmarked = version 1" hold
   until each store's next change? Or should markers be added now, in alpha,
   with the namespace deleted by hand once more, as for 196?
5. **Discard or set aside.** Discard is proposed for degradable stores. Should
   a set-aside copy be kept for diagnosis, limited to stores holding no user
   content?
6. **Attachments root and D11.** Should an unopenable attachments root refuse
   the gateway (today) or degrade the context? Should the two readers of
   `settings.json` be made to agree now?
7. **Audit record shape changes (D6).** Records are never migrated. When their
   shape changes, should the idempotency read-back for creation and deletion
   IDs compare a record of an older shape, or treat it as already present?
