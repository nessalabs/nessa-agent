# Conversation catalogue for linked readers (#259)

The conversation metadata database owns the current catalogue. It has one durable
incarnation, and each owner has one durable head. Each conversation keeps its creation revision
and latest visible change revision. A deletion keeps the original creation key
and advances the change revision once, when the tombstone first fences it.
Changing a tombstone's erasure progress does not change the catalogue value.

The existing alpha schema is replaced as a whole. `nessa-local-database` refuses
an older file with `OpenError::Version` and leaves it untouched. An operator must
explicitly move or remove old local alpha metadata after deciding what to keep;
opening it never imports or resets ownership or deletion evidence. A new file
gets a new incarnation. This is not a silent cache reset.

The reader accepts an authenticated organization and principal from the caller's
current session. SQL narrows to that exact owner, and every returned conversation
is checked through the domain ownership rule. Scope, incarnation, and access
epoch are compared on every read. A mismatch is typed, never answered as an
empty catalogue. A receiver reset must be explicit; an absent identity is
unknown, and only a retained deletion marker says deleted.

| Ordering or event | Source result | Receiver consequence |
| --- | --- | --- |
| Create a new identity | Allocate owner head `H+1`; set both creation and change to it in the creation transaction | A pass with boundary at least `H+1` can page it. |
| Reopen an identity, including a deleted one | No new revision or replacement | Existing ownership and deletion decision stand. |
| Summary, archive, or committed mode changes | Allocate a new owner revision in the same transaction as the visible value | A later pass reads the latest value. |
| Delete races with summary | The deletion fence and revision commit together; later summary write is refused | The retained marker wins, including after restart. |
| Erasure continues after the fence | Keep the deletion revision | Retry does not make another catalogue event. |
| Pass starts at completed `C` and head `H` | Capture `H` once | Pass is finite even if writes continue. |
| Manifest page after key `K` | Select creation `<= H`, change `> C`, key `> K`, ordered by `(creation, id)`, with `LIMIT max+1` | A newer change revision may exceed `H`; it is still delivered. |
| Value changes between manifest and resolve | Resolve current value at the same key and at least the descriptor revision | Receiver commits current value or retries without advancing cursor. |
| Earlier key changes after its page | Its change revision exceeds `H` | The next pass selects it. |
| Source reply or receiver commit is lost | A retry reads current source data under the saved pass | Receiver progress, not a reply, decides continuation. |
| Owner, incarnation, or access epoch changes | Refuse with a typed mismatch | Receiver explicitly resets under current authorization; old live values are cleared and absence stays unknown. |
| An owner's head is missing or disagrees with the largest retained change revision | Refuse head, page, resolve and new creation as unavailable; retain all existing rows unchanged | A damaged counter cannot publish an empty catalogue or reuse an existing revision. Another owner remains readable. |
| Own row is unreadable | Refuse the read as unavailable | Progress does not advance. Another owner's damaged row is outside the query. |

The manifest reads one short transaction per page and releases it before any
payload read. It does not use offsets or hold a snapshot across pages. Page
size and payload size are bounded by the sync contract. The source stores no
per-receiver state or history.

The metadata tests in `crates/nessa-server/tests/conversation/store.rs` cover
creation, summary, archive, deletion and mode revisions; failure rollback;
version refusal; owner isolation; damaged rows; a manifest/deletion race; and a
620-entry pass with changes from a second open store between pages. The sync
adapter tests add receiver progress, lost replies, reset epochs and payload
bounds when the shared dependency is integrated.
