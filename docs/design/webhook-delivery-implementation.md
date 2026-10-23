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
| any pending work | queue or blocking-worker limit reached | typed capacity refusal without cursor/identity advance | `cursor_cas_and_queue_limit_preserve_predecessor`, `cancelled_observer_keeps_physical_worker_and_canonical_owner` |
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

### Registration restore integrity

The first review after the consistency redesign found registrations were not
included in restoration validation. The same persisted registration decoder now
owns canonical identity, destination, key and active/stopped lifecycle metadata.
Startup invokes it after resource accounting and before any audit publication or
cleanup. Live delivery audit and retirement consume that decoder's checked facts,
rather than reading unchecked identity strings. A malformed registration refuses
before it can produce evidence with an invalid principal or initiator.

| Ordering | Guard and regression |
| --- | --- |
| Reopen malformed registration identities, URL or stop attribution | Refuse Invalid before publication or cleanup; `restored_registration_facts_are_checked_before_effects` |
| Live audit consumes malformed registration facts | Checked decoder refuses and transaction rolls back; `delivery_audit_uses_checked_registration_facts` |

### Parent-authorized persisted-state redesign

The parent accepted the blocked review's focused redesign on 2026-10-23 and
explicitly directed implementation to continue. Prior findings and their exact
probes remain in the evidence directory; they are not erased by this decision.

The persisted audit becomes one closed typed representation, used by every
writer, restoration and publication. Delivery records retain complete before
and after snapshots so the domain's existing transition methods, not a second
phase table, validate history. Human/system attribution and credential-stop
facts travel in the record; restoration never substitutes current ownership or
requires a historical registration to still exist. Unknown fields, invalid
identities, impossible transitions and mismatched projections refuse before
publication. No permission decision moves into this representation.

A single persisted cursor decoder owns registration/conversation/scope,
nonnegative SQLite integer position and the reset flag. Startup checks the same
whole value that queue and current-delivery reads consume, including when only
terminal deliveries remain. No schema migration, compatibility alias or new
permission system is added.

| Saved input / ordering | Guard and effect | Required regression |
| --- | --- | --- |
| owed audit with invalid actor, target, time, phase or unknown fields | Decode the closed type and validate complete historical transition before any file publication or cleanup | malformed audit restore table; original major reproduction becomes refusal regression |
| historical registration already retired | Validate retained original identities/actors from its saved record; do not join current registration facts or invent an owner | valid retired historical actor recovery |
| individually legal before and after delivery snapshots that cannot transition | Ask existing DeliveryCause/domain transition owner; refuse Invalid | impossible history and snapshot/projection correlation |
| audit file publication fails after typed record commits | Return visible failure; later operation publishes exact original typed bytes and initiator | typed audit failure/recovery with actor preservation |
| cursor has text position with only terminal history | Decode all cursor fields before publication or duplicate acknowledgement; refuse Invalid | original minor reproduction becomes refusal regression |
| cursor reset flag or identity/scope is malformed | Same decoder refuses at restore and live read | cursor boundary table with valid maximum-integer controls |
| retained delivery audit chain | Each validated snapshot follows the previous saved after-state, keeps checkpoint/target and source-stop context, and ends at the enclosing current delivery; reject missing, foreign or reordered history before effects | historical chain, current-state and checkpoint mutation regressions |

The first fresh review of this parent-directed redesign confirmed two remaining
cross-record correlations. Its report and exact bug demonstrations are preserved
under `docs/reviews/evidence/webhook-outbox/typed-state/review-1/`.

| Ordering / historical fact | Correlation owner and expected result | Regression |
| --- | --- | --- |
| Sending source-change capture precedes any observation audit | Current Sending alone may have captured context absent from its last admission audit; reopen and Unknown recovery preserve it | valid Sending capture/reopen/recovery control |
| observation has settled Sending | Latest audit source-stop context equals the enclosing delivery's complete context, including None/Some, actor and time | terminal source context null/actor/time mutations refuse before publication |
| credential stop commits but audit publication fails | Each authority-stop delivery record retains typed original stop cause, actor and time; its top-level cause/actor agree, and restore correlates them with the immutable owning registration stop | original actor mutation, coordinated actor/context and time mutation, active-registration false stop refusal |
| registration retirement has deleted current facts | Historical retirement evidence validates its own saved stop facts and never substitutes a current owner or actor | existing lost-ack retired historical actor regression |


### Second fresh typed-state review: owning relationships remain incomplete

Review head dc86bc2d, source26b2bd35, found two major defects. The complete report
and exact demonstrations are in `docs/reviews/evidence/webhook-outbox/typed-state/review-2/`.
Registration audit target/original stop actor can disagree with retained
registration facts; cursor audit prior position can disagree with its retained
reset checkpoint. Both survive reopen and publish. Typed record validation is
complete locally, but aggregate correlation is implemented for Delivery only.
No authorization bypass was demonstrated. The parent owns the next structural
decision before another patch cycle; runtime remains uncomposed and unchanged.

| Demonstrated contradiction | Required future relationship / positive control |
| --- | --- |
| owed registration stop names another valid revoker while original stop remains | correlation with original retained stop actor/cause/time; valid historical retirement after registration deletion remains accepted |
| owed registration issue substitutes valid principal/owner and initiator | target correlation with retained registration; do not reconstruct historical target from unrelated current ownership |
| owed source-change audit changes priorPosition 1 to 999 while reset cursor remains at 1 | correlate the transition with the checkpoint retained by its atomic write; define historical/retention behavior before implementing |

These are open findings and proposed requirements, not implemented fixes or a
new review-loop authorization. Existing passing source checks do not close them.


### Parent-adopted total contextual restoration

The parent explicitly adopted this structural correction after fresh round two.
Restoration owns an exhaustive match across Registration, Cursor and Delivery;
no default variant arm may silently omit contextual checks. The same contextual
admission checks all owed rows before the first publication effect, including
live replay. Existing full Delivery history validation remains owned here.

A stopped registration's private URL/key/cursors may still be removed before
its retirement audit acknowledgement. In that transaction retain a bounded,
secret-free retirement fact (original target and issuance time, original stop cause/actor/time,
retirement observation time and exact retirement audit identity). The retained
fact references that audit until both trim in the same transaction; missing or
substituted retirement evidence is invalid. Issue/Stop evidence may survive that same atomic
retirement and correlates against those retained facts. Drained requires these
facts and no live registration. Acknowledged facts and audit projections trim
together; owed facts cannot be discarded. No actor is inferred from current
ownership, and no private credential material is retained for audit recovery.

A cursor captures its original reset observation (observed scope, reader and
time) beside the prior checkpoint when reset-required is committed. Its whole
decoder requires both facts or neither. Contextual audit validation compares
the complete prior checkpoint and original observation. Published cursor audit
is removed before a later source/reset mutation; an owed cursor record must
still have its retained reset checkpoint. This introduces no reset API.

| Ordering / contradiction | Context owner and required evidence |
| --- | --- |
| owed Issue target substitutes valid principal/owner | retained registration or same-write retirement target; refuse before any publication |
| owed Stop actor/cause/time substitutes valid values | retained immutable stop, including after immediate retirement; refuse before publication |
| Drained owes publication after private registration deletion | retained secret-free retirement target/stop/time; valid replay preserves original actor and exact bytes |
| Drained facts missing, changed or paired with live registration | reject impossible ownership before publication/cleanup |
| owed Cursor prior position/scope or observed scope/reader/time differs | complete cursor reset fact and prior checkpoint; refuse before publication |
| Cursor reset flag and captured observation disagree | whole cursor decoder rejects at restoration and live reads |
| Delivery target/checkpoint/credential stop differs | exhaustive variant branch checks enclosing facts; history owner checks chain/current/source contexts |
| second owed record contradicts facts | validate the entire owed batch before the first immutable-file effect |
| live owed Delivery record is locally legal but its history/current end contradicts the enclosing delivery | reuse the complete history validator for the deliveries selected by unpublished audit identity, before any publication; no scan of unrelated retained histories |
| publication acknowledgement is lost | retain original contextual facts until exact ack and common trim; no replacement identities/history |

This is the parent-authorized bounded structural correction, not erasure of
prior findings or a relaxation of browser/CI gates. Composition follows only a
clean fresh persistence review.

### Parent-adopted immutable source-reset owner

After the complete review at5bb457db was preserved in0f6bea1e, the parent adopted
one retained source-reset identity. The three source-provenance demonstrations
are the same structural gap; the selector scan is a separate query-shape fix.
No other independent structural constraint was reported.

The source-reset owner retains immutable registration/chat, prior checkpoint,
observed source and original reader/time. An immutable first-capture membership
links each actually affected delivery to that identity and its original saved
state/position. This prevents a later physically repeated source event from
claiming an old Sending delivery. Current cursor and audit references retain the
same identity; actor/time snapshots in audit are checked projections, never a
second source of provenance. Logical access epochs may differ between the old
delivery and newer cursor; physical source and immutable captured identity must
still agree. Cleanup time is separate from the original reset observation.

Reference accounting is an indexed SQLite projection owned by the reset schema.
Cursor, capture membership and audit insertion/removal retain/release the fact.
The last reference releases it in the same state transaction; a published cursor
audit alone does not release a fact still owned by an old Sending delivery.
Membership survives until the delivery is removed with its acknowledged history.
Immutable schema guards forbid replacing facts or reassigning captured members;
restoration checks the owning schema definitions and indexed reference counts.

New cursor capacity is1024per registration. A reset fact is at most16KiB, and
there are at most cursor1024+pending1024+retained1024=3072facts per registration,
including detached delivery/history owners. This yields48MiB of reset fact bytes
per registration; captured saved states are at most512bytes and one per retained
delivery. Capacity is refused before partial reset/capture effects. Startup
checks count/byte/reference bounds before bulk reads; live preflight checks
only facts/history named by owed audit identities. Immutable audit-file retention
remains its separately documented unbounded history.

| Ordering / contradiction | Single owner and required evidence |
| --- | --- |
| immediate Queued/Retry source refusal | original capture state matches the before-state; original actor and reset time are projected into the audit |
| Sending capture before any response audit | immutable membership names the actual original Sending state/reset identity; no copied unproven source-stop value |
| audit claims the reset before the original capture state existed | history must reach the member’s captured state before any audit may project that reset identity; no inference from wall time |
| later response creates Retry then cleanup refuses | original reset actor/time remain causal; cleanup records its own observation time |
| later cursor replacement/deletion or repeated physical source | old delivery retains first membership; compatibility permits differing logical access epochs but refuses foreign scope/checkpoint/member identity |
| malformed/dangling reset reference or changed audit actor/time/scope | reject before publication/recovery effects; no manufactured reset history |
| cursor, delivery history or registration retires | release only those references; final reference removes reset fact transactionally; valid retired-registration audit still replays |
| count/byte capacity or failed capture write | typed Capacity/Invalid/Conflict or Unavailable, transaction rolls back all reset/member/cursor/evidence changes |
| fixed owed count and growing published history | direct published=0 query uses existing index; deterministic fullscan/VM counts stay independent of published history |

## Runtime registration and delivery composition

The isolated runtime branch starts from main `56602fc6` and clean persistence
review head `78258a8f`. The following orderings are designed before composition;
this section describes work in progress, not executed integration evidence.
Application installation and general application-principal sessions (#1052) are
separate. Existing credential issuance already creates a proposed integration
principal/member. This slice registers its outbound destination, without a
receiver binding or a second permission system.

Cedar's existing credential grant-policy port receives a `WebhookIssue` path.
Its existing member-issuance rule still owns the role decision. A webhook-path
forbid restricts the principal to integration and grants to conversation.read.
The registry derives that path from the server-selected `CredentialIssuePurpose`,
not a client-provided role verdict. The existing issuance fingerprint binds the
webhook purpose, so an earlier ordinary issuance receipt cannot bypass this path
and an ordinary retry cannot reuse a webhook receipt. This is implemented at the
auth boundary; product configuration and registration composition remain pending.
The generic credential registry remains the owner of grant resources, audience,
identity linkage, issuance receipts and credential mutation. The registration
coordinator asks these owners; URL/key possession grants no authority.

| Ordering | Guard and effect | Required isolated evidence |
| --- | --- | --- |
| webhook issue proposes human, agent, peer, administration, write or unrelated grant | existing Cedar owner refuses the webhook path before credential or registration writes | policy matrix and real issue refusal |
| same issuer/request is retried with a different issuing purpose | existing auth receipt refuses the conflicting fingerprint before writes, including after restart; a matching retry preserves original identity/transitions and cannot replay the bearer secret | bidirectional purpose substitution, cold replay and byte-identical registry |
| ordinary or webhook issue while a registration's credential remains current | one principal-scoped coordinator gate covers both issue modes; refuse another active credential/registration | competing issue and generic issue tests |
| expired or revoked credential observed during issue, without due work | retain the original observed stop cause; stop old registration before admitting replacement | expiry/revoke without a queued record |
| lifecycle coordinator observes expiry before any delivery exists | typed Lifecycle observer names the actual automatic owner in the enclosing registration stop fact; no delivery ID or human initiator is fabricated; the first committed observer/time survives later observations and retirement | expiry with no queued row, lost acknowledgement, cold restoration and changed observer rejection |
| auth issue commits, registration acknowledgement is lost | retained bounded intent fixes issuer/request, configuration fingerprint and selected IDs; validate actual auth receipt before registration and expose no recovered bearer secret | crash/restart and unrelated prior-issue receipt |
| intent is reserved but auth has not admitted the request | retain proposed inputs, not a permission verdict; the real auth registry still validates and decides issuance | invalid proposal creates no credential or destination |
| auth returns Conflict or Capacity | discard only the uninstalled intent and preserve the exact typed auth failure; installed registrations cannot be discarded | typed refusal and protected installed intent |
| auth returns Unavailable or caller loses the response | retain the original private intent for an authenticated retry; do not infer rollback or recover a bearer secret | uncertain auth commit and retry |
| caller retries with changed endpoint, key or caller input | refuse before auth; server-selected IDs and observation time resolve to the retained originals | each conflicting input and unchanged original intent |
| auth replays an expired or revoked credential | metadata describes current lifecycle, while issuance transitions retain original cause; hold current credential and refuse installation if stopped | revoke/expiry between auth commit and install |
| auth adapter returns contradictory metadata or transitions | no registration installation or successful response; retain original intent as recovery evidence | wrong identity, original issuer/correlation and transition cases |
| current credential read is unavailable or has a different binding | unavailable remains retryable, stopped lifetime remains Stopped, and contradictory credential/membership stays Inconsistent; preserve the intent and install nothing | read failure, full membership identity substitution and successful retry |
| installed intent contradicts its retained registration | whole owed-publication preflight correlates identity, target, configuration and original observation before any recovery effect | live and cold mutation rejection with valid retry controls |
| installed intent identity is substituted or deleted | registration identity is a mandatory foreign key to the immutable original intent, rather than inferring installation from a lookup that could become a new proposal | dangling reference and identity mutation rejection |
| registration/intent guards are missing or weakened on disk | the outbox schema owner compares all owned non-internal SQLite definitions against its single current schema before restoration; borrowed-row byte checks precede owned schema/configuration allocations | immutable, retirement and reference-schema substitutions |
| retained admission timestamp contradicts auth issuance time | server selects one millisecond observation; auth issuance seconds are its integer-second projection and installation retains the original observation | timestamp substitution and matching retry |
| private intent reaches count or byte capacity | at most64intents,16KiB issuance JSON, bounded whole-row reads before allocation; typed refusal before insertion | count/byte bounds and malformed restored row |
| registration retires after draining | the registration-delete trigger removes its private intent in the same transaction; remaining secret-free retirement evidence retains original cause | retirement/failed acknowledgement and no resurrection by old receipt |
| retired request replays an auth receipt for another selected credential | query the auth owner's current metadata; discard the uninstalled proposal only when that selected ID was not issued, and return Conflict rather than install a new destination from the old receipt | retired replay does not leak intent capacity; contradictory replay preserves known issued evidence |
| credential revoke is replayed by a different caller | registration uses the canonical auth revocation actor/time, rather than the observer | revoke replay and failed stop recovery |
| granted source selected for one conversation | read actual principal grant and epoch through existing Cedar/fences, then canonical recordsPage encoding; checkpoint/body/identity commit together | one-chat source isolation and capability refusal |
| sender prepared endpoint, but Sending or its audit is unacknowledged | no POST; reconcile saved state on the next bounded read | uncertain Sending acknowledgement |
| Sending receipt acknowledged | recheck absolute expiry immediately before POST; keep current credential/grant holds only within the attempt deadline | stale epoch, expiry, revoke and cancellation barriers |
| known HTTP outcome cannot commit | bounded supervised owner retains the known observation while live; do not re-POST Sending; restart without saved outcome records Unknown | lost outcome acknowledgement and process restart |
| physical source changes or is later reset | retain the reviewed immutable reset/capture owner; owner-authorized reset uses its exact identity, never reconstructed actor/time | reset CAS, source cycles and retirement |
| registration, cursor, intent, queue or physical worker reaches its cap | explicit Capacity/backpressure; no partial admission, dropped record or unbounded notification queue | count/byte bounds and cleanup |

The proposed composition uses one bounded command/sender owner, with reserved
control admission and an explicit shutdown/drain contract. Serial ownership is
the initial W1 gate across ordinary issue, webhook issue and revocation; queued
commands have no authority. The owner survives a response observer disconnecting.
It retains at most one known HTTP outcome awaiting durable settlement, releases
credential/grant holds at the attempt deadline and does not POST an uncertain
Sending row. A process restart has no saved observation for that row and uses
the existing Unknown recovery. This ownership is still pending, rather than
inferred from the inner registration use case.

No claim of exactly-once remote effects is made. A recipient deduplicates by the
stable delivery identity, and retries retain the initial timestamp/signature.
Canonical Chromium/WebKit and feature stress/chaos remain required independently
of private Rust recipient tests. Previously denied browser downloads and manual
workflow operations are not retried by this composition.
