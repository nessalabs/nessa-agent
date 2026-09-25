# 202. A store refuses what it cannot read at the smallest scope that owns it

## Purpose

Every store Nessa keeps on disk states the version of its shape. When it meets
data it cannot read, the refusal covers the smallest thing that owns that data:
the whole gateway for gateway-wide records, one conversation for one
conversation's records, nothing at all for a cache. This record also says how
migrations ship once Nessa leaves alpha. It generalizes the refusal
[196](196-conversation-metadata-database.md) introduced, and changes none of
196's tables.

- **Date:** 2026-09-25
- **Status:** accepted
- **Issue:** [#202](https://github.com/nessalabs/nessa-agent/issues/202)

## Context

Each store settled its own reaction to data it cannot read. Two of those
reactions are wrong:

- **Conversation metadata.** `nessa-local-database` refuses a `user_version`
  that is not its own. Composition turns that into `RunError::Agent`, which
  `restart()` classes as `Worthwhile`. launchd therefore relaunches, forever, a
  gateway that will never read that file, and no startup-failure record tells
  the host why.
- **The browser-session journal** fails the same way, as `Authentication`,
  because its open has one error for "cannot be replayed" and "cannot be
  opened".

The rest are already right. The credential registry refuses as
`credentialRegistryInvalid`: not retried, recorded. Session journals refuse one
conversation. Warm-up records, `installed.json` and `shortcuts.json` ignore what
they cannot read and write it again. The owner asked for the right ones to be
the rule, and for the stores the gateway cannot live without to be named.

## Decision

**1. One version per stored shape.** It is SQLite's `user_version`, or a
`schemaVersion` field in JSON, and it starts at 1. It is bumped only when the
on-disk shape changes ("One current contract"). A store that has no marker
today is version 1 as written today. The change that first alters its shape
adds the marker. Nothing is added now, and nothing is deleted by hand.

**2. Three scopes, decided by what the data is.**

| Scope | Stores | What it cannot read |
| --- | --- | --- |
| **Gateway** — truth that decides who may do what for everyone | credential registry; browser-session journal; conversation metadata (ownership, tombstones, archived flag, summaries) | The gateway refuses to start |
| **Record** — truth about one conversation or one upload | session journals; attachment holds (a blob is opaque bytes named by their digest, judged by that digest and never by a marker); managed-runtime reclamation and use markers | That record's operations fail, typed. Everything else carries on. |
| **Cache** — rebuilt from truth or from outside | warm-up records; `installed.json` and runtime binaries; host `shortcuts.json`; browser `localStorage` | Ignored, and overwritten when next written |

Audit sinks are write-only evidence. They are never migrated and never read for
meaning. Their existing rule holds: an audit that cannot be written fails the
audited operation. A deterministic record found at another shape is a mismatch
and is refused, as it is today.

Files one process writes for another (the endpoint record, the startup-failure
record, `gateway-upgrade/*`), secrets (tokens, keychain entries), and
configuration a person writes (`config.json`, the host's `settings.json`) are
not datasets. Their formats change with the protocol or the configuration they
belong to.

**3. A gateway-scope refusal says what it is.** It is
`RunError::Dataset(DatasetRefusal)`, with the exit reason `datasetRefused` in
`protocol/defaults/gateway-exit-codes.json`. The reason is not retried, and it
is recorded in `gateway-startup-failure.json` so the desktop host can say it in
its own words. Only content the build cannot read gets this reason: another
version, or a file that is not a readable database. A failure that can clear on
its own (I/O, a lock) keeps its current, retried reason. The registry keeps its
own reasons, which already work this way.

**4. Migrations.** During alpha, none ship. A gateway-scope store at another
version is refused under rule 3, and the owner resets that namespace by hand,
as for 196. After alpha, each build carries forward-only migrations in the
owning context, from every version any 1.x release wrote. They run at open,
under the store's own lock, all or nothing. A newer version is refused, and
there is no downgrade. The first migration to ship brings its own record, with
the ordering table gate 15 asks for (crash mid-step, two openers, failure). A
migration that fails keeps its own cause and step. It is not reported as "too
old". No
migration code exists before then.

## Opening a gateway-scope store (gate 15)

| Row | Found | Result | Test |
| --- | --- | --- | --- |
| G1 | No file, or an empty one | Created at the current version (see below) | `nessa-local-database`: `an_empty_file_is_given_the_schema_at_its_version_and_reopened_as_it_is` |
| G2 | The current version | Opened | same, and every store test that reopens |
| G3 | Any other version, including tables with none | `datasetRefused`, not retried, recorded. File untouched. | `a_metadata_database_at_another_version_refuses_the_gateway_for_good` |
| G4 | Not a database, or corrupt | `datasetRefused`, not retried, recorded. File untouched. | `a_file_that_is_not_a_database_refuses_the_gateway_for_good` |
| G5 | Directory missing or not private, file not private, I/O failure, busy | Today's reason (`agent`), retried | `a_metadata_directory_that_cannot_be_used_is_still_retried` |

An existing empty metadata file is given the schema, not refused. A first open
that dies before its schema commits leaves exactly that file, and refusing it
would stop that gateway for good. A file emptied by something else has nothing
left to protect: without ownership rows nothing is reachable, and the creation
audit refuses an identity created again (196, "No move"). The registry is
different. An empty `credentials.v1.json` does not parse and is refused; only a
missing one means "not initialized". An empty browser-session journal holds no
sessions, which grants nothing.

The browser-session journal has the same rows. Its open returns
`JournalOpenError`: `Unreadable { line, problem }` for anything replay refuses
(a file or line over its bound, a last line without its newline, a line that
is not a record, a record that is not a legal next step), and `Unavailable` for
the rest. G3/G4 are
`a_browser_session_journal_replay_refuses_stops_the_gateway_for_good` and
`what_replay_refuses_is_unreadable_and_names_its_line`. G5, whose reason stays
`authentication`, is `a_browser_session_journal_held_elsewhere_is_still_retried`.

## Decisions taken for the owner

The owner asked on 2026-09-25 for the simplest design, and delegated these
choices:

- **Browser sessions** are gateway scope. Discarding them would be fail-closed,
  but it would lose their transition evidence.
- **Conversation metadata is not split.** Moving the archived flag out of
  `summaries` would let listings degrade, but that precision was not asked for
  (gate 16).
- **The support floor** after alpha is every version since 1.0.
- **Nothing is quarantined.** A cache is overwritten, and a truth store is left
  exactly as found.
- **The attachments root** unchanged: a root that cannot be opened stays a
  retried startup failure.

## Alternatives considered

- **Keep 196's refusal as it is.** It is honest per store, but it leaves the
  relaunch loop.
- **Readers for old shapes** (serde defaults, optional fields). Every old shape
  would live in the domain forever, which "One current contract" forbids.
- **Down-migrations and backups.** Twice the code, and a backup would keep
  erased conversations readable.
- **Quarantining degradable stores.** Nothing reads the quarantine, and erasure
  would have to reach it too.
- **A per-context "unavailable" state** (attachments down, chat up). That is a
  fourth outcome with its own health and wire states, not asked for.

## Consequences

- A store this build cannot read stops the gateway once, says which store and
  why, and is not retried.
- A cache never costs the gateway.
- After alpha, every shape change carries a migration and a checked-in fixture
  of the version it migrates from.
- A browser-session journal whose last append was cut short by a crash
  now stops the gateway with a sentence, where it used to relaunch forever.
  Either way it needs a hand repair. That append was never acknowledged, so
  truncating it at open, as the SDK's session journals already do, would be
  safe. It is not done here.
- Remaining: migrations, once Nessa leaves alpha.
