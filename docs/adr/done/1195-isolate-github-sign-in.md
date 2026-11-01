# 1195. The GitHub token is only in the unlocked vault

## Purpose

An agent command, the managed shell, and a Claude or Codex process run as the
gateway's OS user. They must not be able to read Nessa's GitHub token by
opening a path they have learned. The token stays in the passphrase-sealed
app-session vault, the same place as the SQLCipher inbox key. A plaintext
sign-in file is not a store.

- **Date:** 2026-11-01
- **Status:** accepted

## Context

After [#1196](https://github.com/nessalabs/nessa-agent/pull/1196) the sign-in
is an encrypted vault entry. A leftover `<namespace>/github/sign-in.json` was
still plaintext, mode 0600, and readable by that same user until a later
unlock imported it. Mode 0600 is not a boundary between processes that share
the user. The remote MCP token store seals its blobs with a key file in the
same directory; that key is readable by the same user, so the pattern does
not fit here. The inbox key does: it is random, it lives in the vault, and
the vault key is derived from the passphrase and held only in the gateway
while the vault is unlocked.

Untrusted, for this decision: agent commands, the managed shell, and the
Claude and Codex provider processes. The gateway process is the only reader
of the token bytes. OpenCode is out of scope.

## Decision

The only durable copy of the GitHub token is the `github-sign-in` entry in
the encrypted app-session vault. The passphrase-derived key is not written
to disk. `sign-in.json` is removed without being read, including while the
vault is locked, on load, save, and disconnect. Nessa is alpha, so an older
plaintext file is not imported. Sign-in and disconnect keep their existing
audit records.

## States

| State | Where the token lives | Who can read the token bytes |
| --- | --- | --- |
| Signed out | Nowhere. A leftover `sign-in.json` is deleted and not read. | Nobody. |
| Signing in | Gateway memory only, until the vault write is verified. | The gateway. No file contains it. |
| Signed in | Ciphertext in `<namespace>/vault/entries.enc`. Plaintext in gateway memory while the vault is unlocked. | The gateway. A same-user command can open the ciphertext file and does not obtain the token. |
| Token in use | Gateway memory, on the gateway's own request. The agent receives the response body. | The gateway. Not the agent command, the managed shell, or Claude or Codex. |
| Disconnect | Vault entry deleted, and the plaintext path removed, before signed-out is published. A failed delete leaves the sign-in in place. | Nobody, when delete succeeds. |
| Gateway restart | Ciphertext on disk. Memory is empty until the passphrase unlocks the vault. | Same as signed in, after unlock. While locked, memory holds no token and the file does not hold the token bytes. |

## Invariants

| Guarantee | Enforcer |
| --- | --- |
| No untrusted process can read the token bytes from a path it knows. The key is not stored beside the ciphertext. | Vault encryption. `an_agent_command_that_knows_the_path_cannot_read_the_token` runs `/bin/bash --noprofile --norc -c` (the managed shell's program) against the sign-in path, the vault file, and every file in the namespace. |
| A plaintext `sign-in.json` is never the store, including while the vault is locked. | `GithubTokenVault` deletes it before it answers. `a_plaintext_sign_in_is_removed_and_not_adopted` and `locked_vault_removes_plaintext_and_refuses_save_or_disconnect`. |
| Disconnect removes the vault entry and the plaintext path. A failed delete does not publish signed-out. | Existing disconnect ordering. The store deletes the file before the vault entry. |

## Alternatives considered

- **macOS Keychain, Secret Service, or Credential Manager for the token.** The same user can still be allowed to read the item, the prompt is hard to automate, and the vault is already the cross-platform store under app data.
- **A filesystem sandbox around commands (Seatbelt, bubblewrap, or AppContainer), which is what Claude Code and Codex use for their bash tool.** It is a different mechanism on each OS, those tools do not ship one on Windows, and it does not cover the provider's own file tools. It is a larger change than the leak.
- **Keep the token only in gateway memory and sign in again after every restart.** The connect contract already restores a sign-in from the vault after unlock. Dropping that does not make the file safer than encryption already does.
- **Seal the file with a key stored next to it, as the remote MCP token store does.** The same user opens the key and the ciphertext and decrypts. That pattern loses.

## Consequences

Restart still restores the sign-in after the vault is unlocked. A namespace
whose only copy was `sign-in.json` is signed out until the person connects
again. While the vault is unlocked the token is in gateway memory, because
the gateway has to send it. A same-user process that can read that memory
(for example by attaching a debugger) is outside this decision. Sign-in and
disconnect audit are unchanged: they still record the account, the caller,
and the cause, and they still do not record the token.

## Review brief

Review `cursor/isolate-github-sign-in-4ae9` against `main`. The tree under
review is the committed head. Intended behavior is the state table above.

Scope: `GithubTokenVault` in
`crates/nessa-server/src/github/infrastructure/token_vault.rs`, the module
comment in `github/infrastructure/mod.rs`, this record, and the sign-in
sentences in `docs/design/github-connect.md` and
`docs/state/services/gateway/github-reads.md`. Out of scope: the device-flow
states, brokered reads, the SQLCipher inbox, the remote MCP key file, and
sandboxing Claude or Codex.

Confirm a real `/bin/bash --noprofile --norc -c` (or `powershell.exe` on
Windows) that is given the sign-in path and the vault path prints no token
bytes, that a locked vault still deletes `sign-in.json`, and that a failed
vault read is still unavailable rather than signed out. A store that imports
the plaintext file, or that leaves it in place while locked, is a miss.
