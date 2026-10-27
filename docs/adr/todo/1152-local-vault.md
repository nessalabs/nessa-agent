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
| A consumer fails to restore after unlocking | Vault unlock remains truthful and reusable; that consumer reports its own storage failure |
| Different namespaces / independent test instances | No shared keys, sessions or mutable process globals |

## Consequences

The passphrase cannot be recovered by Nessa. Reset/recovery is not automatic and
must not erase existing encrypted data. Historical plaintext backups remain
outside this migration. Only the GitHub/inbox keychain introduced by #1137 is
replaced now; the existing provider API-key consumer remains a separate future
vault integration. The vault's internal named-entry port supports that future
consumer without exposing secret values to the UI or adding it now.

An explicit lock increments the vault publication generation under the state mutex. A key derivation admitted before that lock cannot publish afterward, even while the same connection remains alive; a fresh unlock is required.

Round-one adversarial review adds these orderings before repair:

| Ordering | Required result |
| --- | --- |
| Owner closes while the publication audit is writing | Retain an unlock then compensating key-cleared record; inaccessible key is cleared even on audit failure |
| Device-code response arrives after lock, including lock then re-unlock | The old vault generation cannot publish Connecting or start polling; retain the admitted attempt's termination cause |
| Process restarts between audit writes | Persist an injected UTC observation time as well as the durable sequence |
| A secret value is formatted for diagnostics | The reusable wrapper renders only a redaction |
| UI recovery read completes after remount or a newer submit | Recheck request and revision after every await; only the current operation can publish |
| Native command deadline elapses while its child blocks | Cancel that request ID and kill only its owned child before returning; a late start checks cancellation, and later requests keep distinct IDs |

Native passphrase ownership is zeroizing from deserialization, before startup waits or early refusal.

## Consumer ownership revisit after review round two

The generic vault owns the unlock generation. GitHub may not infer session identity
from a boolean. Its retained sign-in and pending attempt each carry the immutable
generation that admitted them; restore captures one before IO. A stale generation
is retired before status, token, provider completion or a new restore can use it.
The credential-store port carries that same generation into every load/save/delete.
The vault compares it under the key mutex, atomically with entry access, so an old
completion cannot write or delete a new session's credential.

Cleanup is independent of the command's audit outcome. The unique connection guard
and explicit lock route both retire stale GitHub state even when key-close audit
fails. Retirement takes and aborts an old attempt once, retaining its original
operation/caller and a vault-locked termination cause. GithubAudit exposes a
blocking cleanup delivery for this synchronous lifetime boundary; its durable
adapter uses the same file writer as ordinary asynchronous records. No detached
UI task or unbounded cleanup queue owns this receipt. Failure clears secrets,
returns an observable refusal on explicit lock, and logs on a departed connection.

| Ordering | Result and owner |
| --- | --- |
| Lock then re-unlock before old cleanup runs | Compare held generation with current generation, retire only the stale cache |
| Old restore load answers after a new unlock | Drop its receipt; publish only matching captured generation |
| Old poll token save or rollback reaches storage after re-unlock | Vault rejects old generation atomically; newer entry survives |
| Key-close audit fails while sign-in is pending | Clear key, abort attempt and attempt its attributed durable termination independently |
| Repeated close/status/restore observes the same stale attempt | State is taken once; retain one termination receipt and its original caller/cause |
| Cleanup audit fails | Drop token/code anyway; explicit caller sees audit failure; no fake successful cleanup receipt |

Terminal sign-in ownership is transferred out of Connecting under its state mutex
before an asynchronous completion/end audit. Retirement and provider completion
therefore cannot both claim the same pending operation. During final persistence
the projection is signed out; only a matching generation's confirmed receipt
publishes Connected. Polling checks its operation/generation before each provider
request and after timer waits. An already admitted provider call may finish, but
its stale result cannot advance to another call or mutate storage.

Restore does not wait for the provider-command mutex: a departed session's network
request must not hold up a reusable vault unlock. Cache publication is one
application-owned compare-and-publish operation under the state mutex. Provider
success, provider failure, rollback, end and cancel all use that operation with
their captured generation. A restore never replaces a current-generation pending
or connected state. Its generation-scoped store load can finish independently;
an old command cannot then clear or overwrite the restored newer cache.

## Dialog placement

A protected-use hook captures the UI kit's nearest portal container. The root
vault provider owns one dialog but renders it in that requesting surface's layer;
a body portal would be hidden behind Settings. The GitHub row requests unlock
inside Settings, rather than in its root data provider. Leaving the surface
releases that placement and cancels an unsubmitted request; app-session key
ownership remains in the native host. No CSS z-index lookup chooses a surface.

A gateway restart can leave the retained child waiting on an already-closed
product socket. A redacted transport-unavailable reply drops that child. Status
may establish a fresh connection and retry once within the same native deadline;
setup/unlock/lock are not replayed. Cancellation is checked before any retry, so
a timed-out old request cannot create a replacement. A Settings-triggered unlock
uses its portal layer for both the unlock form and transport refusals.

Explicit lock fences publication and clears retained key state in one mutex scope; an observer cannot obtain a new generation for an old retained key between those effects.

## Round-three structural correction

Vault generation proves session identity, but does not order operations within
that session. Restore previously bypassed the sole mutation gate to avoid waiting
for a departed provider call. That exemption allowed a same-session restore to
revive a token after disconnect. Replace the process-wide mutation gate with a
generation-owned gate: restore, connect, disconnect, cancel and poll termination
all serialize within their captured generation; a new generation has its own gate.
Revalidate generation after waiting and keep atomic generation checks in storage.

| Ordering | Required result |
| --- | --- |
| Restore captures token, then disconnect arrives | Disconnect runs after restore publication and deletes both cache and persistence |
| Restore fails while connect arrives | Connect runs after restore refusal and retains its own attributed attempt |
| Old provider blocks while a new session restores | Different generation gates allow new restore; old publication remains fenced |
| Retained audit has illegal values or sequence | Refuse before replacement or retention; preserve original evidence bytes |
| Valid retained suffix starts above one or wall clock reverses | Accept; durable sequence orders observations, not wall-clock monotonicity |

Audit restoration validates the same bounded row contract as append. It checks
legal action/cause/result and target/state combinations, identity bounds and
contiguous durable sequence. It does not invent global ordering across concurrent
leases, authenticate same-user file edits, or infer absent rows outside retention.

## Approval receipts remain pending in the public status

CI rerun40446076479 reported a later GitHub-row failure after provider approval.
The following source ordering matches that symptom; its regression is tested
independently of attributing a precise interleaving to the hosted run.
The completion path took the pending attempt into its own terminal ownership,
then awaited audit and encrypted-storage receipts while projecting SignedOut.
A Settings poll during that interval stopped polling and could miss the later
Connected publication. Returning SignedOut before either receipt was a false
terminal result, rather than an authorization or persistence failure.

Keep ownership of the terminal operation in the completion task, but project an
explicit Saving state as Connecting while those receipts are pending. The saving
projection retains only immutable generation/code/deadline information; it does
not own another poll task or another terminal audit. Taking the attempt and
installing that projection must happen under the same state mutex. The existing
per-generation mutation gate still orders disconnect/cancel/restore with save.

| Ordering | Required result |
| --- | --- |
| Provider approved; Connected audit is waiting | Status stays Connecting with the same code; no token is exposed |
| Audit accepted; encrypted save receipt is waiting | Still Connecting; only both receipts permit Connected/token |
| Audit or save fails | Publish the existing actual failure as SignedOut; do not leave a permanent pending projection |
| Vault locks or its owner closes during save | Drop stale Saving projection; original completion owns remaining evidence, and its old-generation result cannot publish |
| Disconnect arrives during save | Same-generation gate orders it after save, then deletes the resulting stored credential |
| Denial, expiry or cancel | The existing terminal paths continue projecting SignedOut; Saving is specific to approved credentials awaiting receipts |

## Mainline integration before landing

The completion integrates current main at
`d0d253e0201d8644a47f001df97e17f39bb92545` into the reviewed vault/relay/Boat
stack. It preserves both existing contracts; it does not add a second credential
owner or relax approval, retention or stage boundaries.

| Concurrent feature boundaries | Integration invariant and verification |
| --- | --- |
| Mainline lease sources and Boat release worker | Publish a credential-free image for the exact compiled fingerprint; require the release worker to be available in CI and verify a fresh clone before pinning |
| Mainline web tunnel and relay TLS | Keep inbound WebSocket handshake plus outbound TLS; retain both typed product routes and CLI options |
| Mainline setup/credentials and app-session vault | Vault remains the GitHub/inbox credential owner; lock/close fences old generations; provider API-key storage remains separate |
| Channel file drops and Boat placement | Keep both independent UI groups and their browser checks; no implicit file mounting |
| CI runner changes and vault coverage | Keep the shipping Linux baseline and all eight vault-inclusive package selections; retain current aggregate checks |
| Mainline paired-channel catalogue fixture and Boat execution | Construct the existing Local execution value in the new fixture; retain grant-scoped channel IDs and prove the real socket test still compiles/runs |
| Mainline focus repair and protected-use dialogs | Keep #1171's bounded close callbacks and exact focus assertions; current main's linked-device contract remains intact |

Generated Rust/TypeScript product payloads are rebuilt from the combined schema
and manifest, not resolved by choosing one generated side. The final integration
gets a fresh source review, affected local checks and hosted full-platform CI.

## Retention-gap wire contract before mainline completion

The late external review found that Worker tests checked HTTP410 while native
catch-up tests injected a typed Gap directly. Neither crossed the actual JSON
boundary: the producer sends `expiredThrough`, but the native decoder expected
snake_case. This is a missing shared boundary receipt, not a new gap policy.
A shared protocol fixture now holds the producer/consumer spelling in place.

| Incoming receipt | Required outcome |
| --- | --- |
| Current-stage HTTP410 gap with camelCase watermark | Decode the typed gap; persist the encrypted gap before reporting it; reopening remains paused |
| No explicit owner acceptance | Keep the gap and original cursor; no replay request can silently skip it |
| Gap/acceptance audit sink refuses | Roll back the transition and retain its prior cursor/gap; replay succeeds after recovery |
| Explicit existing owner acceptance | Advance only to the stored watermark, then allow the next valid page to commit |
| Wrong stage/kind, missing or snake_case watermark | Preserve stage/type refusal; leave cursor and inbox unchanged; no compatibility alias |

The loopback transport tests use the injected HTTP client and private adapter
state; they do not weaken the shipping constructor's HTTPS-only origin contract.
TLS itself is outside this serialization regression. The worker release protocol
inputs are unchanged, so its published image does not need replacement.
