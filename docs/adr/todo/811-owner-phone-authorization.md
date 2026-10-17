# 811. First-party phones hold an explicit owner conversation capability

## Purpose

An owner authorizes their phone once to read and manage current and future owned conversations. This capability preserves the existing per-conversation sharing rules for other devices and peers. Implementation remains in progress; the Auth foundation alone does not establish a connected phone or first Send.

- **Date:** 2026-10-13
- **Status:** accepted
- **Issue:** [#811](https://github.com/nessalabs/nessa-agent/issues/811)

## Context

Existing device pairing deliberately delegates one read capability and requires a share for each conversation. A freshly created empty conversation is absent from the product summary list, although real creation already publishes an empty transcript stream and committed snapshot. Sharing a list cannot provide owner authority over future conversations. A coarse conversation write action also covers ancillary administration, so action grants alone cannot express the requested phone boundary.

## Decision

Auth owns a separate OwnerPhoneReadWrite class, its complete ordered capability set, signed exact device request and durable original-request ledger. The exact original bootstrap credential admits creation, replay, staging and publication. The issued device uses its private native key with its public credential ID for protected product and native read connections. Canonical Auth enrollment and its remembered receiver receipt durably own purpose once; current delegated Auth and current receiver agreement establish the distinct OwnerPhone reader. The purpose Cedar policy consumes published method/action facts and default-denies ancillary administration; it also preserves configured Ask approval and refuses owner-local path links. Current signed purpose and original receiver holding admit a first-party reader only after the canonical linkage policy passes. The shared ConversationAccessPolicyEvaluator then decides actual conversation Read/Act/Locate and catalogue List from current metadata, including deletion. Device and peer readers remain separate paired subjects with their existing share roles. Existing peer/device share roles, tool approval and revocation remain separate authorities.

## Lifecycle

This table is the order for authorizing an owner phone and for the connection that follows. `SessionCloseReason` stays the wire reason. `connection_outcome` in `crates/nessa-protocol/src/product_contract/outcome.rs` is the only projection of a reason, or of a close that carried none, into what the client does next. `retryable()` stays on the reason and is not copied into that projection. A command that already happened is idempotent success; it is not a close and it does not end the session.

A wildcard listener is published as the configured `native.advertisedAddress` on the configured port. A specific bind is published as itself only after the same Auth endpoint validation. An absent listener, a configured port of zero, an unspecified address, or a wildcard with no advertised address discloses nothing. Binding port zero assigns an ephemeral port; that port is not published and is not stored in an authorization. An advertised address is a unicast host a phone can open TCP to. An IPv6 link-local address is refused for both bind paths: its numeric scope is local to the gateway and cannot identify an interface on the phone. Auth owns host validation for discovery, context construction, and signed request construction; composition adds only the wildcard family and non-loopback requirements. Concrete loopback remains available for local clients. IPv4-mapped IPv6 addresses are checked by their IPv4 host meaning. The IPv4 this-network block `0.0.0.0/8` and the IPv4 limited broadcast `255.255.255.255` are refused because a phone cannot open TCP to them. The process does not look up a route.

A phone `conversation.answer` may be one shot. Remembering a permission rule (`nessa.remember.allow`, `nessa.remember.deny`) is a separate purpose decision, `conversation.remember`, and this purpose forbids it. The same decision is what a phone send asks before it offers those choices. A desktop owner is not this purpose and keeps the permission-rule catalog.

| From | Event | Who | Client | Evidence |
| --- | --- | --- | --- | --- |
| no original | `phone.authorize` commits the signed request | owner | success; this is the first commit, not a repeat | `PairingTransition`: before Available, after Approved, event `OwnerPhoneApproved`, owner actor, time |
| Approved | the same `phone.authorize` bytes | owner | success; activation continues | may draw a stage and append `Stage`; it does not append another approval |
| Staging | the same `phone.authorize` bytes | owner | success; activation continues | may append the receiver and activation transitions still missing; no new invitation |
| Active | the same `phone.authorize` bytes | owner | idempotent success; no new entropy | no draw and no append |
| Terminal | the same `phone.authorize` bytes, or deny or cancel again | owner | idempotent success | the stored terminal record; no draw and no append |
| Available, Approved, or Staging | owner denies or cancels | owner | terminal revoke | `End(Denied)` or `End(Cancelled)`, owner actor; the first cause stays. Deny on Active is refused. Active cancel is the next row |
| Active | owner cancels, or the credential is revoked | owner, or the credential authority | terminal revoke | an Active cancel correlates `CredentialTransition` cause Explicit, same actor and second; an independent revoke is event `CredentialRevoked` |
| Available, Claimed, Approved, or Staging | the deadline is due | system | terminal revoke of that attempt; Active is left as it is | `End(Expired)`, system actor; Active and Terminal return no new transition |
| Available | startup loses the unapproved invitation | system | terminal revoke of that attempt | `End(Restarted)`, system actor |
| ready session | the credential is revoked or expired | credential authority | terminal revoke (`credential_revoked`, `credential_expired`) | `session.terminated`, then the socket closes; the credential transition is the audit; the close writes none |
| ready session | access is unavailable or a stale revision; host cleanup closes every product socket at process stop (`ProductRouteState::close_sockets`) | gateway | retryable (`temporary_unavailable`); the authorization stays | `session.terminated`; no pairing write. The shutdown report is separate and is not a session reason |
| ready session | any other access error, or a passive-read refusal | Cedar for the read; other access errors through `close_reason` | terminal revoke (`authorization_lost`) | `session.terminated`; no pairing write |
| ready session | a `session.terminated` whose code is `server_shutdown` | the peer that sent it | terminal shutdown; the authorization stays | the frame. This gateway does not construct that code when the process stops |
| any session | handshake timeout, restarting, overloaded, or the transport is interrupted | gateway or the transport | retryable | the close reason only |
| handshake | authentication fails | gateway | terminal revoke (`authentication_failed`); no Ready | no cache delete |
| handshake | the protocol does not match | gateway | terminal protocol (`protocol_incompatible`); the authorization stays; this is not a revoke and not a shutdown | no Ready |
| retained refresh or follow | native session authentication refuses, or the connection closes with a reason | gateway; retained reader consumes the shared projection | only terminal revoke asks pinned status and reports `Refused`; retryable and terminal shutdown report `Unreachable`; terminal protocol reports `Protocol` | the original reason stays transport evidence; the failed read itself does not purge or end authorization |
| retained conversation records | discovery or page reading ends with a published connection outcome | physical connection owner, before per-conversation error handling | the projected failure ends the whole refresh; it cannot become successful incomplete conversation progress | the typed connection cause stays distinct from a conversation's source/shape failure |
| any session | EOF and no valid `session.terminated` | transport | retryable; not a revoke | none; the client does not invent a reason |
| any session | a malformed `session.terminated` | peer | not a client class; a protocol failure; the authorization is unchanged and the event is not queued | none |
| owned conversation | `conversation.delete` | owner or the phone | success; the session stays up | `ConversationDeletionAuditRecord`: conversation, before Owned, after Deleted, `CallerRequested`, initiator, surface, correlation |
| tombstone committed | the same delete | that caller | idempotent success | the first audit record; a contradicting record is refused; erasure resumes from the tombstone |
| owned conversation | phone `conversation.answer` once | phone | the review is answered; this is not a remembered rule | method `conversation.answer` stays permitted. A failed membership read still answers once |
| owned conversation | phone `conversation.answer` with `nessa.remember.allow` or `nessa.remember.deny` | phone | refused; no rule is stored and the answerer is not noted | Cedar forbid `owner-phone.remember` on `conversation.remember` |
| owned conversation | phone `conversation.send` or `conversation.steer` | phone | the turn is not offered a remember choice | the same `conversation.remember` decision |
| listener | wildcard bind, a fixed port, and a routable `native.advertisedAddress` of that family | configuration | that address and the configured port may be authorized | the signed request stores the endpoint it was shown |
| listener | a specific unicast bind on a fixed port | configuration | that bound address may be authorized after Auth endpoint validation. An advertised address is accepted only for a wildcard of the same family, and only when it is unicast, not loopback, not multicast, not in the IPv4 this-network block, not the IPv4 limited broadcast, and not IPv6 link-local | config parse refusal is the evidence that the setting does not apply |
| listener | no listener, a configured port of zero, or a wildcard with no advertised address | configuration | nothing is disclosed, including after the socket is given an ephemeral port; a new original that names another endpoint conflicts | none |

`authentication_failed`, `credential_revoked`, `credential_expired`, and `authorization_lost` share one next action: stop, and do not retry that authorization. The reason stays the evidence, so a failed proof is not recorded as an owner revoke. A `server_shutdown` frame keeps the authorization and ends this session. This gateway's own process stop is the retryable `temporary_unavailable` close above, not that frame. `protocol_incompatible` keeps the authorization and is not called a shutdown: the process is still up, and a matching client can connect. Plain EOF is retryable because the gateway named no cause. Termination does not delete cached conversations.

The rejected shapes are a route lookup toward a documentation address (it fails with no default route and follows a full-tunnel VPN, and it reads the operating system from inside pairing), one "not retryable" bucket that would drop a phone on a `server_shutdown` frame or a protocol mismatch, and a second client list of the same reasons that also stores `retryable`.

The retained refresh and follow reader uses that same projection for both a native `session.authenticate` error and a close. A `session.terminated` on its connection is the close, and only a terminal revoke asks status again. A temporary authentication failure is not evidence that authority was refused; a protocol mismatch is not an unreachable peer. Both keep authorization and cache data. The retained failure mapping consumes the projected class rather than maintaining a second list of reasons.

The connection/conversation boundary must run before coarsening the error to a `ReadFailure`. A coarse `Protocol` alone cannot distinguish a terminal protocol close from a conversation that could not be read. The record reader therefore preserves published connection outcomes as failures of the whole refresh, and applies its existing incomplete-conversation handling only afterward. This removes the classification loss rather than adding a second reason list or a new recovery state. `source_preparing` still leaves the conversation unread on its usable connection.

`connection_outcome` is the only close-class projection. `connection_outcome_is_the_only_close_class` names every `SessionCloseReason` and a missing reason. The command and endpoint rows stay with `exact_replay_returns_the_stored_original_when_entropy_is_unavailable` (Active, no draw), `owner_phone_delete_retry_returns_the_idempotent_receipt`, `endpoint_publishes_the_configured_address_for_a_wildcard_bind`, `advertised_address_is_a_routable_host_for_a_wildcard_listen`, `owner_phone_remember_is_refused_and_a_once_answer_stays_admitted`, `owner_phone_remember_answer_stores_no_rule`, `session_terminated_is_the_close_reason_and_is_not_queued`, `terminated_session_is_the_close_reason_and_a_malformed_one_is_protocol`, and `plain_eof_stays_an_untyped_close_and_a_bad_termination_is_protocol`.

## Native product settlement ordering

The physical owner settles all unfinished tickets as one batch. Ticket locks
exclude observers while its single completion wake runs; the owner then reads
the effective failure and publishes the batch without another injected callback.
This order lets a completion-wake panic replace cancellation before immutable
settlements become observable. Previously completed factual responses remain
facts, and a decoded failure keeps precedence over a later callback panic.
A broken injected wake port cannot become a second settlement owner: its waiter
asks for stop and waits for publication through a private per-ticket condition
variable. That condition variable carries synchronization only; the ticket's
immutable completion and the physical owner's dispatch record remain authority.

| Ordering | Settlement and close | Regression |
| --- | --- | --- |
| Explicit stop, successful stop wake, completion wake panics | Active dispatch is Uncertain(WorkerPanicked); queued work is NotDispatched(WorkerPanicked); close reports WorkerPanicked | `completion_wake_failure_settles_the_whole_batch_with_the_effective_cause` |
| Decoded failure, later stop, completion wake panics | Pending tickets and close retain the decoded failure | `completion_wake_failure_preserves_a_decoded_failure_and_factual_response` |
| Factual response before stop and completion-wake panic | Original response remains unchanged; unfinished batch uses WorkerPanicked | `completion_wake_failure_preserves_a_decoded_failure_and_factual_response` |
| A waiter's revision or wait callback panics before physical drain, including while the finisher has detached its batch | The waiter requests stop and waits on the ticket's private condition variable. Only the physical owner (or the handle after confirmed join/no start) publishes completion; dispatch remains Uncertain, queued work remains NotDispatched, and the waiter cannot declare drain | `wake_failure_waits_for_physical_settlement_without_inventing_dispatch_evidence` |

## Alternatives considered

- Bulk sharing existing conversations misses future/empty targets and changes the meaning of shared access.
- A persistent bootstrap credential proxy gives the phone the wrong authority and prevents retirement of setup-only secrets.
- Inferring purpose from coarse grants admits ancillary runtime/workspace administration.
- Copying purpose into a changed receiver schema creates a second authority and would require replacing or migrating existing approved receiver history. The canonical Auth consent, issued device proof and remembered receiver already retain the exact linkage.

## Consequences

The gateway must publish authenticated actual native discovery and support exact original-request recovery. Native preparation must be durable before mutation, and protected product must have a separate bounded physical child whose close drains before parent disposal. Active delegated access depends on current device/member/receiver authority, while the original bootstrap remains historical linkage after retirement. Current receiver epochs may advance beyond the original Pair receipt; historical equality cannot stand in for current progress. Completion requires a genuine signed enrollment, no-share creation, positive native empty projection, first Send/reply and current revocation/close proofs. No grants-only subset is a completed implementation.


## Current integration and verification boundaries

`phone.endpoint` returns the authenticated source and a numeric native
endpoint and public pin. Which address may be disclosed is the listener rows
of the lifecycle table. `phone.authorize` and `phone.status` consume the complete
original signed request through the canonical Auth ledger; original TLS recovery
uses the saved UUID/digest and actual device proof without restoring a PAKE
attempt. Mutating resume owns expiry and late receiver Pair/fence settlement;
read-only recovery never expires or issues.

The receiver policy fingerprint includes the Auth profile, purpose bundle and
all shared conversation-access schema/policy sources through one length-delimited
hash owner. The actual receiver journal advances its policy epoch before saved
progress is reused. Ordinary paired catalogue filtering retains its continuation;
a first-party candidate contradiction refuses the complete pass without a new
cursor or a disclosed denied ID.

Current development contracts are used directly. There are no schema aliases or
old-format fallback paths for this feature; any stale development data migration
must preserve existing chats and drafts and cannot manufacture authorization.
The server source/fixture proofs do not establish Apple package adoption or a
connected phone. Actual protected-product composition, positive fresh-empty
native projection and first Send/reply remain required before completion.
