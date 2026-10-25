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
| reserve adapter substitutes a valid destination, key or caller request | one application-owned intent resolution binds returned selection to original caller facts immediately before auth; SQL replay consumes that same owner, allowing only original server-selected IDs and observation/time projection | substituted reservation fails before auth or installation; matching selected retry remains valid |
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

## Durable credential-stop observation for composition

This ordering is designed after the clean original-issuance correction at
`1c1355b537037e7416486856211b74c605804fa7`; it is not implemented by that
checkpoint. The domain owns a closed registration lifecycle and one immutable
stop fact, identified by the registration identity. SQL retains that fact once
with the registration; retirement carries it only while its existing owed
historical evidence needs it. No second source of actor/time is introduced.

The three states are Active (no stop fact), ExpiryObserved (the original expiry
reader is durable, but stopping is not acknowledged), and Stopped (the same fact
has taken effect). Only an expired observation can enter ExpiryObserved.
Revocation uses the original auth transition actor/time directly. Expiry readers
are explicitly the issuing principal, a delivery identity, or restart; a known
issuer is not replaced with a generic lifecycle label. Nothing wakes exactly at
expires_at. The observation's time is the original cause/read time, not an
assertion of when an external effect or database commit occurred.

| Before / event | Single owner, commit and recovery | Required evidence |
| --- | --- | --- |
| Active; owner issue reads expired credential | Existing auth lifetime fact; store ExpiryObserved with that actual principal and read time, then acknowledge its audit before stop | No queued row; actual issuer retained; no replacement issued before stop |
| Active; delivery reads expiry | Store that actual delivery identity before stopping, without inventing an owner | Different later owner/restart cannot replace original delivery |
| Active; startup is first expiry reader | Store Restart before stopping | Cold restore and repeated startup preserve original fact |
| ExpiryObserved; stop fails or response disappears | Retained owner retries the original fact; restart reads it before any replacement or delivery | Observation commit followed by failed stop, lost observation acknowledgement and recovery |
| ExpiryObserved; another reader or later revocation arrives | First stored registration-stop fact remains; canonical auth revocation still keeps its separate original journal event | Owner/delivery/restart substitution refuses; later canonical revocation does not erase first observation |
| Active; credential already revoked | Stop directly from canonical original auth revocation actor/time, not current observer | Different revoke caller/replay/restart preserves original cause |
| ExpiryObserved; stop commits | Transition to Stopped while retaining exact original observation; refuse queued/retry work through existing journal owner | Domain transition/audit agree; no stale work starts |
| Stopped; repeated stop or retirement | Preserve first fact, drain pending outcomes, then existing bounded retirement removes private URL/key/intent/cursors | Original fact remains in owed retirement recovery; no fabricated historical actor |
| Restored lifecycle, audit or observer contradicts retained original fact | Shared typed decoder and exhaustive contextual audit correlation reject before publication or effects | Every variant, actor/time/target mutation and valid retired recovery |
| Capacity / cleanup | At most one fact per already-bounded registration; existing bounded retirement owns temporary historical references and final cleanup | Repeated failed observations do not grow state; capacity never discards pending work |

The future application coordinator will consume these transitions. It must not reimplement
lifetime, Cedar or transition decisions. Runtime composition must retain the
admitted command through acknowledgement/cleanup when its response observer
leaves. Full source/sender and capability E2E gates remain separate.

## Parent-adjudicated original issuance and secret-free acknowledgement

At source `97d5c0991310cf7e10729e3f1cb3c2371be06460`, registration is unmounted.
The parent selected removal of bearer output rather than verification of an
unnecessary returned secret. Ordinary product issuance keeps its existing API.
The auth journal owns provenance; registration does not create another authority.

| Boundary | Owning fact and enforcement | Required evidence |
| --- | --- | --- |
| Original issuance | One validated first issuance transition per credential, including actual cause, initiator and command correlation; read with current metadata/member/revision under existing hold | Altered origin/correlation/identity, malformed restoration, valid original fact after revocation |
| Purpose | Actual auth WebhookIssue path records distinct typed WebhookIssue cause; existing Cedar consumes that immutable cause for session admission | Webhook bearer and resumed session refused; ordinary product and paired entry unchanged |
| Registration acknowledgement | Explicit secret-free RegisteredWebhook returned for initial and matching replay; auth bearer consumed/discarded internally | Same acknowledgement across lost response/restart, no bearer in registration output/storage/diagnostics |
| Ambiguous historical origin | AdminIssue is preserved as AdminIssue; no inference from grant shape or current registration; registration returns typed recovery-required outcome | Historical ambiguity refuses installation without rewriting facts |
| Matching webhook receipt with accepted historical actor or missing/nonreceipt correlation | Before command-transition replay, auth reads the actual original issuance under its current registry lock. An AdminIssue origin returns typed OriginRecoveryRequired through the command port, retaining receipt and private intent; registration maps it to its recovery result. Product receipt replay keeps its existing semantics. | LocalOperator/null command, absent command and different existing actor each return recovery on repeated registration, preserve auth bytes and one intent, and install nothing |
| Retirement | Registration removal does not remove or replace auth provenance | Retired webhook remains barred from product entry; revoked original history stays valid |

This correction does not recover or retry the prior flagged reviewer report.
Fresh review will concern the new correction and its tests only. Sender composition
and recipient E2E follow a clean correction review.

## Retained-state cleanup cost correction

At published runtime source `0e1f44def810c8b91439000f52986869cb0cdfc2`, Windows
job120586159654 timed out after45minutes. The last unfinished tests exercise
terminal and stopped-registration retention at their full1024-row boundaries.
Independent old-source Linux profiling shows growing SQL cleanup work (about
10.1M/40.2M/160.8M VM instructions by128/256/512 terminal deliveries); this
demonstrates repeated whole-history work, not a Windows deadlock diagnosis.

| Ordering | Cleanup owner and intended equivalent effect | Evidence required |
| --- | --- | --- |
| Terminal commit / maintenance | One indexed cutoff per registration replaces ranking every retained row. Delete only rows older than the newest1024 and with no owed audit. | Existing full-boundary tests unchanged; SQL VM budget and query plan |
| Terminal delivery row retires | Its exact published audit rows retire transactionally through the schema's ownership trigger; owed rows are not erased. Existing source-reset reference triggers remain authoritative. | Published/owed ownership and reference cleanup regressions |
| Registration audit acknowledgement | Delete published delivery-null records through their partial index, without visiting retained delivery histories. | Fixed cleanup steps as unrelated published delivery history grows |
| Stop, source reset, audit failure or restart | Preserve original identity, actor/cause/time and existing publication/recovery order; only query work changes. | Canonical outbox failures, reset/retirement and full1024-row assertions |

Production ceilings, retry/deadline behavior, fixtures' full boundary loops, test
assertions and CI timeouts stay intact. Platform qualification still requires an
allowed exact-head run; local profile numbers alone do not prove Windows fixed.

## Canonical revocation coordination still pending

The auth correction at d883b0ed6544c112f441ae1de73f443c7306280f now exposes
original issuance and optional actual revocation in the same held snapshot.
Explicit and Superseded retain Principal or offline LocalOperator. The current
outbox Revoked input still accepts a PrincipalId; runtime coordination must
consume the complete checked original fact without inventing a principal or
rewriting supersession. Its first expiry observation remains independent.
The held-fact contract and evidence are recorded below. Outbox/runtime mapping
and supervised command completion remain unfinished. Generic application
installation/no-binding admission stays in #1052.

## W24 M1 retained-context correction

Fresh review at4953b288 reproduced three phase/ack/work contradictions. The correction uses the existing retained-proof contract, rather than adding a second transition/ack history. It does not pretend to distinguish coherently rewritten Stopped+Sending after all registration projections have trimmed.

| Retained relationship | Contextual owner before restoration/publication | Required counterpart |
| --- | --- | --- |
| Owed expiry Observation with live registration | Require ExpiryObserved and exact original fact; Stopped cannot owe its prerequisite acknowledgement | Lost observation ack in ExpiryObserved recovers with all three reader kinds |
| Owed expiry Observation with retired fact | Refuse: original observation must acknowledge before stop and retirement | Genuine Stop/Drained owed retirement recovers |
| Current Queued with registration | Ask domain lifecycle's queued compatibility, also used by queue admission; Stopped cannot retain Queued because stop refuses it atomically | Active/ExpiryObserved queued remains valid |
| Current Sending or late Retry with Stopped | Preserve valid prior admission and late known outcome; do not replace first cause or reject all pending work | Late Sending→Retry with lost response ack restores and cleans up |
| Current work scan | Stream distinct Queued registration IDs through existing ready index, capped atMAX_REGISTRATIONS+1; no terminal/history scan or new acknowledgement flag | Query work stays independent of terminal history |

The same contextual owner runs before cold recovery and before the first owed publication. Canonical regressions must turn the three preserved acceptance demonstrations into explicit rejection expectations, with no new files, acknowledgements or retirement before refusal, and successful original-byte recovery. Existing domain/SQL failure controls remain retained.

## W24 M2 issuance acknowledgement monotonicity

Parent adjudicated the adjacent round-two finding as part of existing restoration,
replay and cleanup invariants. An initial issuance obligation cannot become owed
again after an operation that had to finish its acknowledgement. Validation uses
only retained proof; absent historical rows do not authorize invented history.

| Retained original Issue | Existing contextual owner's decision before effects | Evidence |
| --- | --- | --- |
| Owed, live Active, no retained cursor or delivery | Allow original initial publication/lost-ack recovery | Genuine failed initial publication, cold reopen and later queue acknowledgement |
| Owed, ExpiryObserved or Stopped | Invalid: lifecycle advancement required issuance acknowledgement | Exact review reproduction, all first-reader kinds, live/cold refusal and original repair |
| Owed, live Active with retained cursor or delivery | Invalid: that work required an earlier completed issuance obligation | Separate cursor-only and delivery-only retained proofs, no publication or SQL changes |
| Owed, retired | Invalid: retirement required earlier issuance acknowledgement | Genuine retained retirement, original Issue reinserted, refusal then original recovery |
| Published, any valid live/retired phase | Keep historical target/time correlation; do not treat published history as owed | Same adjacent histories accept original published evidence and recover |
| No retained later work and Active | Do not infer completion merely from absence of history or an already present audit file | Idempotent original acknowledgement remains valid |
| Work query | Existing registration-leading cursor primary key and delivery unique index answer EXISTS; no terminal-history scan | Query-plan and bounded SQL-step controls at full retained capacity |

No new lifecycle state, acknowledgement flag, schema, authority or history owner
is introduced. The existing contextual owner checks the owed Issue prerequisite;
published original Issue still follows the existing target/time relationship.

## Parent-selected held revocation fact

The parent chose the minimal typed optional fact in the existing AccessSnapshot,
without renaming CredentialOrigin or adding a journal. Actual revocation comes
from the same published registry read and lifetime hold as current metadata and
original issuance. It has no bearer and makes no permission decision.

| Facts at consumption | Owning relationship and refusal | Required evidence |
| --- | --- | --- |
| Active credential, no actual revocation | Explicit None is valid; expiry alone does not create a revocation or actor | Ordinary Product compatibility and first expiry observation |
| Revoked credential, actual original record | Existing domain transition validates Explicit or Superseded and Principal or LocalOperator; typed fact binds current lifecycle and actual sequence/revision/correlation | Explicit, both supersession kinds, offline action and cold restore |
| Revoked metadata, absent original record | Fail closed; do not derive actor/cause from caller or issuance | Missing-fact accessor and malformed retained state |
| Supplied fact names another credential, lifecycle or future revision | Fail before lifecycle coordination; actual snapshot ordering must follow original issuance | Identity/time/before-after/revision substitution and restore |
| First expiry precedes later canonical revocation | Preserve two different original facts; expiry is a reader observation and revocation remains its actual auth event | Actual journal read after expiry, no invented expiry initiator |
| Historical ordinary issuance and replay | Keep original one-time Product behavior and existing Cedar/session policy | Product issuance/replay and historical actor regressions |
| Journal identity fields | One private shared decoder enforces nonzero sequence/revision and bounded command correlation for issuance and revocation | Existing original-issuance controls and new fact mutations |

The outbox coordinator must consume the checked fact when a canonical revocation
is required. It must not substitute its current caller for an offline actor or
collapse supersession provenance into an explicit command. This auth checkpoint
publishes provenance only; product command/source/sender composition remains
separate and unfinished.

### Retained initial-publication proof (parent decision A)

Restoration and live publication share one ephemeral retained-progress summary;
there is no new durable acknowledgement field. An Active registration may owe its
initial projection only when exactly one retained initial identity is pending,
no retained initial identity is published, and no retained later-work fact exists.
A file already written with its SQL acknowledgement lost remains retryable under
that same identity. Erased proof cannot establish historical acknowledgement:
after all relevant proof is trimmed this design makes no post-trim guarantee.

| Retained owner | Summary contribution and ordering |
| --- | --- |
| `registration_intents` | Original input binding, validated by the existing intent owner; no acknowledgement proof alone |
| `registrations` | Validated Active is ambiguous; ExpiryObserved or Stopped contradicts pending initial publication |
| `cursors` | A registration-keyed current cursor proves later work even without deliveries |
| `deliveries` | Every phase proves later work, including retained terminal rows |
| `source_reset_facts` | Retained immutable reset proves later work after current cursor/delivery deletion |
| `source_reset_members` | Subordinate to reset/delivery owners through foreign keys and reference validation; no independent registration identity |
| `audit` | Decode and contextual validation precede classification; exactly one initial identity, with published state distinct from file existence; every other closed variant is later progress |
| `retirements` | Original target/stop/retirement identity proves completed initial and stop progress |

The table inventory is checked against the canonical schema. Audit classification
exhaustively matches Record and Cause variants, consuming the existing decoder
and contextual validator. A registration-leading audit index keeps retained reads
scoped. Cold restoration checks all retained initial families; live publication
checks registrations with pending initial records before any file effects.
Contradictory pending initial publication is rejected without repair or invented
history. Both M3 substitutions (reset-only proof and a second initial UUID beside
a published original) must fail before publication and on cold restore. Valid
lost-answer recovery and unrelated registrations remain accepted.

### General credential-stop composition boundary

The held Auth snapshot is the source of an actual original revocation fact.
A principal-only outbox stop input cannot represent LocalOperator, supersession,
original command time and effective revocation time faithfully. The isolated
correction carries the canonical fact through the application boundary;
expiry remains a distinct first-reader observation and never invents revocation.

| Ordering | Fact and recovery contract |
| --- | --- |
| Explicit revoke | Consume the actual held original fact, retaining principal, cause, journal identity/correlation and command time |
| Offline surface/owner supersession | Retain LocalOperator or the actual original principal and replacement/cause; never substitute the current observer |
| Expiry observed first, revoke later | Keep the first expiry reader/time for registration cleanup; retain the independent original Auth event unchanged |
| Original supersession command predates effective revoked_at | Preserve both original command time and the canonical lifecycle's effective revocation time; do not clamp one into the other |
| Cold retained stop restoration | Reuse Auth's journal identity and transition decoder; bind projections to the original registration target, without fabricated Credential objects |
| Missing or mismatched current canonical fact | Typed failure before admission/effects; no inferred actor or synthetic history |

Auth exposes a validated bound fact through AccessSnapshot. Its current-binding
restoration method requires a whole current Credential. Reconstructing a
Credential with invented grants solely to decode historical evidence would create
false authorization state. Copying the private journal-identity checks into the
outbox would create a second owner. The implemented seam is a clearly named
typed RevocationRecord decoder in the existing Auth fact owner, shared by its
current-binding restoration method and the outbox's secret-free historical
representation. CredentialRevocation retains its current-binding construction
contract; shape-only decoding returns a different type, never an unbound
CredentialRevocation.
Decoding proves shape, not actual journal origin or permission; current admission
still requires the actual held snapshot and its binding checks. This seam changes
no authorization policy and adds no parallel history owner.

The isolated implementation uses a distinct validated RevocationRecord for
retained shape and keeps CredentialRevocation bound at construction. It never
returns an unbound CredentialRevocation from the new decoder. RegistrationStop
retains that record or an actual first expiry reader, with an immutable derived
domain projection. The domain uses Auth's Initiator and RevocationCause values;
it does not turn a LocalOperator into a principal or erase supersession.

| Stored projection | Single retained representation and checks |
| --- | --- |
| Live registration | One bounded stop JSON value; phase decoder consumes its derived observation |
| Retirement | Same codec and complete original fact after live private configuration is removed |
| Registration audit | Complete retained stop plus human-readable projection, checked together against the owning live/retired fact |
| Delivery audit | Same retained stop, compared to the stopped registration; no authority from the record itself |
| Source restoration | Shared Auth record decoder and domain transition owner, then original registration credential/issuance binding |
| Revocation after observed expiry | Preserve the existing expiry stop record; the later canonical Auth event remains independent |

This replaces the independent SQL stop cause/actor/time columns in the current
private schema. Old malformed-state tests must exercise the new representation
without changing their refusal and valid-recovery assertions. The eight-table
inventory, capacity ceilings, acknowledgement order and external grant/credential
admission rules remain the existing owners. The [general-stop evidence](../reviews/evidence/webhook-runtime/general-stop/README.md)
records actual Auth fact integration, mutation/recovery checks and fresh independent
review of this isolated correction. Runtime mounting remains incomplete; local
qualification of this seam is not full feature acceptance.

The stop record distinguishes clocks explicitly: its `atMs` is the first local
stop observation/effect time, with millisecond precision. A revoked record also
retains the unchanged Auth transition's original command `at` and effective
`revokedAt` in seconds. Those are distinct facts. Do not replace local observation
time with a rounded Auth command time, or rewrite the original command time with
a later cleanup clock. Clock rollback does not synthesize chronology; original
journal identity remains the correlation. Expiry keeps its actual first reader
and millisecond read time. Repeated stops preserve the original retained value.

| New canonical stop input versus retained state | Before owed publication |
| --- | --- |
| Different credential or original issuance | Refuse Invalid before replaying any owed projection |
| Same credential with a different retained revocation record | Refuse Invalid; a retry cannot replace original journal identity, actor, cause or time |
| Same original record, later observer clock | Preserve the first local observation and unchanged original event |
| Prior expiry followed by actual revocation | Keep the first expiry fact while Auth retains its independent original event |

The retained physical-operation owner performs this data-correlation preflight
before its existing owed-publication step. It is not an authorization decision;
Cedar/current credential/grant admission remain required at the runtime boundary.


Retirement restoration, direct lookup and indexed credential selection now consume
one shared bounded row projection including the registration ID. Borrowed byte
validation precedes every owned field; the existing canonical UUID decoder remains
responsible for identity shape. This removes the separate outer ID allocation and
second read without changing retention bounds, audit correlation or retirement
semantics. The malformed 1 MiB ID allocation probe, maximum valid stop-size control,
indexed-limit control and ID-budget omission mutation are retained in the
[general-stop review evidence](../reviews/evidence/webhook-runtime/general-stop/README.md).

### Retained credential ownership during recovery

The recovery environment has only published general-stop source. Unpublished
cancellation source and its correction were not recovered. Before composing that
work again, retained retirement validation must consume the same indexed
credential selection as command correlation.

| Retained relationship | Decision before restoration or owed publication | Regression |
| --- | --- | --- |
| Two retired registration IDs name one credential | Invalid; neither projection may publish or trim evidence | Duplicate complete retirement histories, live and cold refusal, repaired original-byte recovery |
| Live and retired registration IDs name one credential | Invalid; a live row cannot bypass the retained original owner | Matching intent/live row with a conflicting complete retirement, live and cold refusal |
| Distinct credentials own distinct live or retired registrations | Accept, including owed Drained acknowledgement recovery | Positive controls for both relationships |

This is retained data correlation, not permission. It adds no durable state,
acknowledgement flag, schema, credential authority or cancellation behavior.

### Cancelling an issued but uninstalled reservation

Recovery must not mint a credential merely to discover whether an earlier issue
finished. A registration command that reads its actual original issuance and a
current revocation or elapsed expiry can remove its uninstalled private intent.
It must not install the destination temporarily or call that cleanup a drained
registration. The following is the next implementation slice; runtime command
supervision and startup enumeration remain separate composition work.

| Before / ordering | Owning fact and effect | Regression required |
| --- | --- | --- |
| Reserved, actual original issue matches, credential revoked | Held original Auth revocation, including actual actor/cause/time, cancels the uninstalled intent and retains secret-free cancellation evidence atomically | Explicit and supersession facts, no installed destination or second credential |
| Revocation commits after the issue reply but before its current Auth hold | Validate the actual original held revocation, advance only that lifecycle field from the earlier reply, and retain full credential binding equality before cleanup. A changed audience or grant remains inconsistent. | Actual Auth race for uninstalled and installed ownership; substituted current audience/grants refused before cleanup, valid retry accepted |
| Reserved, actual original issue matches, expiry elapsed | The actual command reader and local read time become the cancellation stop fact; no Auth expiry event is invented | Expiry and clock-boundary controls |
| Matching identity with disabled current membership and a canonical stop | Cleanup consumes the actual stop despite lost installation authority. A current credential without a stop remains uninstalled; membership state alone creates no expiry or revocation. | Disabled expiry/revocation cleanup and current-credential no-install control |
| Current credential, future issuance, missing/mismatched original issuance or revocation | No cancellation or installation based on missing proof | Typed refusal, no audit publication or replacement |
| Installed registration | Cancellation refuses; the existing stop/drain lifecycle owns it | Live and stopped installed controls |
| Cancellation commit, audit write or acknowledgement lost | Remove URL/key/intent in the same commit as a distinct Cancelled retirement kind and its exact audit identity; retry replays original bytes | File failure, lost acknowledgement, cold recovery and held Auth lifetime |
| Cancelled versus Drained retained facts | The retained kind owns which terminal cause is possible. Cancelled accepts only its single matching cancellation audit; Drained cannot accept cancellation evidence | Both contradictory cause orders, duplicate identities, cross-kind history, duplicate credential and live/retired overlap |
| Valid distinct credentials | One cancellation must not borrow another registration's facts or block its valid recovery | Positive live/drained/cancelled controls |
| Retained acknowledged fragments | A drained history can retain initial issue, first expiry observation, first stop and drain. Read five indexed rows as the impossible fifth sentinel; retain initial UUID decisions in the existing progress owner, and reject duplicate lifecycle facts before publication or trim. | Four actual original acknowledged facts accepted; fifth published stop/observation duplicate refused, including cold open |

The retirement kind is data correlation, independent of its audit projection.
There is no new table, authority, acknowledgement flag or parallel stop codec.
The existing bounded retirement projection and credential selector cover both
kinds. The private schema remains one current contract and refuses old shapes.

### Principal-authorized canonical record reads

The next source slice uses a separate principal selector, never a receiver
binding. Existing `AdmitDelivery` remains the sole credential/grant/Cedar gate.
A selector is data, not permission. Its replication consumer ID is the existing
registration UUID, matching the durable outbox; it creates no receiver binding.
Physical SDK payloads are binary; HTTP composition must use the existing
recordsPage encoder and preserve its RecordWireRecord JSON, not parse raw frame
bytes as text. The physical SDK reader shares its existing
tracked worker, budgets and exact identity checks through two concrete entries.

| Ordering | Effect | Required control |
| --- | --- | --- |
| Before any source I/O | Try-reserve the composition-owned global record slot without waiting, then acquire Auth hold, then principal grant revision hold, then load actual metadata and ask existing webhook Cedar policy | Other chat/tenant/owner/credential and stale epoch call no source |
| Principal page request | Compare registration-specific selector, conversation and epoch before SDK I/O; validate returned page through canonical sync owner | Wrong selector/request/body budget refuses without checkpoint advancement |
| Physical identity changes | Compare complete SDK-owned origin/stream/incarnation/schema with requested tuple | Replaced source refuses; receiver entry remains unchanged |
| Read waiter disappears | Actual read fence and capacity remain with tracked physical worker until source join | Revoke/unshare still wait; shutdown joins original work |
| Adapter substitutes a valid reply lease | Original capacity and credential/grant fences share one application/physical owner; returned lease data cannot release or manufacture that ownership | Actual canonical read with replaced reply lease retains global capacity until result drop |
| Read completes | Retain original fence with bounded result; check absolute expiry before exposing it | Expiry during read and after return refuses data exposure |
| Cursor or sender later consumes the result | This source-only slice creates neither cursor nor HTTP effect. Later composition must retain the fence through queue acknowledgement and independently re-admit each send | No source-only result is labelled recipient E2E or current send permission |

No new Cedar action, paired principal, product session or receiver authority is
introduced. No protocol, SDK or client contract changes are required.

### Principal share composition

The webhook-specific share use case resolves only a committed active private
registration. It does not synthesize a receiver or grant a principal owner access.
The existing conversation.share ownership/target Cedar rules and canonical grant
transaction remain authoritative. Credential binding validation is shared with
source/send admission, and the actual Auth hold stays alive through the grant
commit. No protocol, SDK, client or Cedar permit expands in this slice.

| Input/ordering | Required result | Evidence |
| --- | --- | --- |
| another owner or organization | ownership refusal before private lookup; no grant | `private_target_is_never_looked_up_for_another_owner` |
| live Auth principal, no committed registration | no target fallback and no grant | `a_live_auth_principal_without_a_committed_registration_is_not_a_share_target` |
| registered read principal | exact conversation/principal/credential grant, receiver absent | `registered_principal_share_has_no_receiver_and_grants_exactly_one_conversation` |
| comment or drive | existing Cedar refusal, no grant | `webhook_comment_and_drive_are_refused_by_existing_cedar_before_any_grant` |
| stopped/expired/mismatched target or disappearing registration | refuse before grant | `revoked_and_expired_credentials_cannot_create_a_principal_grant`, `replaced_or_disappearing_private_registration_cannot_be_granted` |
| cancelled during target recheck | original Auth hold released; no grant | `cancelling_share_during_private_recheck_releases_original_auth_without_grant` |

This is an inner share boundary, not product command mounting, a source-feed
supervisor or recipient delivery E2E. Expiry is rechecked before grant mutation;
subsequent source/send admission independently checks current expiry and revision.

The canonical grant port additionally transfers the actual Auth hold to its
physical writer. Unsupported adapters refuse this ownership-bearing operation;
ordinary receiver changes retain their current signature and path. Native grant
retries share one original Auth owner, and each blocking attempt owns its clone
until commit/rollback/panic finishes. Its mutex is never locked. Acquisition stays
Auth then principal grant then metadata connection; no new task supervisor or
state flag is introduced. The existing test-only grant-write pause is reused,
with crate visibility for this integration test.

| Physical boundary | Required result | Evidence |
| --- | --- | --- |
| caller cancelled after real blocking grant write enters | actual original Auth hold survives until physical commit; real revocation waits, then admission refuses | `cancelled_physical_share_write_retains_original_auth_until_actual_commit` |
| expiry elapses during private recheck | no grant mutation despite the held mutation fence | `expiry_during_private_registration_recheck_refuses_before_grant` |

The positive share test reads a genuine synthetic SDK source under the new grant.
This still mounts no HTTP sender and claims no recipient E2E or command supervisor.

The grant-only native adapter derives its exact credential and principal from
the actual Auth hold and constructs receiver absence.
`credential_owning_grant_port_derives_exact_principal_and_credential` checks
the complete actual stored target against those held facts. Contradictory
recipient/transition fields are not input parameters.

`physical_grant_panic_rolls_back_and_releases_original_auth_after_worker_exits`
injects a panic through the existing native store policy dependency inside the
actual blocking transaction. The joined typed failure has no grant, and real
revocation completes after the worker unwinds. This does not claim supervision
of the still-unmounted outer product command.

### Principal share review M1: receiver absence is exact

Round1 at e64af135 reproduced a false fresh/replay acknowledgement from the
ordinary grant writer's intentional None-means-keep/lost-reply semantics. The
principal-specific credential-owned operation must instead mean receiver absent.
It remains on the same native transaction and grant gate; ordinary paired change
semantics stay unchanged. No implicit receiver removal, new state or permit.

| Relationship at actual native boundary | Required result before acknowledgement/effects |
| --- | --- |
| dedicated grant input | accepts resource/access/revision/actor/time and actual Auth hold; derives exact principal/credential and constructs Grant with receiver absent |
| live same-principal row retains any receiver | refuse before replay acknowledgement or fresh mutation; preserve row/journal |
| original request journal retains a receiver, including after later unshare | refuse its reinterpretation as a receiver-absent request |
| original receiver-absent request after unshare | return its original historical receipt without recreating the grant |
| ordinary paired change omits receiver | preserve its existing keep/lost-reply meaning |

The dedicated port is named `grant_principal_with_credential` to identify the
exact contract. The actual retained credential owner identifies this native
entry path; no caller-provided mode flag or persisted state is added.

The dedicated grant-only signature accepts no receiver, revoke, or separate
principal/credential field. Its native mapping constructs those facts from the
actual held credential, preventing contradictory input shapes without a new DTO
or mode/state flag. Current receiver evidence is still checked transactionally
before replay; historical receiver evidence is checked before replay receipt.

### Product runtime composition

One concrete runtime owns configured credential commands and the source/sender
lifetime. Configuration selects a principal on the existing credential.issue
contract; it supplies no new wire fields. Actual auth command transitions remain
attached to the secret-free registration acknowledgement. All issue modes pass
one retained issuance gate. Caller loss drops only observation; shutdown seals
admission and joins admitted work before dependencies are released.

| Boundary | Required behavior |
| --- | --- |
| configured principal issue | original durable reservation, actual Webhook issuance/install, secret-unavailable wire result with actual command transitions |
| ordinary issue | unchanged auth outcome/one-time secret, through the same retained issuance owner |
| existing different command for principal | conflict before another issuance |
| revoke | actual canonical revocation, then current original stop coordination before acknowledgement |
| startup | reconcile bounded stored unfinished commands before admitting source/send work; unresolved work refuses startup |
| shutdown | seal, stop source/send admission, retain physical work, join command owners; unresolved work is not successful cleanup |
| first command panic | mark failed and seal admission before waking the response observer; actual retained drain remains failed |
| registered-webhook share/unshare admitted | retain the existing authorized grant operation in the same bounded owner until its physical write joins; ordinary grant targets keep their existing path |
| stop while a canonical grant write is in flight | retain original share/unshare work, poll other drains, and withhold successful webhook cleanup until the mutation returns |

These rows govern the composed path; adding its wiring does not itself qualify
recipient delivery, known-outcome settlement or startup/shutdown fault behavior.

### Ordinary registry writer handoff

A registry mutation must serialize with another physical registry mutation before
trying the effect fence. Taking the effect fence first turned ordinary pairing
writer contention (including a no-op expiry) into `Locked`, which pairing maps to
`Unavailable`. The original contention probe holds a real expiry in its Clock;
expiry and cancellation both fail before their own Clock/persistence stage.

| Ordering | Required outcome |
| --- | --- |
| ordinary synchronous writer behind another registry writer | wait on the existing canonical registry mutex in the already admitted physical worker, then try the effect fence; do not add a task or retry loop |
| physical writer finishes | release its effect fence before releasing the registry mutex, so the next ordinary writer cannot observe the previous writer's fence |
| synchronous mutation encounters a current credential read hold | keep the existing `Locked` refusal; no credential/grant effect or blocking fence acquisition |
| asynchronous revoke waits for read holds | await the effect fence without a registry mutex; once obtained, take the registry mutex and retain both through persistence/publication |
| async revoke owns its fence while another registry holder finishes | the synchronous holder only tries the fence and releases on refusal; it never waits for the fence while owning the registry mutex |
| observer cancellation | the existing admitted physical worker retains its operation and guards; no new detached worker or nested blocking task |

Only the existing mutex may wait synchronously, as before effect fencing was
introduced. Native owner/connection composition already runs these physical store
operations on bounded admitted blocking workers. The fence acquisition under that
mutex remains nonblocking. No opposite-order cycle can wait on both locks: async
revocation never awaits its fence with the registry mutex held, and a synchronous
mutation never waits for a fence. Current authority and revision checks still run
against the locked canonical registry before persistence.
