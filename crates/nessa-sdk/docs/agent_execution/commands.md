# Creation commands

`commands::CreationCoordinator` saves a verified host caller's immutable principal,
request, origin, target and canonical-input SHA-256 fingerprint on the existing
record runtime. Input configuration remains with the target's deletion owner.
The coordinator does not authenticate principals or infer authority from stored
receipts. Its `CreationTarget` consumer checks current access/deletion before any
saved readiness or interrupted response, and again after initialization.

The first actual host consumer is `ConversationService::create_command`, with a
shared `RecordStorage` supplied explicitly to that entry point. Product socket
activation, turn acceptance/Stop receipts and a client outbox are later increments.
The command waits for the original provider attachment and its publication;
ordinary conversation preparation alone is not a successful initialization.
`CreationInitializationFailure` preserves a host refusal separately from the
original initialization supervisor's `CreationTaskFault`.

| Saved state | Explicit create/retry | Read-only lookup |
| --- | --- | --- |
| Absent | Save original acceptance and attempt before initialization | Return absent without creating a stream |
| Accepted | Check current target, save first attempt, then initialize | Return accepted; no write or provider open |
| Attempted | Return the original interrupted attempt; no blind provider open | Return attempted evidence; no write or provider open |
| Ready | Return original receipt after current target check | Return original receipt after current target check |

One supervised task retains the original principal lease and initializer after
caller loss. The host retains its existing admission guard through the final
receipt write; retirement joins that guard before storage shutdown. Adapters
retain the original reservation while an append finishes after its caller stops
waiting. The host's original attachment owner retains its typed completion for
concurrent and later waiters; a consumed join handle is not success evidence.
Runtime termination cannot prove external provider cleanup or an original
provider result. An attempted record without a saved success remains interrupted;
the current provider has no lookup by initialization command identity.

The supplied adapter uses the same SQLite runtime as session records, not a second
receipt database. Control records are one bounded atomic event each. Replay reads
bounded pages but scans the full retained control history, retaining at most 4,096
request bindings per principal; another binding returns `StorageError::TooLarge`.
At most `commands::MAX_CREATION_OWNERS` principal leases coexist. Capacity or a
competing owner returns `StorageError::Busy` without a queue. These limits do not
bound all request objects already allocated by a caller or total on-disk history
across principals. Bindings remain after target deletion and are not collected by
this slice. No transaction orders control and target streams together.

The owning state/order table and regression fixture mapping are in
[command creation](../../../../docs/design/command-creation.md).
