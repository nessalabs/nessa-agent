# Enrolled record Head lab (#1378)

The lab uses the workspace's single Sync Engine pin
`341c9b844257403bb297f27d7ed9aecbe93e4406` (Sync Engine #121) and the same typed
scope as an ordinary Product read. A supplied scope is a selector, not a grant.
No real app mounts the lab or selects this transport.

The existing `read_only_online` fixture already starts a gateway with the
real listener, relay, credential registry, receiver authority, Cedar and SDK
record source. It pairs its own device, approves consent, and stores one local
device key and the gateway pin. Reuse that fixture and its existing grant;
do not pre-approve a new credential or take a second listener.

| State | Input/decision | Next state and effect | Required lab evidence |
| --- | --- | --- | --- |
| Dial selected before connection | Direct or forced local relay | Verify the existing native TLS key and exact gateway pin; both routes use the same connection owner. | Split direct and relay-only legs; same enrolled device and source identity. |
| Native TLS established | Explicit test-composed Head selector | Handle one bounded native opening, retain its original TLS and existing wake/permit owner, then give bytes exclusively to the selected codec. The ordinary product selector stays unchanged. | Disabled fixture route refuses; the default paired product read remains unchanged. |
| Probe opening | Named issued credential, checked on the just-proved key by the current product verifier and `AuthenticateSession` | Capture existing authenticated context. No scope, source or subscription grant comes from the selector. | Enrolled credential admits; revoked credential or missing current grant refuses the normal read. |
| One selected scope | Current `AdmitPassiveRead(RecordHead)` then one bounded `ReadRecords(Head)` | Obtain the physical identity from the existing record source, correlate exact receiver, access epoch, conversation and scope. Bind/Head consult the same authorization owner before acquisition and release. | Exact semantic head matches the existing authorized product read; no cache/checkpoint effects. |
| Head returned, unsupported, refused, timeout or closed | Result accepted or failed | Close this probe. Never reopen on the product protocol or reacquire a new credential. | Receipt or typed terminal result within the fixture deadline; no fallback dial. |

The lab preselects a conversation and obtains its exact scope via the
existing authorized discovery path. Record discovery is not a claimed
cold-open optimization. The new request must not be given a value that
stands for a host claim about source origin. The source owns physical origin,
stream, incarnation and schema; Auth owns credential and receiver access
epoch. The negotiated `Bound` Head is advisory too.

Reuse the existing native worker's finite phase deadline (or shorten it),
existing connection permit, the existing gateway record semaphore, and
shutdown wake. Do not add a detached Tokio worker, nest TLS or switch after
the product stream began. The binary phase is one opt-in connection with one
advisory HEAD_CAP; Bind's own Head authorization is still mandatory. The probe's
operation adapter denies Page and every operation not selected by this lab.
The existing physical source refuses pruned streams; HEAD_CAP publishes only
the correlated scope and position, with no extra capability requirements for
this schema. It does not claim an origin epoch or offer writes. The upstream
#121 operation callbacks do not authenticate Nessa callers.

This is not production negotiation, cross-OS or iPhone acceptance, continuous
Follow, catalogue, an origin-epoch migration, or a page/durable-cache claim.
The local relay is real and opaque, but does not constitute a deployed relay
or a weak-link performance result.

The integration test is
`composition::read_only_online_tests::relay_reading::record_head::enrolled_head_matches_the_existing_product_read_on_direct_and_relay`.
It obtains the exact scope from a separately authorized old-path read,
then compares the two binary replies to that checkpoint without moving
it. Both routes refuse when the selector is not installed; the ordinary
product path still reads successfully. The real credential store revokes
the same device. On direct and joined local relay, pinned enrollment status
returns the exact correlated terminal cause `CredentialRevoked`. The
separate probe then closes before NSR1 with EOF or a connection reset, not a
typed NSR1 refusal from an authenticated session. No reply byte or timeout
qualifies. This test does not qualify revocation mid-frame.
