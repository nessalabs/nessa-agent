# Local passphrase vault

This reusable library owns passphrase validation, one connection's unlock lifetime,
and named entries in an authenticated encrypted private file. It has no provider,
UI, keychain, or network dependency. [ADR 1152](../../docs/adr/todo/1152-local-vault.md)
owns the ordering table; application integration is tracked by #1152.

```text
connection owner -> Session -> Vault -> Repository <- EncryptedFileVault
request clones  -> Lease      |           |
                              +-> Audit <- FileAudit <- ObservationClock
                              +-> Clock
```

Arrows name calls/ownership or port implementation. A request's lease clone does
not keep the unique connection owner alive. Session close invalidates admission
before an in-flight derivation may publish. Other sessions cannot take over a
live unlock or cancel its pending derivation (`another_connection_cannot_cancel_an_unlock_still_deriving_its_key`). Repository IO is blocking and composed onto a bounded blocking
worker by its consumers. Already-admitted repository operations may finish. Generation-scoped entry access
compares its captured unlock identity under the key mutex; stale consumer work
cannot read, overwrite or delete a newer session's entry.

The file format selects Argon2id with the fixed bounded parameters in
`protocol/defaults/vault-contract.json`, then XChaCha20-Poly1305 with a fresh
24-byte nonce per write. Salt and format identity are authenticated associated
data. The encrypted map owns its secret allocations; the derived key, owned Argon2 block allocation and transient
plaintext/base64 buffers zeroize when dropped. Argon2 scratch is passed explicitly
through `hash_password_into_with_memory`, with the dependency zeroize feature
enabled; its allocating convenience method does not erase that allocation. No key or passphrase is serialized.
Private temp publication, directory sync, receipt verification and a shared file
lock preserve durable truth on retry and competing writers. Wrong input and
corruption preserve ciphertext rather than initializing a replacement.

The metadata audit is separate and writable while locked. Its bounded journal
records caller identifiers, connection/operation identity, transition, cause and
result and injected UTC observation time, and contains no secret values. Secret Debug formatting is redacted. Audit failure refuses unlock publication;
close still removes its key before reporting audit failure. Restored rows must satisfy
the same action/state/target and identity contract as new observations, with a
contiguous retained sequence. Invalid history is refused before replacement or
retention; UTC clock reversal and valid suffixes starting above one remain valid.

`cargo test -p nessa-vault` exercises real encrypted files and audit persistence,
including connection-close/KDF races, audit faults, corruption, quota admission,
multiple writers and failed-attempt cooldown. Clock and IO admission faults are
substituted through public ports. The library does not implement UI or keychain
migration; those are owned by its consumers.

## Security boundary

The gateway is trusted with every entry while unlocked. The current format
uses the Argon2id-derived key directly; reading one entry decrypts the whole
map. Erasing owned allocations reduces residual bytes after use, but does not
exclude a process that can read gateway memory, swap, core dumps or a live copy.
This library does not install anti-debugging, memory-lock or process isolation
controls. Private file permissions separate OS users, not arbitrary processes
running as the gateway user.

One gateway namespace composes one vault and one unlock scope. Independent
namespaces get independent random salts. Namespace directories alone do not
provide cryptographic gateway isolation. The format does not provide rollback
protection or an authenticated, independently durable audit history. Deployment
isolation and a separately trusted freshness anchor are required before claiming
those protections.

Failed guesses are tracked per authenticated principal/credential pair, so a
reconnected connection retains its budget and separate controllers do not share
a cooldown. One global derivation permit still bounds KDF work. At most 1,024
compact identities are retained; active penalties and known successful-controller
slots are not evicted. An unknown controller at capacity receives Capacity while
tracked controllers remain eligible. Inactive unverified slots expire after a
full quiet cooldown interval. Restart clears these process-local budgets. This
is not an offline-guessing defense or a guarantee of unlimited first-time admission.

Setup/change validation uses its own fifteen-character floor, a public licensed
common-password list and periodic repetition checks. Existing unlock parsing
retains its original twelve-character bound. The finite check does not claim to
recognize every guessable phrase; no password is sent to a checking service.

Ordinary passphrase change uses the current file format and leaves the vault
locked after admission. A private temporary replacement must preserve and verify
all entries before publication. Storage commitment and audit acknowledgement stay
separate, including an uncertain replacement. A lost response must not be retried
automatically; the person can explicitly unlock with the new phrase, then the old
phrase if needed. Historical copies remain decryptable with their original phrase.
Recovery and second-factor enrollment are not implemented.

The gateway's Linux bootstrap separately establishes checked process-local core
and dumpability protections before command dispatch. This library does not set
process policy itself. Locked-memory/swap protection and full OS-user isolation
are separate deployment and allocation concerns.
