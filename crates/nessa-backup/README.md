# Backup

A consistent export cut of the gateway's stores and the deletion inventory
captured beside it, the standalone bundle verifier, offline restore planning,
and quarantined restore (stage, accept, and fetching one deferred conversation
when it is opened).

```text
nessa-server ──▶ nessa-backup ──▶ nessa-auth, nessa-protocol, nessa-local-storage,
                                  nessa-local-database, nessa-timer
```

Arrows are compile-time dependencies. nessa-server imports it as
`nessa_backup`. What meets the conversation context stays in the server's
`backup/infrastructure/`: the export sources over the namespace's stores
(`capture_namespace`), both outboxes, the conversation service's admission
lock, and `CommittedInventory`, the `FenceCapture` the conversation context
calls after a deletion commits. So do the tests that drive a whole gateway
namespace, the three commands (`nessa-backup-verify`, `nessa-backup-plan`,
`nessa-backup-restore`) and their composition.

| Path | Responsibility |
| --- | --- |
| `src/domain/` | What a cut may hold, the manifest and inventory, verification metadata, and restore's paths, seal and fence omission. |
| `src/application/` | The capture order and its ports, the deletion inventory, verification, restore planning, and stage, accept and open. |
| `src/entrypoint/` | Argument parsing for the offline commands. |
| `src/infrastructure/` | Snapshots, the bundle publisher, the deletion log and its Cedar policy and decision audit, the decoders, the verification readers, and the quarantine. |
| `tests/` | Unit tests, each loaded by its source file, and the verification fixtures the server's command tests share. |

The `test-support` feature exposes the restore internals the server's restore
tests drive: `ordered_roots` and `promote_with_failed_source_retirement`.
Design: [export cut](../../docs/design/export-cut.md),
[backup verification](../../docs/design/backup-verification.md),
[quarantined restore](../../docs/design/quarantined-restore.md).
