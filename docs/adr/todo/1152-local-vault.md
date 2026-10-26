# 1152. A passphrase unlocks one app-lifetime local vault

- Date: 2026-10-26
- Status: accepted; implementation in progress
- Issue: #1152; builds on #1137

## Decision

One namespace-scoped vault owns reusable named secret entries. GitHub sign-in
and the separate SQLCipher inbox key are its first consumers. A longer passphrase
(minimum 12 characters, user-approved) derives the vault encryption key with
Argon2id; authenticated encryption seals every durable entry. No passphrase,
unwrapped key or decrypted entry is saved in configuration, UI persistence or
logs. Tests compose private temporary vaults and injected entropy, without OS
keychain prompts. Cloudflare remains Free with managed at-rest encryption.

The gateway owns vault persistence and its narrow internal secret-entry ports.
Only setup/unlock/lock/status cross the existing authenticated product socket;
there is no read-secret wire operation. Existing Cedar credential-management
admission authorizes those operations. A retained native-host product connection
owns the unlock lease, across onboarding, window hiding and webview reloads.
Closing that connection, including host crash, clears the gateway's vault key.
A browser development surface owns its own connection lease. A gateway restart
also requires unlocking again. Already-admitted operations may finish; credentials
already given to an agent cannot be recalled from its process environment.

Extract the existing local product transport before changing behavior. The native
host retains a `nessa vault session` child over private bounded stdin/stdout; its
product socket owns the unlock. Closing the parent pipe closes the child and its
socket, including host crash. This reuses the Rust client through the bundled
binary without raising the native host's declared Rust 1.85 floor to the client
library's 1.89. Passphrases and credentials travel only over anonymous pipes, not
arguments, environment variables or files. UI dialogs use one reusable
vault port and hold passphrase input only in transient component state. Onboarding
sets up the vault; protected GitHub access requests an unlock when needed.
Existing OS entries are read only for an explicit one-time verified migration;
normal setup on a fresh namespace and normal operation never access a keychain.
An unavailable legacy key needed for existing ciphertext refuses migration and
preserves source data. There is no silent reset or plaintext key-file fallback.

## Ownership and ordering before code

| Case / ordering | Required result / regression |
| --- | --- |
| Fresh onboarding setup | Publish encrypted vault durably before reporting unlocked; confirm passphrase in UI |
| Weak, oversized or mismatched passphrase | Refuse before crypto/persistence; publish bounds from one contract |
| Two setup attempts / lost setup reply | Never overwrite; same passphrase can reopen the committed vault |
| Wrong passphrase / modified ciphertext / truncated file | Refuse, preserve bytes; no new key or empty-vault fallback |
| Concurrent guessing / long KDF | One bounded KDF at a time; typed busy and bounded retry cooldown |
| Unlock completes after session closes | Unique connection-owner guard invalidates lease independently of cloned request state; publication rechecks liveness |
| Native onboarding hands off or a webview reloads | Host retains the same lease, without persisting the passphrase |
| Full app quit / native host crash | Socket lease closes and vault key is cleared; gateway may keep serving |
| Unrelated socket closes / old lease closes after newer unlock | Cannot lock the newer lease |
| Vault locks during provider sign-in / inbox access | No new protected reads; fence stale sign-in publication and clear cached token state |
| Already-admitted DB work or running agent at lock | May finish; no claim to erase copies already handed to those owners |
| Competing secret writes / disk failure / crash after rename | Atomic private file replacement under shared lock; next operation reads durable truth |
| OS entry imported / crash before old entry deletion | Verify encrypted receipt before deleting source; retry cleanup without overwriting divergent values |
| Legacy key unavailable / existing encrypted inbox key absent | Preserve legacy and ciphertext; surface migration/unlock refusal |
| Disconnect while unlocked | Delete GitHub sign-in and retained payloads, keep inbox key for encrypted audit |
| Locked startup | Keep ordinary workspace available; ask for unlock when protected data is needed |
| Vault setup without any agent/GitHub configuration | Compose vault first; onboarding is independent of its consumers |
| Direct CLI relay command | Explicit passphrase stdin, process-scoped lease; no secret in argv or environment |
| A second live lease requests unlock | Refuse ownership transfer while the first lease is live; another socket close cannot seal it |
| Secret-free audit cannot be written | Refuse setup/unlock publication; close still clears keys and reports failure after effect |
| Audit evidence while locked | Bounded private metadata journal independent of vault key; actor, namespace, operation/lease, before/after, cause/result, no secret bytes |
| Different namespaces / independent test instances | No shared keys, sessions or mutable process globals |

## Consequences

The passphrase cannot be recovered by Nessa. Reset/recovery is not automatic and
must not erase existing encrypted data. Historical plaintext backups remain
outside this migration. Only the GitHub/inbox keychain introduced by #1137 is
replaced now; the existing provider API-key consumer remains a separate future
vault integration. The vault's internal named-entry port supports that future
consumer without exposing secret values to the UI or adding it now.
