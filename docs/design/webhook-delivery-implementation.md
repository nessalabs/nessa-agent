# Outbound webhook implementation (#896)

This work implements the Webhooks contract in [access grants](access-grants.md).
The issue's original short description does not replace that document's W-*
ordering table. This page tracks implementation slices and test coverage. It
does not introduce a second authorization model.

Current working slice: delivery values, message encoding and bounded HTTPS.
Runtime registration, source replay, durable outbox and credential/grant
coordination are not wired yet. No gateway sends a webhook from this slice.
Application registrations (#1052) and inbound workflow triggers (#1088) remain
separate. The existing principal-keyed grant row is the authority to reuse.

## Dependent integration work

`codex/896-webhook-delivery` starts at frozen foundation head
`2613b8f68a6f12d08a74903a14fddce2b3cdba1d`. It is work in progress, not
an implemented usable delivery flow. Issue #1052 remains open at this
checkpoint; no implementation PR was found. This branch does not add an
application registration or change unbound product-session owner fallback.

The credential registry owns effect holds and mutation publication. The grant
store owns principal/resource revision holds using its existing change gates.
The webhook application composes those facts with the existing checked metadata
loader and conversation Cedar evaluator. A new `WebhookReader` Cedar subject
has only a read permit; it is never represented as a receiver or person.

| Before | Guard/event | Effect and recovery |
| --- | --- | --- |
| credential live | effect hold acquired | Read a coherent snapshot while mutation publication is fenced; this is not permission. The caller must bound the hold and evaluate current lifetime, linkage and Cedar. |
| credential held | asynchronous revoke | Wait for the hold, then persist revocation and its existing audit before publication. A subsequent hold reads the revoked credential. |
| credential held | synchronous registry mutation | Return the existing Locked refusal without changing state; synchronous callers may retry after the bounded effect. No runtime thread blocks waiting for an HTTP effect. |
| grant present at epoch E | principal/resource hold acquired | Reload that principal's grant after taking the same gate that changes use. Missing or unequal revision refuses; another conversation has another gate. |
| grant held at E | unshare or replacement | Wait for the hold before changing the row. A queued attempt taking the gate afterwards cannot reuse E. |
| credential and grant held | Cedar webhook.read permits | Both holds remain owned by delivery admission; record source selection and dispatch must consume them within their deadline. |
| credential or grant held | policy/lifetime/linkage/target mismatch or unavailable | Release both holds and make no POST. Unavailable does not become a final policy refusal. |

Shared files are `nessa-auth` ports/local registry, conversation share-grant
ports/store and access-policy/Cedar bundle. Registration/configuration,
durable outbox/source replay, runtime composition and end-to-end delivery remain
required before this dependent branch can satisfy #896.

## Delivery ordering

The immutable domain value returns a replacement. Only an acknowledged store
write makes that replacement durable. The network adapter has no authority to
select a conversation or grant access.

| Before | Event and guard | Replacement / effect | Evidence |
| --- | --- | --- | --- |
| queued or retry | reserve transport and DNS capacity, validate and pin destination before delivery admission | a single-use prepared POST handle; no key, body or POST yet; total deadline starts | `https_tests::local_backpressure_precedes_sending_and_does_not_consume_an_attempt` |
| queued or retry | transport or retained DNS capacity unavailable | no delivery transition and no consumed attempt | `https_tests::retained_dns_capacity_refuses_preparation_while_delivery_stays_queued` |
| prepared handle | total deadline already passed | timed out before constructing or polling a request; release capacity | `https_tests::expired_preparation_refuses_before_polling_a_request` |
| queued | current epoch equals admitted epoch | sending; retain first timestamp; increment attempts; persist before POST | `delivery_tests::admission_pins_epoch_and_timestamp` |
| queued or retry | epoch changed or grant removed | not sent; no POST | `delivery_tests::refusal_cannot_cancel_an_inflight_observation` |
| retry | before stored due time | refuse admission; no POST | `delivery_tests::retries_are_bounded_and_keep_identity` |
| retry | due, same epoch, attempt budget remains | sending, same ID and timestamp | `delivery_tests::retries_are_bounded_and_keep_identity` |
| sending | complete 2xx | sent | `delivery_tests::observations_are_terminal_except_bounded_retry` |
| sending | complete 408, 429 or 5xx; proven connection failure | retry at 1, 2, 4, 8 seconds; fifth failure is final | `delivery_tests::retries_are_bounded_and_keep_identity` |
| sending | complete 3xx or other final response | not sent; no redirect | `https_tests::redirect_is_a_response_and_receives_no_second_request` |
| sending | deadline before complete response | timed out; close local request | `https_tests::a_hung_response_is_bounded` |
| sending | response incomplete or too large, including unproven request errors | unknown; no retry | `https_tests::responses_are_bounded_without_retaining_remote_text`, `https_tests::lost_or_malformed_headers_do_not_claim_zero_response_bytes` |
| sending | restart with no durable observation | unknown; no POST | `delivery_tests::restart_does_not_repost_an_unknown_effect` |
| terminal | any admission | refuse; no POST | `delivery_tests::observations_are_terminal_except_bounded_retry` |

The caller prepares before admitting or persisting `sending`, then holds the
grant revision and registration permits through the attempt and its outcome.
`WebhookPost` consumes the prepared handle; its error type has no Busy variant.
These domain and transport APIs alone do not establish the authority holds,
crash-safe audit, source filtering or revoke ordering. Those remain integration
requirements before #896 can close.

## Wire and network contract

The MAC uses the existing `hmac`/`sha2` primitive used by backup manifests.
Input is three u64 big-endian length-prefixed fields: canonical decimal Unix
milliseconds, the UUID delivery ID, and the exact stored body. The header is
`Nessa-Webhook-Signature: v1=<64 lowercase hex digits>`. Retries reuse the
timestamp and body, preserving the signature. There is no fresh timestamp on a
retry. Recipients choose a replay-age policy compatible with the retry window
and deduplicate effects by `Nessa-Webhook-Id`; neither HTTP success nor retries
provide exactly-once remote effects. Reqwest cannot prove zero response bytes
from an arbitrary request error: a malformed or partial response produces the
same error predicates. This adapter conservatively records those errors unknown
without retry. Proven connection failures and complete 408, 429 or 5xx responses
remain retryable. This is narrower than the design's optional no-response retry.

The outer JSON fields are `conversationId`, `incarnation`, `position`, `record`
in that order. Raw record bytes are preserved, without sorting keys or changing
escaping. The source must supply the authorized committed `recordsPage` record.

The current owner representation is `RecordWireRecord`: a decimal-string
position, record ID and base64 payload (`product/record_read.rs`). The adapter
preserves that record JSON; it does not decode or reconstruct the payload. The
`{text:...}` signing golden is a synthetic framing fixture, not a declaration of
the record schema. A separate regression constructs the actual owner DTO.

Isolated HTTPS tests generate an in-memory test CA and an eight-day server leaf
with server-auth EKU and a recipient.test SAN. They retain the production
platform verifier; untrusted-root and wrong-hostname controls must fail with a
certificate error before any HTTP bytes. This replaces a fixed 100-year leaf
that macOS rejected, without changing production trust or destination policy.

Destinations are HTTPS DNS names on port 443, bounded to 2048 bytes, with no
userinfo, query, fragment or control bytes. This deliberately permits a subset
of HTTPS URLs. Endpoint paths can contain secrets and diagnostic output hides
the whole destination, message and MAC key.

Each attempt resolves DNS afresh, accepts at most 16 addresses, rejects the
entire answer if any address is outside the adapter's public-unicast subset,
and pins that answer into a fresh HTTP client. TLS still validates the original
DNS name. Proxy environment variables and redirects are disabled. IPv4 private,
loopback, shared, link-local, documentation, benchmark, multicast and reserved
space are refused. IPv6 accepts 2000::/3 excluding 2001::/23, documentation
prefixes and 6to4; mapped addresses take the IPv4 checks. DNS names alone are
not trusted, including a public name resolving to an internal address.

Four preparations or HTTP attempts at most are admitted by one adapter. Capacity
exhaustion returns Busy from preparation, before `sending`. One 15-second budget
starts at preparation and includes DNS, the caller's admission/save interval,
connect, TLS, request and the complete response. The post retains that same
deadline and refuses an expired handle before polling a request. A response body
is discarded chunk by chunk, with a 16-KiB bound checked against both Content-Length and actual bytes. The
HTTP/1 parser separately bounds its header buffer through Hyper's finite default;
16 KiB is the body cap, not a custom header cap. No
response text is stored or logged. Busy is local backpressure, not a failed
remote delivery. An unproven request error may follow a remote effect and is
unknown, with no retry. The libc DNS worker keeps its bounded permit after a
waiter's timeout, until the physical lookup returns. Call cancellation closes
the HTTP future; that is not remote rollback. The blocking DNS worker retains
capacity but has no promised physical duration or bounded runtime shutdown.

## Remaining integration gates

Registration is owner-issued, one active registration per integration principal,
and uses the existing credential lifecycle rather than a product socket or a
receiver binding. Share accepts that registered principal through Cedar. A read
permit must combine current credential identity, membership, capability facts,
grant epoch and registration state. Principal grant holds must share the store's
existing principal/resource gates with unshare. Credential revoke must take the
registration permit before it publishes revocation. A stale epoch must be
refused before the HTTP adapter is called.

The outbox must commit contiguous source checkpoint plus queued identity in one
transaction, bound pending and audited-terminal retention to 1024 each, persist
sending before any POST, and preserve audit causality across failures. Restart
replays queued/due work and marks unresolved sending unknown. A live process
reconciles held outcomes without reposting. Incarnation reset remains explicit
and atomic with each affected row's audit. These are the W-* contract's gates,
not behavior delivered by this first slice.


## Dependent outbox ordering design (implementation in progress)

The private outbox owns a separate SQLite schema, a lifetime filesystem lock,
and four retained blocking-operation permits. This adds persistence, not an
additional authorization decision. The registration caller and dispatcher still
have to compose existing credential/grant/Cedar admission.

| Saved state | Event / guard | Commit and effect | Required regression |
| --- | --- | --- | --- |
| no cursor | authorized contiguous first checkpoint | queue ID, exact body, physical source scope, grant epoch, cursor and audit commit together | `checkpoint_and_identity_survive_reopen` |
| existing checkpoint | identical body, scope and epoch | return original identity without another audit or cursor advance | `conflicting_checkpoint_does_not_replace_bytes` |
| existing checkpoint | conflicting bytes or scope | refuse conflict; retain original | `conflicting_checkpoint_does_not_replace_bytes` |
| live cursor | predecessor differs | refuse conflict; no skipped checkpoint | `cursor_cas_and_queue_limit_preserve_predecessor` |
| live cursor | source origin/incarnation/schema changes | commit reset-required plus refusal and causal audit for queued/retry rows; sending untouched | `source_change_requires_explicit_reset` |
| queued / retry | registration stop with canonical initiator | stop registration and refuse pending rows in one transaction; sending resolves own observation | `stop_preserves_inflight_observation_and_cause` |
| queued / due retry | valid current authority, sending CAS saved and audit projected | caller may dispatch exact bytes; persistence refusal means no POST | `checkpoint_and_identity_survive_reopen`, dispatcher E2E pending |
| sending | known response observed | CAS observation and audit; loss of local acknowledgement never authorizes a second POST | `transition_cas_rejects_lost_ack_repost` |
| sending | restart | commit terminal unknown and restart audit; no automatic POST | `restart_recovers_sending_without_repost` |
| SQL state committed | audit projection fails | return unavailable; replay immutable audit projection before more work | `audit_failure_recovers_before_work` |
| any pending work | queue or blocking-worker limit reached | typed capacity refusal without cursor/identity advance | `cursor_cas_and_queue_limit_preserve_predecessor`, physical permit test pending |
| terminal | published audit; retention exceeded | delete old SQL projections, retain immutable audit files | `terminal_retention_is_bounded` |

No outbox-only test is registration, capability/revoke E2E or browser evidence.
The full integration acceptance remains open.

### Outbox review corrections before sender composition

Round-one adversarial review found four structural defects and one restoration
bound. The corrected design uses one StoredDelivery decoder for enclosing SQL
identity, scope, epoch and state agreement. Indexed phase and due time are
SQLite generated projections of the saved state, so the index cannot classify
a different phase. Startup validates all restored rows before publication or
cleanup; every mutation validates its enclosing row again. A canonical owner
lock excludes a second store adapter while this process owns the database.

| Ordering | Guard / same-write correction | Required regression |
| --- | --- | --- |
| restored row hidden by contradictory phase projection | phase/due are generated from state; direct projection writes fail; startup validates saved state before any filtered scan or trim | `restored_rows_are_checked_before_projection_cleanup` |
| bulk refusal sees another delivery's valid state | shared decoder correlates enclosing ID and complete checkpoint before any update; affected-row count is one; failure rolls back the whole bulk commit | `bulk_refusal_rejects_foreign_state_without_partial_effects` |
| cursor names a different receiver or stream | refuse Invalid before replacing cursor or touching deliveries | `cursor_scope_identity_is_not_overwritten` |
| stopped Sending produces retryable response | preserve the response transition and audit; common stopped cleanup refuses Retry with the original stored stop cause; inactive work is not returned ready | `late_retry_after_stop_does_not_become_ready` |
| incarnation replacement creates terminal rows beyond retention | trim older already-published terminal projections in the same state/audit transaction | `bulk_reset_retention_is_bounded_in_the_commit` |
| one reader replaces a physical source | explicit Live/Restart reader attribution is copied to cursor and every actually changed delivery audit | `bulk_reset_keeps_one_reader_initiator` |
| restored retry has an impossible due time | due must be at least its backoff delay from Unix zero; do not compare it with the original timestamp, because the wall clock can move backward | `restored_retry_respects_backoff_without_assuming_wall_clock_order` |

Retirement and stopped-response cleanup read an injected auth Clock for the new
transition time, while retaining the original stop timestamp and initiator as
causal facts. They do not relabel the old stop time as a new observation.

### Consistency-boundary redesign after consecutive review rounds

Round two found the retention omission on credential stop as well as source
reset. Rather than add another call-site repair, the outbox journal will own
**one transaction commit path**. Every state transaction (queue, reset-required,
stop, observation, recovery, registration and cleanup) uses it. Before commit it
removes only terminal projections whose audit is already published, enforces
the active retention ceiling, and removes already-published terminal projections
of stopped registrations. Publication failure cannot postpone those deletes.
The cleanup path publishes owed immutable records, retires drained registration
material and commits through that same owner. No new delivery state or policy
rule is introduced.

The restore boundary also covers audit keys and resource accounting. UUID text
must be the canonical spelling the writer emits; encoded delivery state must
match the current encoder shape, so a semantic alias cannot strand a byte-CAS.
An audit row's key, recordId, registration and optional delivery must agree.
Publishing acknowledges exactly one saved row or refuses; it cannot loop on a
zero-row acknowledgement. Restored registrations and nonterminal/terminal rows
must fit their existing limits before scanning or collecting them. Bulk reads
also take at most the owning limit plus one sentinel row before allocation,
then refuse an excess. These are file/consistency facts, not Cedar decisions.

| Ordering | Shared owner / regression |
| --- | --- |
| full retained history + full pending queue + stop, publication unavailable | same commit owner as source reset; inspect committed SQL before recovery in `stop_retention_is_bounded_when_publication_fails` |
| noncanonical restored audit key | canonical restore refusal before any publication in `noncanonical_audit_key_is_refused_before_publication` |
| canonical SQL key with aliased saved-state UUID/encoding | current encoder shape is required in `aliased_saved_state_is_refused_on_restore` |
| individually valid rows exceed registration or pending bounds | shared accounting rejects restored excess before bulk allocation in `restored_capacity_is_bounded_before_collection` |

This redesign addresses the store review. It does not provide registration
issuance coordination, the source/sender loop, an expiry-observation journal,
owner reset admission or capability/revoke E2E. Those remain integration work.

An already-Sending row now also needs the source-change context that existed
when its cursor was reset-required. The source-change transaction captures that
immutable reader/time context without cancelling Sending or claiming a phase
transition. A later retry is refused by common cleanup using that same context,
even if an owner reset has since replaced the cursor. A known success remains
success; restart without an observation remains Unknown. When a credential stop
is already committed at settlement, its canonical stop cause owns refusal.
No new delivery phase or grant/authorization epoch is introduced.
