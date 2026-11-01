# 1185. Direct relay gap and acceptance trim the audit store they write

## Purpose

A cloud retention gap and the owner's acceptance of that gap are audited in the
same transaction that records them. Each local audit store stays within the
contract cap when that transaction commits, including when the store was
already full. This closes the exception recorded on
[#1185](https://github.com/nessalabs/nessa-agent/issues/1185). Settings
selection and page commit already trim both stores; this decision makes the
direct paths do the same for the store they write.

- **Date:** 2026-11-01
- **Status:** accepted

## Context

The encrypted inbox has two audit stores. `audit` records payload and cursor
facts (`cloud_retention_gap`, `gap_accepted`, retention, connect, disconnect).
`binding_audit` records generation facts (connect, disconnect, page commit,
Settings selection). The cap is `auditRows` in
[the relay contract](../../protocol/defaults/github-relay-contract.json). The
contract states the number. It does not excuse a path from it.

`SqliteRelayStore::gap` inserts `cloud_retention_gap` into `audit`, then
deletes overflow from `binding_audit`. Direct `bind(..., true)` inserts
`gap_accepted` into `audit` and deletes overflow only from `binding_audit`.
Starting at the cap, each of those commits leaves `audit` one row over. The
next Settings selection or page commit trims it, because those transitions
already delete overflow from both stores. Until then the cap does not hold.
Repeated gap-and-accept cycles keep the extra rows. The cloud payload quota,
the cursor, and the gap marker are unchanged.

Nessa is alpha. Rows already over the cap are not migrated. The next
transition that inserts into that store trims it. There is no sweep for files
this build does not touch.

## Decision

The transaction that inserts an audit row deletes that store's oldest rows,
by primary key, down to `auditRows`, and then commits. `gap` trims `audit`.
`bind` trims `audit` and `binding_audit`, because a new binding writes both
and acceptance writes `audit`. The delete statement for each store lives once
and is called from the transition. Settings selection, page commit, and
unbind already trim both stores in their own transactions; they call that
same statement. The newest row has the greatest id, so the trim keeps the
transition's target, cause, actor, and before-and-after cursors.

An insert or that delete that fails aborts the transaction. The gap, the
cursor, the events, and both audit stores stay as they were.

## States

`auditRows` is the cap from the contract. "Trim" means the primary-key
offset delete in the same transaction, before commit. A failed audit write
rolls the whole row back.

| Transition | `audit` | `binding_audit` | On audit-write failure |
| --- | --- | --- | --- |
| Direct gap, no gap open | Insert `cloud_retention_gap`: user, repository, before cursor, after cursor = through, actor `system:cloud-relay`. Trim `audit`. | Not written. Left as it was. | `gap_through` unset. The new row is absent. |
| Direct gap, gap already open | No insert (`gap_through` already set). `gap_through` becomes the max. | Not written. | No audit write, so nothing new to roll back. The generation check still refuses a stale binding before any write. |
| Direct / CLI acceptance (`bind` with acceptance, gap open) | Insert `gap_accepted`: same target, before cursor, after cursor = the gap, actor `local-owner:github:{user}`. A new binding also inserts `connected`. Trim `audit`. | Insert `connected` when the generation changes. Trim. | Cursor and `gap_through` stay. No acceptance row. |
| Settings selection | Disable inserts `live_events_disabled` per removed event (delivery, caller). Trim. Gap acceptance writes no payload row here. | Disable, enable, and gap acceptance insert their generation row, including before and after cursors on acceptance. Trim. | Selection, events, and cursors stay. |
| Page commit | Insert `retention_expired` per expired event. Trim. | Insert `page_committed` when the cursor moves: both generations, before and after cursor, actor `system:local-relay`. Trim. | Events and cursor stay at the page's previous cursor. |

## Invariants

| Guarantee | Enforcer |
| --- | --- |
| After a committed gap, `audit` has at most `auditRows` rows and `binding_audit` is unchanged. After a committed bind, Settings selection, or page commit, both stores have at most `auditRows` rows. | The offset delete in that transaction. `direct_gap_and_acceptance_keep_each_audit_store_within_its_cap` starts both stores at the cap, repeats gap and acceptance, and checks both neighbours. |
| The committed row for that transition keeps its target, cause, actor, and before-and-after cursors. | The same test reads the newest row after the trim. The new row is the greatest id. |
| A failed insert or a failed trim commits nothing: gap, cursor, events, and both stores match the pre-transition rows. | The same test aborts the insert and, separately, the delete. Selection refusal stays `cleanup_audit_failure_refuses_disable`. Page refusal stays `failed_page_rolls_back_events_and_cursor_and_reopen_replays`. |
| No later job is required for the cap to hold. | There is no sweep and no `COUNT(*)` in the transition. The delete is the statement above. |

`auditRows >= storedEvents` in `crates/nessa-server/build.rs` keeps one bulk
expiry inside the cap, so the expired identities are the rows the trim keeps.

## Alternatives considered

- **A periodic sweep.** The cap would be true only after the sweep. A process
  that only records gaps would stay over the cap, and a crash between the
  insert and the sweep would leave the overshoot committed. The sweep would
  be a second owner of the same decision.
- **A full-table recount.** `COUNT(*)`, then delete the surplus, reads every
  row to learn what the primary-key offset delete already expresses. The
  cloud relay already refused a full scan at capacity
  ([1132](../todo/1132-github-relay-hardening.md)).
- **Trim outside the transaction.** The insert can commit and the trim can
  fail. The caller then sees a failure while the gap and the extra row
  remain, or a success with the store over the cap. Rolling back the audit
  write would not include a trim that already committed.

## Consequences

Gap and acceptance stay within the cap without waiting for another
transition. The cost is one indexed delete of the overflow in the
transaction that already writes the row. At the cap that delete removes as
many rows as the transaction inserted. `gap` does not trim `binding_audit`,
which that transition does not write. `bind` trims both stores on every
call, including a call whose conditional insert wrote nothing: a store
already within the cap loses nothing. An older file already over the cap
comes back only when one of these transitions runs its delete. There is no
separate migration.

The state page's retention section currently says the direct paths may
exceed the cap. That sentence is updated with this decision. The contract
JSON is unchanged: `auditRows` was already the cap, and it never stated an
exception.

## Review brief

Review the branch `cursor/bound-relay-audit-retention-dcab` against `main`.
The tree under review is the committed head; do not restack it during the
check. Intended behavior is the state table above.

Scope: `SqliteRelayStore::gap` and `bind` in
`crates/nessa-server/src/github_relay/infrastructure/sqlite.rs`, the single
trim statement those transitions share with selection, page commit, and
unbind, the regression in `crates/nessa-server/tests/github_relay/inbox.rs`,
this record, and the retention paragraph in
`docs/state/services/github-webhooks/README.md`. Out of scope: the HTTP gap
decoder, Cloudflare quotas, desktop Settings rendering, and migration of
existing files.

Try to break the guarantees with the real SQLCipher store. Start both stores
at `auditRows`. Run gap, repeat gap, and `bind(..., true)` for several
cycles. Abort the insert. Abort the delete. Confirm the cursor, the gap, and
both counts. Then run a page commit and a Settings selection from that same
full store and confirm the newest target, cause, actor, and cursors. A trim
that runs after commit, a trim of only `binding_audit`, or a `COUNT(*)`
before the delete, is a miss even if a later test is green.
