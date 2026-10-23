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
