# External event admission (#1088)

## Design checkpoint

GitHub CI and Linear events are inbound webhooks. They do not require a persistent
bidirectional WebSocket connection. This slice provides a composable HTTP router,
not provider registration, an automation editor, or a workflow engine. A workflow
is presently one queued turn in an existing, server-selected conversation.
Production composition does not mount this router. #896 owns outbound delivery;
#1087 owns frontend lifecycle verification. Neither is changed here.

The trigger binding supplies an already authenticated Nessa session, a unique
stable trigger identity, an existing conversation, a signing key, current access,
Cedar, clock and command storage. Incoming JSON cannot choose an actor, target,
permission mode, tool, file, or credential. Each event asks existing current-state
`AuthorizeAction` for `conversation.write`; conversation admission and SDK tool
permissions remain their existing owners. Composition must authenticate the
binding through `AuthenticateSession`; a caller DTO is insufficient.

A sender signs `"nessa-event\n" + binding-id + "\n" + timestamp + "\n" + raw JSON` with HMAC-SHA256 and sends the standard
base64 MAC as `x-nessa-signature`, and Unix seconds as `x-nessa-timestamp`.
Each binding needs its own random 32-byte or longer key. Keys are constructor
inputs, never request fields or persisted by this primitive. A future provider
adapter translates verified provider envelopes into this contract; these headers
are not GitHub's or Linear's native signature format.

JSON is `{ "id": "provider-delivery-id", "payload": ... }`. Event identities use
1–128 ASCII letters, digits, `_`, `-`, `.` or `:`. Bodies are at most 16 KiB;
requests have a five-second body deadline and eight concurrent handler slots.
A timestamp more than five minutes away from the local clock, in either direction, is refused. Exact retries
can be re-signed with a fresh timestamp. Deduplication uses the stable trigger and
event identities, independent of timestamp and key rotation. Payload changes under
the same event ID conflict through the existing command fingerprint.

## Ordering and failure table

| Row | Event / ordering | Result and owner | Regression |
| --- | --- | --- | --- |
| E1 | Invalid signature, missing headers, stale/future timestamp | 401 before JSON/admission | ingress rejection tests |
| E2 | Malformed, unknown fields, invalid id, oversized body | 400/413, no command | ingress rejection tests |
| E3 | Revoked/expired binding, denied Cedar, unavailable authority | 403/503 before command | authorization tests |
| E4 | Valid event | SDK saves Accepted then Attempted before enqueue, then Settled | HTTP execution test |
| E5 | Same event arrives concurrently or after response loss | Command-store lease excludes competing owners; overlap may return 503, settled retry returns same turn | duplicate/concurrency test |
| E6 | Same event ID with changed payload or target in the same principal namespace | Existing binding conflicts; no replacement command | conflict test |
| E7 | Storage fails before durable admission | 503, sender retries same ID | recovery test |
| E8 | Crash/error after Attempted, no saved terminal receipt | 409 interrupted, no automatic redispatch | existing mutation recovery tests |
| E9 | Execution fails or requires permission after enqueue | Dispatch receipt remains dispatch; existing turn view reports result/review | execution tests |
| E10 | Caller disconnects after command handoff | Existing supervised submit owns command; retry same ID | existing mutation caller-loss tests |
| E11 | Explicit cancellation / execution retry | Existing exact-turn stop; retry execution requires an explicit new event ID | `queued_event_can_be_stopped_with_existing_exact_turn_command_and_replay_does_not_revive_it` |

202 means durable dispatch receipt, not workflow completion. It is returned only
for Settled/Dispatched, including an identical replay. A 503 is not proof that no
effect happened: the response may be lost after dispatch. Retry the same event ID;
never mint a new ID to recover an uncertain delivery. Interrupted is an explicit
operator reconciliation state, not a retry invitation. There is no autonomous
inbox worker or redelivery promise. The sender retains undelivered events.
Command receipt retention follows the existing command store: at most three bounded records per request (`session_storage/creation.rs`, `MAX_REQUEST_EVENTS`). There is no new in-memory delivery history. Distinct event identities create distinct durable streams; there is no total stream-count quota or automatic time-based receipt pruning in this slice. A future production binding must define its storage lifecycle before mounting the router. Deduplication does
not survive erasing that store; binding identities must not be recycled. This
primitive does not promise exactly-once external tool effects.

The body bound is applied to received bytes without decompression. JSON parsing
uses serde's default nesting limit. Envelope fields are strict; payloads are
opaque JSON data, not trusted instructions or tool authority. Header/read limits,
TLS, connection rate limits, and shutdown belong to the listener that eventually
mounts this optional router. The eight-slot bound covers active handlers, not
workflow execution count; the existing conversation service bounds detached work.

## Composition and remaining integration boundary

The HTTP adapter assumes the supplied `CreationStorage` preserves its exclusive
request-lease contract and the host preserves its existing namespace ownership.
This slice tests one composed gateway instance and reopen, not multiple independently
configured gateways writing one namespace. It does not create provider credentials,
register public hooks, or install a persistent signing-key store. Binding target,
identity and request-store lifecycle are composition choices, never event fields.

The permanent reject responses are 400 invalid event, 401 invalid/stale signature,
403 current authorization refusal, 409 changed event or interrupted dispatch, and
415 unsupported content type. Body overflow returns 413 and read timeout 408.
Capacity and unconfirmed command/storage delivery return 503. For a 503 or lost
response, retain the original ID; an existing receipt can be replayed after the
problem clears. A deleted or otherwise unavailable conversation currently returns
503 `dispatch_unconfirmed` from this small transport adapter; reconcile the target
through the existing conversation API rather than expecting automatic redelivery.

A trigger's authenticated principal, credential and target are immutable binding
facts. Rebinding to another principal is not detected as a conflict: command
storage is principal-scoped. Such a rebind requires a new trigger identity and
signing key. Reusing a trigger identity across principals is unsupported, rather
than an enforced cross-principal deduplication guarantee.

## Source and regression map

- [HTTP signature and resource boundary](../../crates/nessa-server/src/external_events/entrypoint/http.rs).
- [Current authority and durable submit mapping](../../crates/nessa-server/src/external_events/application/admission.rs).
- [Delivery identity value](../../crates/nessa-server/src/external_events/domain/value_objects/identity.rs).
- [Loopback HTTP, failure, concurrency and reopen tests](../../crates/nessa-server/tests/external_events/ingress.rs).
- [Existing command crash, caller-loss, permission and exact-stop regressions](../../crates/nessa-server/tests/conversation/mutation_commands.rs), including
  `an_interrupted_submit_does_not_enqueue_again`,
  `a_submit_admitted_before_retirement_does_not_hold_it_back`, and
  `an_owner_turn_offers_remember_only_when_the_membership_may`.

The existing conversation stress scenarios `stop`, `fail` and `restart` exercise
the shared execution owner. This slice changes no conversational command or UI;
the new, unmounted HTTP boundary is exercised by the isolated ingress suite.
