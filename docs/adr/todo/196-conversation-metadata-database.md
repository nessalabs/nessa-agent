# 196. Conversation metadata lives in an embedded database

## Purpose

Keep who owns each conversation, its tombstone, and what a list shows about it
in one private SQLite database per gateway namespace. Reading them back then
costs what a question actually needs, not the whole gateway's history. The
shared opener is a crate of its own, so the next context that needs to ask its
data more than "give me this ID" can use it too. This record changes
[182](182-conversation-deletion.md) where that one describes the files.

- **Date:** 2026-09-25
- **Status:** proposed
- **Issue:** [#196](https://github.com/nessalabs/nessa-agent/issues/196)

## Context

Until now, conversation metadata was three directories of JSON files, one file
per conversation, named by its ID: ownership records, tombstones beside them,
and summaries. Looking a conversation up by ID was one file read. Any other
question meant reading every file.

`conversation.list` asks one of those questions: which undeleted conversations
does this caller own that have a summary, newest first, archived or not? To
answer it, the gateway read every record of every principal, kept the caller's,
read a summary for each, sorted them, and only then cut the result to 500. The
bound limited the response, not the work, so any authenticated caller could make
the gateway read its entire history as often as they liked.

A per-owner index of files would fix that one question. Sorting by summary time
would still read every one of the caller's summaries. Every later question — an
archived-only view, a page token, search — would need another hand-made index,
kept in step with its records by crash-ordering rules written again each time.

The binding constraint is how the data is stored, so that is what changes here.

## Decision

**A reusable opener.** `crates/nessa-local-database` opens one SQLite file for
one context, and nothing else. It checks that the directory is private (through
`nessa-local-storage`), creates the file privately when absent, and turns on
foreign keys, a rollback journal with full sync, and `secure_delete`. It then
compares the file's `user_version` with the context's schema version: an empty
file is given the schema in one transaction, a matching one is opened, and any
other version is refused with a typed error. There are no in-place schema
migrations. Under "One current contract", a schema change bumps the version and
ships its own move, decided in its own record. Each
context owns its own database file and schema. Nothing shares tables across
contexts.

**The conversation store.** `LocalConversationStore` keeps
`conversations/metadata.sqlite3` with three tables, defined once in
`crates/nessa-server/src/conversation/infrastructure/schema.sql`. The server
includes that file:

| Table | Key | Holds | Written |
| --- | --- | --- | --- |
| `conversations` | `id` | organization, owner, creator surface and action, creation time, agent | once, at creation |
| `deletions` | `conversation_id` → `conversations` | the tombstone: who decided, the provider session read, what became of the agent's record, erased | at the fence, then carried further |
| `summaries` | `conversation_id` → `conversations` | title, preview, updated time, archived | on every change; removed at erasure |

`conversations(organization, owner)` is indexed, and so are unfinished
deletions. Foreign keys mean a tombstone or summary without its record cannot
exist. The orphaned tombstone that 182 counted separately is gone, along with
`DeletionsLeft::orphaned_tombstones`.

The one store implements three application ports:

- `ConversationRepository`: `load`, `create` and `record_deletion`, unchanged.
  `list` is replaced by `unfinished_deletions`, the startup finish's only
  question.
- `ConversationSummaries`: unchanged.
- `ConversationListing`, new: `list(organization, owner, archived, limit)`
  returns that owner's undeleted conversations whose summary has that archived
  flag, newest summary first and then by ID, at most `limit` of them. It also
  says how many of the rows it met could not be read.

The service asks for `MAX_LISTED_CONVERSATIONS + 1` rows and reads live state
only for the rows it keeps. Whose a row is has two expressions: the domain's
`Conversation::allows`, and the query's indexed `=`, which is what keeps the
work to the caller's own rows and cannot ask the domain row by row without
reading everybody's. Gate 13 allows that only with the two held to one answer,
so a test runs owner text chosen to separate them — case, Unicode
normalization, lookalike characters, another organization — through both and
requires the same set (`the_list_asks_whose_a_conversation_is_as_the_domain_answers_it`).
Changing either comparison fails it. The work is one indexed range over the caller's own
records plus a lookup of each one's summary by key, sorted with a limit, so at
most 501 rows are held.

**`complete` becomes exact for each caller.** A list is complete when the bound
cut nothing and no row it met could be read. Only the caller's own rows are met,
so a damaged record owned by someone else no longer makes everybody's list
incomplete. Ownership here is the owner's text compared byte for byte, SQLite's
`BINARY` collation, which is the same comparison `Conversation::allows` makes.
A row the caller cannot be shown to own therefore cannot be theirs.

### Listing, as a table

One row per condition a candidate row can be in. "Met" means the query returned
it inside the `limit + 1` window.

| Row | Condition | Listed | Counts against `complete` |
| --- | --- | --- | --- |
| L1 | Caller's, undeleted, readable summary with the asked archived flag | yes, newest first | no |
| L2 | Caller's, summary has the other archived flag | no | no |
| L3 | Caller's, no summary (nothing was said) | no | no |
| L4 | Caller's, has a tombstone (readable or not) | no | no |
| L5 | Caller's, met, but the record or summary row fails domain validation or holds text that is not UTF-8 | no | yes |
| L6 | Somebody else's (a different organization, or an owner that differs by any byte, including case) | no | no |
| L7 | More than `limit` rows qualify | the newest 500 readable rows among the 501 met; fewer when some of those are L5 | yes |
| L8 | Exactly 500 qualify | all 500 | no |
| L9 | The database cannot be queried | the list fails (`conversation_storage_unavailable`); one that cannot be opened stops the gateway starting | — |

L5 changes what 182 said. There, a conversation whose summary could not be read
was listed bare, in the default list. Here a summary row is unreadable only
after a hand edit: one that breaks a domain bound, or leaves text that is not
UTF-8, which `STRICT` does not refuse. One such row costs its list that row,
never the whole list. Its archived flag is still a readable column, so the query files it
under that flag. Showing it bare in the other list would mean fetching both
flags and re-sorting in the service. That precision was not asked for (gate 16),
so the row is left out and the list says it is incomplete.

### No move

Nessa is in alpha, and the owner decided on 2026-09-25 that this change ships
no migration: the JSON files earlier builds wrote are deleted by hand, once,
and nothing in this build reads, refuses or mentions them. Keeping a mover and
the startup refusal beside it would have been a second reader of an old shape,
which "One current contract" forbids without that decision.

### Erasure

A permanent delete erases the summary. With files, that was an unlink.
`secure_delete` overwrites a deleted row's bytes in the database file, and the
rollback journal holding the old page is removed at commit, so the summary is
not left readable in free pages. A WAL journal would keep old page images until
a checkpoint, which is why the store uses the rollback journal.

### Owner agreement

Gate 16 asks for the owner's agreement before building precision. The owner
chose this on 2026-09-25, in the session that opened #196: an embedded
database over a file index, and `complete` exact for each caller rather than
gateway-wide. Pagination was asked about and deferred; see Consequences.

## Alternatives considered

- **An owner index of files beside the JSON records.** It bounds the scan to
  the caller, but sorting still reads every one of the caller's summaries. The
  index adds a crash ordering between two writes, and every later question
  needs another index. Rejected for the store that answers all of these.
- **A generic key → IDs index in `nessa-local-storage`.** Reusable, but it
  rebuilds part of a database by hand, without ordering or transactions.
- **An in-memory index built at start.** No format change, but the gateway
  keeps its whole history in memory, and every start rereads every record.
- **Page tokens alone.** They shrink each response, but the first page of a
  newest-first list still needs every record read. They belong on top of an
  indexed store and are a small follow-up there.
- **The owner copied into `summaries` to read exactly 501 rows.** It would
  bound the read to the page as well as the retained rows, but it copies
  ownership into a projection that is rewritten every turn. The normalized
  query's work is the caller's own conversations, which is the bound asked for.
- **In-place schema migrations in the opener.** A second reader for an old
  shape, which "One current contract" forbids without a decision.

## Consequences

- Listing, the startup finish, and any future question with an index cost what
  they ask, not the gateway's history.
- A new native dependency: `rusqlite` with bundled SQLite, compiled from C, adds
  build time and a C toolchain on every target (already needed by other
  crates).
- A corrupt database file fails every conversation metadata read, where a
  corrupt JSON file cost one conversation. SQLite's own integrity makes that
  rarer than a damaged file was, but it is louder when it happens: the log says
  what SQLite reported (and names the file when it cannot be opened), and
  `conversation_storage_unavailable` says so to the caller.
- Every schema change ships a version bump, and its own move once there is
  data worth keeping. A gateway run
  against a database of another version refuses to open it rather than guess.
- One connection serves every caller, one call at a time, as the files' write
  lock did for writes; reads now wait too. An owner with a very large history
  makes every other caller's metadata call wait for their list. The store's
  own history bounds that, not the gateway's, and it is the first thing to
  watch: more connections are the remedy if it shows.
- New questions become new schema, each with its version bump. A
  fact a conversation has one of — a note for the coordinator agent, a
  category — is a column, indexable alone or with others. A fact it has
  many of — tags — is a table of its own, `(conversation_id, tag)` indexed on
  the tag, since a list packed into one column cannot be indexed and asking
  it would read every row. Searching inside note text is SQLite's full-text
  index. Each names its conversation by a foreign key, so deletion reaches it.
- What to watch: the `limit + 1` query's cost for an owner with a very large
  history, which is the owner's own and linear in it. A covering index on
  `summaries` would be the next step, and page tokens after that.
