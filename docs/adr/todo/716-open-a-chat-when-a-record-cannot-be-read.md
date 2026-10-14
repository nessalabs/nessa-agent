# 716. Open a chat when a saved record cannot be read

## Purpose

A person can open a chat when some of its saved records cannot be read. Each
unreadable record is one line in the transcript, and every record that can be
read is shown. The chat is refused only when its identity cannot be trusted.

- **Date:** 2026-10-13
- **Status:** proposed

## Context

Reading a chat applies one `TranscriptFold` ([252](252-runtime-roles-and-execution-leases.md)).
A save unit is one semantic batch inside one physical fact, and a save ends
with a completion record. ADR 252 admits new record kinds without changing the
meaning of an existing kind. #717 refuses the whole chat when one record is
another version or does not decode (`conversation_state_unreadable`) and leaves
partial reads here.

The product rule is that invalid or old data is skipped. Alpha has no
migration: a record this build cannot read is a gap, not a converted history.
Nessa does not rewrite a Claude or Codex session to repair the gap.

## Decision

The fold skips a record it cannot read, keeps the records it can, and remembers
at most 32 gaps. Each gap is one inline line, "Couldn't read this part of the
conversation", placed after the last turn that had been accepted when the gap
was recorded. Pieces of the skipped record, and a later record whose turn was
never accepted once a gap exists, stay out of the transcript and do not add
another line. The line's element carries the gap's id so a future report
action can attach to it; this change adds no report control. A record skipped
after those 32 is still skipped, and the transcript ends with one more line,
"More parts of this conversation couldn't be read".

A lease record that cannot be decoded or applied does not fail the chat. When
a lease is held, that record marks the current lease unreadable and keeps its
revision, which is the rule an unknown lease kind already follows. The gap
line is still shown.

### What is skipped and what is fatal

| Record | Result | Why |
| --- | --- | --- |
| Save unit whose body is corrupt, another version, or not a JSON object | One line. Nothing in that unit is applied. | The unit is one record. Its turns and tool calls belong to it. |
| Save unit whose header is sound and names this chat, but whose chain, generation, or base does not follow | The header is adopted so a later unit can apply. The body is read as above. | Append after a gap. The chat is still this stream. |
| Save header that does not name this chat, or whose digest does not match | One line. The header is not adopted. | It is not a later unit of this chat. |
| Physical frame that is not a frame, including an unknown tag or a bad seal | One line for the run of bad frames. The next frame is read. | The next record is the boundary. A byte scan is not. |
| Change whose turn was never accepted, once a gap exists or the chat was never opened | Not applied. No extra line. | It belongs to the record that was skipped. |
| Observation of a turn that is present, when that observation itself cannot be applied | One line. The turn stays. | The turn was a different record and it could be read. |
| Lease body that does not decode, or a lease change the current lease refuses | One line. A held lease becomes unreadable; its revision stays. | A lease is not the chat. Unknown kinds already keep the chat. A note in the same unit does not keep the held lease when the lease is the change the fold refused. |
| Lease of a kind this build does not know | Applied as unreadable, and one line. | Same lease rule. The person still sees that something was skipped. |
| Decoded `Opened` that the session accepts, whose session id is not this stream | The page is `conversation_state_unreadable` (`TranscriptError::Scope`). | That open would name a different chat, so the page is not published. An open that does not decode is a skip: the stream already names this chat. A later open the session fold refuses is a skip too. This is the only saved record that refuses the page. |
| Unit that stages a snapshot whose id is not this stream | One line. The staged unit is dropped. The page continues. | Invalid data that got as far as a snapshot. The wrong identity is not kept. The open above is refused before it is staged, so this row is the check after a stage that was not that open. |
| Checkpoint whose snapshot id or scope is not this stream | The checkpoint is refused (`Scope`) and the chat is read again from its records. | Not a record of this chat. The fold was asked to resume a different one. Replaying the records decides the gaps. |
| Observation of a source head for another scope, or a fold built for another physical schema | `Scope`, and nothing is read. | Not a bad record. The caller named a different chat, or a schema this fold does not read. |
| Record whose scope is another stream | One line at that position. The next record is read. | Invalid data. The bytes are this position of this download; they are not a reason to hide the records around them. |
| Position that is missing, repeated, or ahead of the downloaded prefix | Retryable I/O (`Position`). | The bytes may still arrive. This is not a record that cannot be read. |
| Store, busy, and I/O failures | Unchanged. | The records were not read, so there is nothing to skip. |

A chat whose every record is a gap still opens. Its transcript is empty except
for the lines.

### Where a gap is remembered

Gaps live on the fold, its checkpoint, and `CommittedSession`. They are not
fields of `SessionSnapshot`. The checkpoint requires the gap list and whether
it was cut, so a checkpoint written before this change is reread from the
records. The list it stores is the prefix published with the last complete
save, oldest first, and the cut flag as of that save. A gap noted after that
save is left out; replaying the suffix notes it again. At most 32 lines are
kept. A later gap is still skipped, adds no line, and sets `unreadableTruncated`
when the save that follows it completes. The id is the decimal physical position of the record. A checkpoint is reread
from the records when its ids are not strictly increasing positions, or when
a gap's position is past the applied cursor. `afterExecutionId` is the last
accepted turn at that moment, and it is absent when there was none. A
duplicate of an id already kept does not set the flag.

The view carries `unreadable`: an array of `{ id, afterExecutionId? }`, always
present, possibly empty, and `unreadableTruncated`: a boolean, always
present. An empty committed transcript may still contain these lines. When
the flag is true the transcript ends with "More parts of this conversation
couldn't be read", after the lines and the turns.

## Alternatives considered

Refusing the chat, which #717 does, hides every turn that can be read because
one record cannot. Keeping that behavior leaves the person with no transcript
and no way to see which part failed.

Inferring which later records depend on a skipped one, and storing that graph,
is the shape #695 grew to thousands of lines to express. A later record is
hidden only when it names a turn that is neither in the snapshot nor accepted
earlier in the same save unit, and a gap is already present. An open earlier
in that unit is the session. That is the whole dependency rule.

Putting the gap on `SessionSnapshot` would make every snapshot literal and
every adapter carry a display fact. The snapshot remains the session the fold
could apply.

A second schema version, or a migration that rewrites old records, is a
compatibility shim. Alpha deletes old data by hand. A record that does not
decode is a gap.

Scanning the byte stream for the next header would resynchronize inside a
payload. The next physical record is the boundary the frame codec already has.

## Consequences

A chat with a torn record opens. The torn part is a line, and a turn that was
saved after it is shown when that turn's own record can be read. A checkpoint
from before this change is rebuilt from the records once.

The fold no longer treats a malformed frame, a contradictory decision, or a
record from another stream as a reason to refuse the page. A position gap
still refuses the page, because those bytes may still arrive. The one saved
record that refuses the page is a decoded open the session would accept for
another session id. Reviewers should treat a new `Err` on the read path as a
new fatal case that needs a row in the table above.
