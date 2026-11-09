# Final source-checkpoint documentation review for #1158

## Result and reviewed source

**No open findings at any priority in the scoped documentation delta.**

Reviewed the uncommitted changes in these three pages in
`/Users/nessa/.codex/worktrees/github-webhooks-1107/nessa-agent`, against committed
source `ada97db780cd9457f6e1a5efdaa30eda2e64f1d3`:

- `docs/state/services/gateway/local-vault.md`
- `docs/state/services/github-webhooks/README.md`
- `docs/state/services/gateway/channel-workspaces.md`

Ordered path-NUL-content SHA-256 for those reviewed page bytes:
`e1c0c0dec5bcc358e736b5dd7eb13341c63ed17513961969c90b86274d0d838c`.
The exact source checkpoint exists and all three frontmatter values name it.
No unrelated page checkpoint was advanced in the reviewed delta.

## Finding and closure

One minor documentation overclaim was found and corrected during this review.
The first new webhook paragraph said that database work retained the unlock
identity and that delayed work could not publish into a later unlock. That
incorrectly conflated credential-generation fencing with an already-admitted
SQLCipher transaction. `SqliteRelayStore::run` obtains a key before executing its
bounded database operation; account identity and binding generation govern relay
admission and cursor commits. They do not cancel a transaction merely because
the vault locks after key acquisition.

The corrected webhook paragraph now limits the unlock-generation statement to
credential reads/writes, explicitly allows an already-admitted inbox transaction
to finish after lock, and says that new key acquisition requires unlock. The
local-vault page similarly narrows “cache and storage work” to “credential cache
and secret-entry work.” Verified against `VaultSecret`'s `GithubSecret` methods,
`GithubVaultAccess`, `Vault::read_in/write_in/delete_in`, `SqliteRelayStore::run`,
`ConnectedRelayStore::current/commit`, and the SQLite cursor/binding-generation
checks. This is closed as a documentation correction, not a newly demonstrated
cross-user disclosure or a requirement to cancel an admitted transaction.

## Source and contract checks

- Compared each page's linked source since its former checkpoint. The local-vault
  linked implementation is unchanged from its prior source checkpoint; inspected
  current owner/session close and explicit-lock publication fences, native retained
  pipe ownership, UI provider, fixed KDF/passphrase contract, and encrypted-file
  implementation for the unchanged lifecycle claims.
- Verified routine GitHub sign-in and inbox key composition use encrypted vault
  entries. Legacy OS reads remain limited to migration capture for pre-vault
  namespaces. `verify_and_retire` checks required keys and exact encrypted receipts
  before the source adapter retires its entries; product setup/unlock locks again
  when retirement verification fails. The corrected prose no longer describes
  ordinary credentials as OS-store entries.
- Checked receiver success/refusal mappings, relay read authorization/audit order,
  page/cursor commit order, catch-up/watch subscribe-and-re-read ordering, stage
  defaults and shared retention contract. The page continues to distinguish
  webhook plaintext at Cloudflare, managed at-rest retention, bounded Free
  capacity, and local SQLCipher encryption. It makes no end-to-end encryption or
  unlimited-delivery claim.
- Verified `worker_image()` gates the published pin on exact compiled protocol
  equality, `Workspaces::connect` refuses absent matching image, and
  `shipping_worker_covers_the_compiled_lease_protocol` exercises that shipping
  gate. The new workspace paragraph describes readiness rather than claiming a
  particular provider deployment/inference result. Existing assignments retain
  their original image; no fallback or protocol alias was added by these docs.
- Reviewed the new key-acquisition statechart against key lookup, migration and
  SQLCipher open paths. Its prose explicitly preserves admitted work across lock.
  Existing page IDs, parent links and ownership remain unchanged. All **52 source
  path references across these three pages exist**. `git diff --check` passed.

## Limits

This is a source-grounded documentation and diagram review, not a fresh review
of the entire stacked implementation. No Cargo, browser, native GUI, provider VM,
Cloudflare deployment or hosted CI was run by this reviewer. No source or docs
were edited by this reviewer. The coordinator owns catalog/Mermaid rendering
validation, real-provider evidence and final runtime/CI results. Runtime statements
in the pages retain their stated macOS and historical deployment limits; a source
checkpoint does not certify all platforms or future deployment availability.
