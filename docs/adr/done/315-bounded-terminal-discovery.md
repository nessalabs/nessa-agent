# 315. Product record reads discover the committed head in bounded, resumable steps

## Purpose

A product read of a conversation's records must cost a bounded amount of work,
however long the stream's history is. This record settles who owns terminal
discovery progress between reads, what it may retain, and what a bounded read
may answer instead of a head. It is the SDK prerequisite for authorized record
reads ([#296](https://github.com/nessalabs/nessa-agent/issues/296)) and the
shared transcript fold ([#277](https://github.com/nessalabs/nessa-agent/issues/277)).
The [bounded terminal discovery design](../../design/bounded-terminal-discovery.md)
holds the exact bounds, ordering table and sequence.

- **Date:** 2026-09-30
- **Status:** accepted
- **Issue:** [#315](https://github.com/nessalabs/nessa-agent/issues/315)
- **Implemented in:** [#319](https://github.com/nessalabs/nessa-agent/pull/319)

## Context

A fresh SDK record source validated framing from offset zero, so discovering the
committed head or checking a page target scanned the whole physical prefix. A
product request builds a fresh source, so every request repeated that scan, and
a read timeout could leave one scan running while the retry started another.
Bounding the returned page did not bound this work.

The binding constraint: a bounded product read must not hide an unbounded
physical-prefix scan, and retries must not create competing full scans. A
committed head must still never be an unvalidated physical tail, because
receivers fold from it.

## Decision

Product callers discover terminals through `NessaRecordSource::bounded_head` and
`bounded_page`. Each call validates one finite step (at most sixteen frames, one
MiB of returned accounted bytes) against a physical tail captured when the pass
began, then answers `Ready` with a fully validated committed head or a typed
`RecordReadStatus::Preparing`, which exposes no physical or partial head. The
generic sync `RecordSource::head` is unchanged: it still returns a fully
validated committed head and never `Preparing`.

Progress between steps has one owner: `RecordStorage`'s process-local
`TerminalCache`, at most sixteen entries keyed by the exact `StreamKey`,
incarnation included. An entry holds only a validated offset, the last terminal,
the captured tail and the shared `FrameValidator`'s header and hash state — no
semantic body and no worker. A step checks its entry out exclusively before I/O
and a drop guard returns it on completion, error or unwind. A second operation
on an occupied key, or a cache whose sixteen entries are all occupied, gets
`Preparing` without doing physical work; only idle entries are evicted.

Progress is never reused across physical histories. A reset produces a new
incarnation and so a new key; the old source refuses with `IdentityChanged`.
A retention floor above zero refuses with `Pruned`. A tail behind validated
progress refuses with `IdentityChanged`. A malformed frame refuses without
advancing the validated offset or hash.

An abandoned answer does not cancel the step: its progress returns to the cache,
and a later call continues from it. Eviction or a process restart repeats
bounded steps from zero and reaches the same committed head; neither can turn an
unvalidated tail into a semantic head.

## Alternatives considered

- **Scan the full prefix on each request:** the behaviour this replaces. Work
  grows with history and every retry starts another full scan.
- **One long-lived worker per stream holding progress:** an idle thread per
  conversation, with its own lifetime and shutdown to own. The cache keeps only
  metadata; a source's worker lives no longer than the source.
- **Return the raw physical tail as the head:** cheap, but a partial fact would
  become a head receivers fold from. `Preparing` says "not yet" instead.
- **Cache semantic bodies alongside progress:** memory proportional to fact size
  and a second owner of decoded content. Discovery needs only framing evidence;
  body consumers keep their own storage.
- **Make `RecordSource::head` itself bounded:** would change the sync engine's
  generic contract for every source. The bounded path is a product addition.

## Consequences

Product reads have finite per-call work, and retries resume rather than compete.
Memory is finite: sixteen entries of fixed metadata.

Accepted costs: after eviction or restart the same bounded steps run again. With
more than sixteen streams under discovery at once, callers see `Preparing` until
an entry frees. A pruned stream is refused outright rather than read from its
floor. Bounds count decoded and returned records, not disk I/O or time: there is
no wall-clock guarantee for a stalled disk, and an abandoned caller does not
interrupt SQLite.

Not decided here: transport read permits, shutdown and holding the runtime
through worker completion belong to #296; semantic folding and applied
checkpoints belong to #277.

Revisit if the sixteen-entry cache yields `Preparing` under normal load, if
repeated cold validation after restarts dominates read cost, or if readers need
history above a retention floor.

## Evidence

Tests named in the design's ordering table:
`crates/nessa-sdk/tests/infrastructure/session_storage/terminal_discovery.rs`
(real SQLite: step limits, resume across recreated sources, appends during a
pass, abandoned answers, cold restart in a child process, occupied and full
cache, reset and pruning, corruption and byte-limit lookahead), plus the inline
tests in `crates/nessa-sdk/src/infrastructure/session_storage/record_source.rs`
and `stream_fact.rs`.
