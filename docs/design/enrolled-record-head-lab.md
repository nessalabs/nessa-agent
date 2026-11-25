# Enrolled record Head lab (#1378)

The lab uses the workspace's single Sync Engine pin
`341c9b844257403bb297f27d7ed9aecbe93e4406` (Sync Engine #121) and the same typed
scope as an ordinary Product read. A supplied scope is a selector, not a grant.
The typed native v1 carrier is compiled but unmounted in ordinary gateway
composition; no client switches to it. The private fixture uses the same
mount seam, not another selector.

The existing `read_only_online` fixture already starts a gateway with the
real listener, relay, credential registry, receiver authority, Cedar and SDK
record source. It pairs its own device, approves consent, and stores one local
device key and the gateway pin. Reuse that fixture and its existing grant;
do not pre-approve a new credential or take a second listener.

| State | Input/decision | Next state and effect | Required lab evidence |
| --- | --- | --- | --- |
| Dial selected before connection | Direct or forced local relay | Verify the existing native TLS key and exact gateway pin; both routes use the same connection owner. | Split direct and relay-only legs; same enrolled device and source identity. |
| Native TLS established | Explicit `openRecordCarrierV1` envelope | Handle one bounded native opening, retain its original TLS and existing wake/permit owner, then give bytes exclusively to the selected codec. The ordinary product selector stays unchanged. | Disabled fixture route refuses; the default paired product read remains unchanged. |
| Probe opening | Named issued credential, checked on the just-proved key by the current product verifier and `AuthenticateSession` | Capture existing authenticated context. No scope, source or subscription grant comes from the selector. | Enrolled credential admits; revoked credential or missing current grant refuses the normal read. |
| One selected scope | Current `AdmitPassiveRead(RecordHead)` then one bounded `ReadRecords(Head)` | Obtain the physical identity from the existing record source, correlate exact receiver, access epoch, conversation and scope. Bind/Head consult the same authorization owner before acquisition and release. | Exact semantic head matches the existing authorized product read; no cache/checkpoint effects. |
| Head returned, unsupported, refused, timeout or closed | Result accepted or failed | Close this probe. Never reopen on the product protocol or reacquire a new credential. | Receipt or typed terminal result within the fixture deadline; no fallback dial. |

On direct or joined relay, the first `openRecordCarrierV1` envelope selects
one exact conversation scope and explicit `head` or `page` mode. The existing
native framing replies `recordCarrierV1Accepted` only after current enrollment
and requested Product operation admission. It is not a source identity
receipt; NSR1 Bind still checks the real physical scope. A refused or disabled
selection gets only the existing redacted `refused`, and closes. An older peer
or an unsupported version never starts NSR1 or falls back to Product. The
tunneled carrier remains unsupported.

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
advisory HEAD_CAP; Bind's own Head authorization is still mandatory. In
HEAD-only mode the adapter denies Page and unselected operations.
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
separate probe receives the redacted native selection refusal and then closes
without an NSR1 byte. A timeout is not accepted as evidence. This test does not qualify revocation mid-frame.

## Explicit two-page restart fixture

The Page fixture selects `mode: "page"` on the typed v1 opening; the Head
fixture explicitly selects `mode: "head"`. Both use one finite native TLS
connection, one exact conversation scope, and (in Page mode) one Page
allowance per connection. Opening proves the original enrolled
TLS device; current credential, receiver, access epoch and grant checks use
the ordinary Page authority. Limits come from the existing Product record
ceilings; this receipt requests one record per page. The binary session does
not fall back to Product. The codec also maps its combined OPEN to the Page
permission; this fixture only sends fixed-target Pages.

Each Page uses the existing gateway physical record worker and one existing
global permit. The operation view keeps that permit through canonical
encoding, but releases it before the writer can send a byte. If the physical
wait is uncertain, the worker keeps its lease and this connection can admit
no further source reads. The fixture does not retry that case.

For each route, the fixture commits exactly one bounded row, closes the
connection, and reopens the same SQLite cache after a gateway restart.
A fresh pinned connection fetches the next row with the saved cursor. The
existing retained pass and cache own validation and commit. Both prefix rows
must match positions, IDs and bytes from the ordinary authorized Product
read. The receipt reports actual decoded payload and protocol byte counts;
it excludes the underlying TLS and relay carrier bytes. This is a two-page
prefix, not a full download, mid-frame loss, live failover or performance
qualification.

The ordinary Nessa receiver/access epoch gates a product grant. It does
not prove an origin authority epoch, and cannot substitute for a separately
trusted origin claim/ledger and stamped receiver commit. Catalogue reads
retain their separate scope, permission, manifests and deletion coverage.
