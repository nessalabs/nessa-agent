# Local passphrase vault

This reusable library owns passphrase validation, one connection's unlock lifetime,
and named entries in an authenticated encrypted private file. It has no provider,
UI, keychain, or network dependency. [ADR 1152](../../docs/adr/todo/1152-local-vault.md)
owns the ordering table; application integration is tracked by #1152.

```text
connection owner -> Session -> Vault -> Repository <- EncryptedFileVault
request clones  -> Lease      |           |
                              +-> Audit <- FileAudit
                              +-> Clock
```

Arrows name calls/ownership or port implementation. A request's lease clone does
not keep the unique connection owner alive. Session close invalidates admission
before an in-flight derivation may publish. Other sessions cannot take over a
live unlock. Repository IO is blocking and composed onto a bounded blocking
worker by its consumers. Already-admitted repository operations may finish.

The file format selects Argon2id with the fixed bounded parameters in
`protocol/defaults/vault-contract.json`, then XChaCha20-Poly1305 with a fresh
24-byte nonce per write. Salt and format identity are authenticated associated
data. The encrypted map owns its secret allocations; the derived key and transient
plaintext/base64 buffers zeroize when dropped. No key or passphrase is serialized.
Private temp publication, directory sync, receipt verification and a shared file
lock preserve durable truth on retry and competing writers. Wrong input and
corruption preserve ciphertext rather than initializing a replacement.

The metadata audit is separate and writable while locked. Its bounded journal
records caller identifiers, connection/operation identity, transition, cause and
result, and contains no secret values. Audit failure refuses unlock publication;
close still removes its key before reporting audit failure.

`cargo test -p nessa-vault` exercises real encrypted files and audit persistence,
including connection-close/KDF races, audit faults, corruption, quota admission,
multiple writers and failed-attempt cooldown. Clock and IO admission faults are
substituted through public ports. The library does not implement UI or keychain
migration; those are owned by its consumers.
