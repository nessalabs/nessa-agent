# Device pairing and native secure connections

Owned by [issue 264](https://github.com/nessalabs/nessa-agent/issues/264), with the native connection boundary shared with [issue 265](https://github.com/nessalabs/nessa-agent/issues/265). Accepted by the coordinating product/architecture owner on 2026-09-29. Implementation is in progress; this document describes the accepted target, not a completed feature. The client is a simple native Rust example; no native mobile app, relay or hosted account is in scope.

## Auth producer slice

The current producer slice contains the auth pairing domain, application ports,
local registry and private-storage/TLS/OPAQUE adapters. Native listener and
protected product activation remain subsequent consumers; the accepted target
and unresolved full resource policy below remain open. No existing product
protocol defines issuance-cause data: that persisted representation belongs to
auth's `IssuanceCauseDto`, exhaustive domain mapping and registry codec.

Pairing identity/key/code widths belong to auth's `wire-values.json`. The existing
product generator consumes that owner once through its pairing-values helper and
derives both auth's Rust constants and `pairing-values.generated.json`. Producer
A publishes those value shapes without adding native product methods or activating
a listener; subsequent consumers use this same publication. Issuance cause remains
in the persisted auth DTO/registry mapping, independently of these width values.

| Private acknowledgement path | Original owner and ordering | Refusal and evidence |
| --- | --- | --- |
| Live published object | Original PublishedPrivateFile remains alive while acknowledgement compares exact bytes and original named identity, syncs that same file, rechecks identity and syncs directory under original lock. | Wrong bytes/name/lock refuse; reopened equivalent bytes cannot substitute for original live identity. Public pending-state fixtures cover same-save retry and restart. Direct retained-handle failed acknowledgement is not publicly reproduced by this producer. |
| Restart acknowledgement | Reopen existing canonical name through private reopen_for_acknowledgement using existing storage OpenMode::ReadWrite; then invoke the same acknowledgement implementation. | Never create missing canonical name. Public save/load/reopen checks exact original bytes and intent. Direct writable-handle reflush and read-only substitution evidence is withdrawn; the storage owner retains the writable-open contract. |

## Decision

Recommend **OPAQUE-3DH through `opaque-ke = "=4.0.1"`**, with the Ristretto255/SHA512 suite and explicit Argon2id configuration below. Run enrollment over TLS 1.3 with RFC 7250 raw Ed25519 public keys using the **already locked** rustls 0.23.45/ring 0.17.14/tokio-rustls 0.26.5. Bind the TLS channel, both actual public keys and exact enrollment intent in OPAQUE's built-in authenticated context. Use TLS record protection to deliver claims and receipts; do not add an application encryption protocol or send the PAKE session key over the network.

This satisfies the issue's request for a reviewed PAKE with a concretely reviewed application profile; it does **not** claim an independent audit of this Nessa profile or of opaque-ke 4.0.1. The missing current-version audit is a disclosed assurance limit, not an invented requirement to hire an auditor or stop implementation. Normal repository implementation review and enforcing tests still apply.

Protocol evidence: [RFC 9807](https://www.rfc-editor.org/rfc/rfc9807.html) is the CFRG consensus Informational specification for OPAQUE, with 3DH mutual authentication, application context/identities, serialization and vectors. Its registration and login are different operations. [RFC 9382](https://www.rfc-editor.org/rfc/rfc9382.html) describes SPAKE2 and bilateral confirmation, but was not the CFRG competition selection. RustCrypto's [SPAKE2 README](https://github.com/RustCrypto/PAKEs/blob/master/spake2/README.md) expressly disclaims an independent security/correctness audit and describes its Python-compatible Ed25519 implementation. Its [open transcript issue 186](https://github.com/RustCrypto/PAKEs/issues/186) also warns against assuming RFC wire interoperability. Selecting that implementation and then retrofitting RFC operations would be a larger crypto change than selecting OPAQUE's existing APIs.

The [opaque-ke 4.0.1 manifest](https://github.com/facebook/opaque-ke/blob/v4.0.1/Cargo.toml) declares Rust **1.85** (README agrees); the changelog's 1.83 entry is less authoritative than effective package metadata. Nessa auth already requires 1.89; workspace minimum 1.85 does not override this. Stable 4.0.1, released 2025-11-03, is tagged commit `75fe4cdddb7946440054da0c8e7cdd73828af3f9`. Main currently recommends 4.1.0-pre.2 and requires 1.87: do not silently use that prerelease. Check [releases](https://github.com/facebook/opaque-ke/releases) and the tagged manifest when implementing.

The [NCC original report landing page](https://www.nccgroup.com/research/public-report-whatsapp-opaque-ke-cryptographic-implementation-review/) confirms a June 2021 review. The [tagged README](https://github.com/facebook/opaque-ke/blob/v4.0.1/README.md) attributes that review to v0.5.0 and says fixes were incorporated by v1.2.0. The [changelog](https://github.com/facebook/opaque-ke/blob/v4.0.1/CHANGELOG.md) documents later protocol/dependency changes. This evidence cannot extend the 2021 audit to v4.0.1, its present transitive tree, SIGMA-I, Nessa's application binding, raw-key bootstrap verifier, or TLS profile. Do not copy secondary claims calling 4.0.1 audited. No audited current alternative was established by this focused review; no such claim is needed for this recommendation.

## Concrete library profile

Features: opaque-ke default features **off**, `ristretto255` and `argon2` enabled; no serde or SIGMA-I. Define its existing `CipherSuite` with `OprfCs = opaque_ke::Ristretto255`, `KeyExchange = opaque_ke::TripleDh<opaque_ke::Ristretto255, sha2::Sha512>`, `Ksf = opaque_ke::argon2::Argon2<'static>`. Supply an explicit KSF to both local registration finish and client login finish: Argon2id, version 0x13, memory 65536 KiB, iterations 3, parallelism 4, output 64 bytes. The library supplies its 16-byte zero salt to KSF over the already OPRF-derived password; do not independently salt/prehash the code. This is an application parameter choice drawing on [RFC 9106's memory-constrained option](https://www.rfc-editor.org/info/rfc9106/), adapted to this suite's 64-byte output, not a claim to implement RFC 9807's 2-GiB default profile. Do not accidentally use `Argon2::default()` or `ksf: None`.

Tagged dependencies include curve25519-dalek 4, voprf 0.5 (upstream enables `danger` for implementation needs), hkdf/hmac 0.12, digest 0.10, rand 0.8, subtle 2.6, zeroize 1.8, derive-where 1.4, elliptic-curve 0.13, generic-array exactly 0.14.7 and optional argon2 0.5. Selecting default features off avoids extra serialization and optional ECDSA/Ed25519/SIGMA-I dependencies. These are manifest requirements, not a resolved Nessa transitive lock: the implementation owner must resolve one lockfile, record effective versions/MSRVs/advisories and check supported macOS/Linux/Windows targets. No broad package checks were run because no dependency changed. Pure Rust PAKE primitives and ring-backed TLS fit the simple native Rust client; WASM/native mobile is outside this review. ring has native build tooling/platform requirements; a manifest minimum is not cross-target compilation evidence.

Use the tagged [OPAQUE API](https://github.com/facebook/opaque-ke/blob/v4.0.1/src/opaque.rs), not pseudocode reimplementation. It exposes ClientRegistration start/finish, ServerRegistration start/finish, ClientLogin start/finish, ServerLogin start/finish, `Identifiers`, `ClientLoginFinishParameters { context, identifiers, ksf }` and `ServerLoginParameters { context, identifiers }`. The client verifies KE2 before emitting KE3; the server verifies KE3 before recording a successful proof. A client-side finish alone never publishes a claim or credential. The library's [RFC vectors file](https://github.com/facebook/opaque-ke/blob/v4.0.1/src/tests/rfc9807_vectors.rs) and associated tests are evidence of upstream vector coverage; they are not evidence Nessa has run those vectors. Run them when implementing and add Nessa profile fixtures using the actual KSF, identities and context, including every failure case below.

Use injected OS CSPRNG work through the consuming application's randomness seam, adapted to the library's rand 0.8 CryptoRng/RngCore bound. Do not confuse Nessa's existing getrandom 0.3 dependency with the required rand interface. CSPRNG failure returns a typed failure. The library zeroizes some handshake state, but output keys and app copies need explicit Zeroizing ownership; generated code bytes, registration client state/export keys and login output keys must be cleared after use. Redacted Debug is not zeroization. Never serialize handshake state just to ease retries.

## Invitation secret, budget and restart policy

Proposed defaults, expressed once as validated composition policy: lifetime 10 minutes, exclusive expiry; maximum **5** admitted PAKE attempts per invitation; maximum 1 pending attempt per invitation; maximum 32 live invitations, 8 concurrent network pairing handshakes, and 1 invitation-creation KSF worker. One pairing attempt may exchange only the fixed profile frames, with 4-KiB application payload limit with a separate four-byte framing prefix, fixed canonical context profile, 4-KiB cumulative TLS handshake ingress/egress ceilings, 10-second TLS handshake deadline, 30-second PAKE deadline and a provisional 128-KiB connection transport/handler-memory target. This engineering target is unproven and may be revised at the composition policy owner after actual maximum-valid and retained-lifetime accounting. It includes connection-attributable raw/base64/JSON/TLS/parser/auth snapshot/application copies; separate KSF, runtime stacks, process baseline and kernel classes are excluded. No unbounded queue; capacity refusal precedes attempt admission. Close and cancel release physical workers before releasing their capacity. These values are a concrete initial policy proposal, not requirements quoted from an RFC or user; performance tests may change one policy owner with revised evidence.

Manual password: 8 independent uniformly sampled symbols from the 32-symbol ASCII alphabet `ABCDEFGHJKLMNPQRSTUVWXYZ23456789`, displayed `XXXX-XXXX`. This is 40 bits of generated entropy, with at most 5/2^40 online-success probability per uniformly generated invite under the online PAKE model. It is not a user-chosen password. Canonical input accepts exactly 8 alphabet symbols, or a hyphen solely after the fourth symbol, with ASCII lowercase mapped to uppercase. Reject spaces, Unicode folding, O/I/0/1 aliases, extra hyphens and other lengths. Do not trim or silently normalize other input. Parse locally before starting crypto. Keep the public locator separate: random 128-bit invitation ID, not a code-derived lookup. Enter code through a secret prompt, never command arguments/environment/logs/history. Public gateway address is a locator, not a trust root.

Optional QR invitations can use 32 random secret bytes and carry the gateway SPKI pin, public locator, fixed profile and address. That is a different secret kind on an invitation, not a fallback after PAKE failure. If a QR encodes the same manual code, it still has only 40 bits; do not claim 256-bit QR assurance while showing a short equivalent secret. Both modes use the same one-use decision owner and PAKE exchange; manual mode alone is enough for the initial requested example.

At invite creation the gateway runs the **ordinary complete OPAQUE registration flow locally**: it is both temporary registration client and server, on the generated password, with gateway identity and invitation identity fixed. No remote registration endpoint exists. Store ServerRegistration and a per-invitation ServerSetup only in secret memory; erase temporary client/export key/code copies after the protected owner display. The durable PairingRecord retains IDs/budget/state/audit, not a plaintext or hash manual-code verifier. On gateway startup, invalidate Available invitations whose volatile setup is lost and settle pending attempts with typed restart cause before serving status. Claimed/Approved/Staging recovery continues through durable proved-device receipts; these phases need no password-file setup. Active key credentials do not expire at the invitation deadline. Persisting manual setup/secrets is explicitly out of this minimal profile. A per-invitation setup disappears after claim/deny/expiry; global cleanup cannot silently resurrect it.

An attempt reservation consumes a count durably **before** invoking ServerLogin or returning any password-dependent result. Malformed admissible PAKE elements count as failed attempts; pre-parser frame/TLS capacity refusals are resource controls, not password-dependent verdicts. Retry of identical immutable attempt input can read its receipt, but never return a different KE2 for that same reservation or allow another KE1 to reuse it. A lost attempt connection gets a charged terminal failure; retry uses a fresh attempt while available. An admitted fifth correct attempt can settle before expiry despite count zero; no sixth starts. Attacker exhaustion remains denial of service, requiring a new owner invitation. Successful claim atomically closes future code use before approval.

## Initial trust and device possession: no circular assumption

Use TLS1.3 with mandatory raw-public-key client proof. Gateway and device long-term keys are Ed25519, encoded as the exact canonical RFC8410 44-byte SPKI (OID parameters absent, 32-byte public key); fingerprint is SHA256(SPKI), while the full SPKI is the equality/trust binding. [RFC7250](https://www.rfc-editor.org/info/rfc7250/) requires an external binding for raw-key identity. Here the gateway binding is established by the invitation PAKE and owner consent; later connections use the stored pin. ring's [Ed25519 key API](https://docs.rs/ring/0.17.14/ring/signature/struct.Ed25519KeyPair.html) supplies generation into PKCS8 v2 and validated reload. No new signing primitive is implemented.

For the **first manual connection only**, the client has no gateway pin. A dedicated enrollment-only rustls verifier may admit a well-formed allowed raw key provisionally, while still verifying the real TLS CertificateVerify signature and Finished. The gateway likewise verifies the device's mandatory actual CertificateVerify proof before making it available as trusted possession evidence. Neither preliminary key acceptance nor a displayed key string grants product authority. This provisional connection can reach only fixed pairing frames; it must not send existing credentials, private source/cache data or invoke product routes. The only client identity disclosed is its generated public key; create a fresh key per new pairing attempt sequence to avoid sharing an existing identity with an untrusted locator.

The manual PAKE uses the actual TLS-observed gateway and device keys and the **locally derived** exporter. Two attacker-terminated TLS channels have different exporters, so relaying KE1/KE2/KE3 between them fails context authentication. Substituting an attacker key also changes context. A byte-forwarding relay preserves the end-to-end handshake, learns no protected receipts, and controls only availability. An attacker who guesses or steals the invitation secret can present their own key; exact key-specific owner approval remains required. A successful ClientLogin.finish verifies the server's KE2 MAC and thereby authenticates the gateway/key/channel/context to the device. Persist the authenticated gateway pin and pending enrollment together with the device key **before sending KE3**; this fact establishes gateway trust, not a server claim or completed mutual confirmation. Server confirmation and claim still require verified KE3 and the durable claim transition. This order lets a client recover after the server commits but its reply is lost. QR can pin the supplied gateway key before TLS, still checking all PAKE/consent rules. Later connections compare the persisted SPKI strictly; gateway-key changes require explicit re-pairing.

rustls 0.23.45 actually provides [AlwaysResolvesServerRawPublicKeys](https://docs.rs/rustls/0.23.45/rustls/server/struct.AlwaysResolvesServerRawPublicKeys.html), [AlwaysResolvesClientRawPublicKeys](https://docs.rs/rustls/0.23.45/rustls/client/struct.AlwaysResolvesClientRawPublicKeys.html), verifier `requires_raw_public_keys()`, and [verify_tls13_signature_with_raw_key](https://docs.rs/rustls/0.23.45/rustls/crypto/fn.verify_tls13_signature_with_raw_key.html). Use that helper with the injected ring provider's supported algorithms and allow Ed25519 only. Never return an assertion marker in place of verifying a signature. Restrict configuration to TLS1.3, no 0-RTT, no resumption for this first profile, no key logging, mandatory client authentication, and raw-key resolvers/verifiers on both peers. Negotiate fixed application protocol `nessa-device-v1`; reject fallback to X509 or a different application protocol. Existing bearer/browser routes keep their own composition; provisional enrollment routing never supplies a product AuthContext.

The actual [export_keying_material API](https://docs.rs/rustls/0.23.45/rustls/server/struct.ServerConnection.html#method.export_keying_material) accepts owned output, label and context, and fails before handshake completion. Use [RFC9266](https://www.rfc-editor.org/rfc/rfc9266.html) `tls-exporter`: 32 bytes, label `EXPORTER-Channel-Binding`, empty context, after the completed handshake. Both endpoints derive their expected binding; a claimant's supplied exporter is never the expected value. This also avoids an invented exporter or encryption construction.

The native adapter's exact proof path is completed TLS1.3 CertificateVerify -> rustls raw-key helper -> rustls-webpki RawPublicKeyEntity parsing -> configured ring Ed25519 verification -> verified Finished -> connection-derived immutable proof context. The one auth-owned key representation holds exactly 32 Ed25519 key bytes; its boundary codec admits only the canonical 44-byte RFC8410 SPKI shape, with no trailing bytes, variable parameters, alternate key type or DER allocation chosen from wire lengths. Both parsers/encoders use that owner, not separately maintained validation constants. Frame byte bounds apply before parsing. Application code receives server-selected gateway audience, the actual gateway/device key bytes, completed-handshake binding and phase from this trusted adapter; wire DTOs cannot construct it through a `verified: true` field. One current device authentication contract consumes that transport-proved context plus the existing registry current-state reader. A connection-scoped verifier can retain the context without widening every bearer call. Ordinary browser WebSocket clients must not be assumed to support RFC7250 raw keys; this profile is for the native Rust example and does not replace local bearer/browser authentication.

Historical proposed resource arithmetic (superseded as a protected-read ceiling; final connection budget remains open): the single creation KSF worker reserves 64 MiB plus measured primitive/runtime overhead. Network ServerLogin does not perform the password KSF; each native example client reserves 64 MiB for its own login. Eight 128-KiB transport buffers reserve 1 MiB before measured TLS/handshake overhead; configure ingress/read and egress/write limits separately, since rustls's outbound `set_buffer_limit` alone does not bound ingress. The live-phase capacity is32; terminal cleanup obligations remain bounded by configured receipts/registry bytes, with owned restoration histories separately charged. Holding a network handshake permit never grants permission to run an extra KSF worker. Benchmarks must report measured peak resident/allocation memory and cancellation latency; these calculations are proposed budget accounting, not observed measurements.

## One canonical authenticated enrollment intent

The application-owned pairing wire codec defines a bounded fixed-order length-prefixed encoding once, consumed by gateway/example and vectors. Binary fields stay bytes; text fields use their existing owner's UTF8/identity limits. Each variable field has an unsigned 16-bit big-endian byte length; IDs are generated canonical values, not arbitrary text identity aliases. No JSON serialization or map iteration order becomes the signed/authenticated representation.

OPAQUE registration identifiers: client = canonical invitation identity; server = the full canonical gateway SPKI under the fixed pairing identity domain. These remain unchanged at login: the transient OPAQUE client's envelope key is not the permanent device key. OPAQUE credential_identifier is the unique invitation ID. Login context, in this exact order: domain `nessa-device-pairing-v1`, fixed suite identifier including explicit KSF parameters, mode, actual gateway SPKI, actual device SPKI, invitation ID, attempt ID, exclusive invitation deadline, immutable consent-intent ID, desired grant generation, fixed public intent class `gateway-conversation-read`, and RFC9266 exporter. The current fixed typed profile encodes 335 bytes. `public_native_context_preserves_profile_and_same_channel_binding` inspects contexts obtained from both actual TLS peers; `fixtures/context.hex` is a historical manually assembled profile example, not a live TLS exporter observation. This producer has no configurable context limit: the transport constructor selects the fixed parts, retains each u16 representation conversion and never truncates them.

The consent-intent ID is a fresh random 128-bit opaque identity allocated when the owner creates the invitation, never a hash of private metadata or an alias derived from owner/org/name/grant. Its immutable auth-owned record identifies the full gateway audience/resource, organization, owner principal/member linkage, exact canonical grant set and generation. The initial class has only conversation.read. Changing any full intent field creates a new consent-intent identity/generation through its owner and invalidates old activation, rather than changing what an existing ID means. PAKE authenticates this opaque reference; the authoritative immutable registry mapping and later protected disclosure enforce its relationship to full intent. This is deliberately not a claim of a portable cryptographic commitment to undisclosed metadata. No digest/password prehash/commitment primitive is added. Resource/action canonicalization remains with the existing auth owner.

Before PAKE confirmation, the wire may disclose only the protocol/suite/KSF/mode, random invitation and attempt IDs, expiry, both already exchanged public SPKIs, random consent-intent ID/generation and the fixed public read-intent class, plus native PAKE messages. Public here means explicitly admitted to unauthenticated enrollment peers who know a locator; gateway audience, organization, owner principal/member IDs and resource/grant selectors are **not** public. The exporter is locally derived and is not transmitted. Client sends invitation ID, attempt ID and KE1. Gateway reserves that exact attempt against the TLS-proved device key and authoritative invitation, then sends the bounded public context metadata with KE2. Each side builds its native OPAQUE identities/context from these fields and its own actual transport keys/exporter. Device rejects unknown mode/suite/class or mismatch with its locally selected key/IDs. Received metadata is only provisional until ClientLogin.finish authenticates KE2; gateway never takes the device's metadata as the authoritative expectation.

After verified KE3 and immutable claim commit, the gateway sends a TLS-protected ClaimReceipt binding invitation/attempt/gateway/device/consent-intent ID/generation/stage plus the full gateway audience/organization/resource/exact grant disclosure. Omit owner principal/member identifiers from claimant receipts: the auth record and owner approval surface retain that linkage, and the device does not need to know it. The device compares receipt identities to its authenticated pending record and verifies the public read class against the full grant, refusing product use on discrepancy. Any full grant outside the declared read class cannot be approved/activated by this profile. Owner consent names the full gateway/resource/organization, exact key fingerprint, exact grant and the same intent identity/generation. PAKE confirms invitation knowledge/channel binding, TLS proves key possession, and registry consent establishes authority. Private disclosure is sent only after the server confirms the client; client KE2 authentication alone does not let it request private invitation metadata.

After owner approval and staged activation, send the ActivationReceipt with credential/receiver identities and current canonical receiver epoch, retaining the same consent-intent identity/generation. No permanent private device key, server key, PAKE key or reusable bearer secret is transferred. Receipt IDs are lookup handles only; receipt/status resume requires fresh raw-key TLS possession on pinned gateway and exact expected device binding, restricted to status/cleanup when revoked. A copy of a public receipt cannot authenticate. TLS authenticates receipt delivery; this minimal design does not claim an independently verifiable offline signed receipt.

## Client persistence before KE3 and exact recovery order

The tagged [TripleDh.generate_ke3 implementation](https://github.com/facebook/opaque-ke/blob/v4.0.1/src/key_exchange/tripledh.rs) checks the server MAC before returning; ClientLogin.finish propagates that failure. Therefore successful client finish is sufficient to save the authenticated gateway pin. It does **not** establish delivery/acceptance of KE3, a claim, owner approval or activation. Distinguish those meanings rather than add independently writable `gateway_verified`/`ke3_sent` flags.

1. Generate the device key and begin provisional TLS/PAKE. Before a successful client finish, retain no newly trusted gateway pin. A crash here cannot have produced KE3, so no server claim can arise from this client. The existing server attempt deadline/close owner settles the reservation.
2. After valid KE2, atomically commit one client pending-enrollment record containing its durable private-key reference/bytes, authenticated full gateway SPKI, invitation and attempt IDs, consent-intent ID/generation/class, expiry and the exact authenticated public context metadata. The durable key and pending record must agree; a failed save closes without sending KE3 and consumes that attempt. Use the injected private storage owner; no raw handshake/session key is stored. The client may already have saved a fresh private key, but the atomic pending publication is the point after which it is eligible to send KE3.
3. Only after the successful durable save, send the in-memory native KE3 on its original TLS connection. Do not replay KE3 onto a new exporter or serialize PAKE state for crash recovery. Erase app-owned code/output keys after sending or failing; a retry may ask the user for the code again.
4. Gateway verifies KE3 using the retained exact attempt context, then asks the single durable invitation transition owner to claim. KE3 cryptographic success is not itself a claim if expiry/deny/close already won. Successful claim commits key/attempt/intent evidence before erasing its now-unused volatile setup or sending ClaimReceipt. The durable receipt survives reply loss/restart. Private ClaimReceipt disclosure follows only this committed claim.
5. Client receives and correlates the ClaimReceipt, or uses its saved pending record to open a fresh **strictly pinned** TLS connection and prove the same private device key for bounded attempt-status lookup. This lookup never submits code, reuses a charged attempt, synthesizes a claim or grants product access. Server compares the actual TLS-proved key against the immutable reservation's key for that exact invitation/attempt and reveals only that attempt's own status. Before claim it reveals only Pending or its charged terminal outcome, with no private full invitation/owner metadata. After claim it can return that key's protected ClaimReceipt; the same key may later query activation through the same enrollment status surface.

| Ordering | Durable server outcome and trusted recovery |
| --- | --- |
| Client pending save fails before KE3 | No KE3 is sent. Reservation closes/fails once. No new trusted pin/pending enrollment is published; no claim, credential or source work. |
| Pending save succeeds; client crashes before KE3 leaves | Same key/pin survives. Fresh status sees Pending while the original reservation can still finish; after close/deadline it sees the existing terminal failed attempt. Fresh attempt requires available budget and a newly entered code; it may keep the proved key for this invitation sequence. |
| KE3 packet lost before server receives it | Same behavior as the preceding row. Status cannot declare final NotClaimed while the pending attempt can still accept KE3. It reports Pending without private intent disclosure; close/deadline orders failure. |
| KE3 verification ready races original close/deadline | Single invitation transition owner orders them. If claim commits first, status is Claimed; if terminal close/expiry commits first, completion refuses. A status read returns one coherent Pending/terminal snapshot and creates neither cause. |
| Server commits claim; ClaimReceipt is lost; client crashes | Saved authenticated pin/key supports fresh status. Exact claim receipt is returned without PAKE rerun, code reuse, second attempt charge or second credential/receiver. |
| Server commits claim then restarts before receipt | Durable claim/intent evidence survives; missing volatile PAKE setup is irrelevant. Key-proved status returns the same receipt and canonical activation stage. |
| Server restarts before claim commit | Startup settles pending reservations/invalidation before serving pairing status. Fresh pinned status returns charged terminal restart failure; it cannot resume missing PAKE state or accept old KE3. A new owner invitation is needed under the volatile-setup policy. |
| Another proved device wins claim first | This attempt gets its superseded terminal result, without the winning device's private intent/receipt. Its saved pin is authenticated gateway trust but never a device grant. |
| Client loses/corrupts its key or pin despite prior save | No recovery proof is available. Refuse implicit new-key claim/status or trust update; owner cancels/revokes and creates a new invitation. Crash-safe storage, not protocol reconstruction, is the enforcer. |

The pending client record can be replaced only by a correlated canonical claim/activation/terminal receipt from the strict pinned connection; its existence is not local authority to read product data. A timeout leaves it pending. A terminal not-claimed receipt permits a fresh charged attempt only if the invitation is still available, before expiry and within budget. The server's key-bound attempt receipt supplies status admission; no additional independently mutable recovery authorization flag is necessary. Preserve the first terminal cause and original actor when later close/cleanup/status events join it; a diagnostic or receipt read cannot rewrite the cause. Existing bounds apply to preclaim status, and denied/revoked keys get only their correlated terminal/cleanup evidence. An attacker knowing public IDs but holding another key gets no private details.

The canonical auth owner validates the full grant against its bound public class and immutable intent identity before creation, protected disclosure, owner approval and activation. A discrepant disclosure is refused as an owner/representation inconsistency, not repaired by changing the intent under the same ID. The device independently consumes that same canonical published representation and checks correlation/class before using it. No separate `client_acknowledged_scope` flag or hidden additional consent round is introduced: the owner's explicit approval of the exact claimed key and canonical intent remains the grant authority; the profile does not claim a durable proof that an offline client received the disclosure.

## Auth ownership, conditional publication and issue265 boundary

Confirmed seam: `application/ports.rs::CredentialVerifier::verify` accepts only CredentialEvidence and server AudienceId. LocalCredentialStore parses a bearer token, SHA256-checks its stored verifier, and rejects revoked/wrong audience. It cannot verify a caller nonce/exporter without trusted context. Extend the **consuming auth verification boundary** with a typed transport-owned proved key/channel context, or inject a connection-scoped key verifier carrying that context; a proof-only registry lookup is not verification. Keep raw TLS library types in the adapter. AuthenticateSession must remain the owner constructing AuthContext after coherent current credential/member reads. Current AccessReader/Cedar continues authority on each admission.

Registry publication proposal is sound only with missing capabilities implemented: one durable PairingRecord owns attempt/claim/consent/staging, typed key binding belongs to the existing credential store, and credential eligibility is derived from active enrollment stage. Current registry has bearer verifier representation only. Do not put a parallel key-grant store next to it. Receiver pair/ensure/resume/fence stays with the canonical receiver authority. Reserve credential identity durably, ensure receiver once with an idempotent correlation, then publish usable key credential and Active receipt in one auth commit. Partially staged receivers cannot yield data because authentication has no active credential; every partial-state source-call count must remain zero.

Current public issue path cannot be reused unchanged: `issue_internal(false)` rejects Admin membership and conflicting existing membership data. A self-device read key must preserve the owner's current membership rather than downgrade it or create a second membership. `validate_issue` checks issuer existence, not current delegation. Use a dedicated registry-owned enrollment command with canonical current credential.manage authorization and exact approved read intent; no new self-enrollment action is needed for the initial owner/admin example.

Fresh final authorization can evaluate the existing current-session/Cedar owner against a coherent snapshot, producing exact target/grants, expected registry revision and policy identity. Final mutation CAS verifies revision, invitation generation/stage, expiry and record linkage; changed revision requires fresh canonical authorization. Storage checks relationships/CAS, **not a copy of Cedar policy**. Do not hold registry mutex during async authorization. Policy identity is fixed by composition for this first profile; don't add a general hot-policy subsystem. CAS contention is bounded and returns a typed retry/status outcome.

Issue265 owns direct TLS listener, transport bounds/reconnect/replay and secure request/response framing. This recommendation specifies its required pairing handshake dependency and exporter/key-proof output; it does not claim that listener exists or select a competing transport elsewhere. Adopt this profile jointly so pairing and later sessions use the same key bytes/pins. Once active, strict raw-key verification maps only to active registry credential; every request reads current exact grant and receiver epoch, and binds operation correlation/channel/nonce as issue265 requires. TLS does not replace core namespace/epoch/correlation validators. Revocation/narrowing fences old receiver and credential authority; affected-cache purge waits for authenticated bound contact evidence. No timeout, offline failure or generic TLS/auth error authorizes cache destruction. Already admitted bounded responses can finish under the existing admission contract.

## Transition owner and implementation map

The auth PairingRecord is the one durable owner of invitation, attempt receipts, immutable consent intent, key claim, approval, reserved credential, receiver staging and cleanup obligation. Current credential validity/member linkage/Cedar permission belongs to existing auth owners. Receiver identity/epoch/effect legality belongs to the canonical receiver transition owner. TLS/PAKE adapter proves the key/channel/invitation; it grants no product authority. Client storage publishes its key/pin/pending identity atomically before KE3. Application orchestration coordinates these owners without a second approval flag or epoch counter.

| Path | Owns |
| --- | --- |
| `crates/nessa-auth/src/domain/pairing/` | Pure immutable constrained values, PairingRecord decisions, causal transitions and replay |
| `crates/nessa-auth/src/application/pairing/` | Current authorization/conditional-commit orchestration and caller-owned effect ports |
| `crates/nessa-auth/src/adapters/local/registry/pairing/` | Pairing representation and atomic persistence in the existing registry authority |
| `crates/nessa-auth/src/adapters/pairing/` | The selected OPAQUE implementation and canonical bounded crypto/wire conversion and retained private key/pending codec |
| `crates/nessa-server/src/device_pairing/` | Native bounded transport adapter, owner commands and scoped status dispatch |
| `crates/nessa-server/src/conversation/infrastructure/receiver_authority.rs` | Canonical receiver ensure/resume/fence, correlated once-only effects |
| `crates/nessa-server/examples/` | Thin consuming native Rust client composition consuming the injected private pending port |

Each source layer gains a module map when implemented; tests use matching feature/layer directories. No extra package is added just to share wire types. Exact record-read route wiring waits for the issue296 source owner; the native proof and scoped admission boundary does not invent a record source.

## Complete event and ordering table

Manual entry uses one current contract: gateway address plus the eight-symbol secret code. Each gateway resource has **one Available manual-code invitation**. Provisional Hello advertises only that slot's public invitation ID, expiry, immutable opaque intent ID/generation and fixed read class. It discloses neither password nor a hash and never tries a request against multiple password files. Claimed/Approved/Staging enrollments may continue independently within the 32-live bound. Slot ownership is the canonical Available phase; attempt reservation does not release it. Only confirmed claim or an explicit canonical terminal transition releases the slot. Creating another invitation never implicitly expires/cancels/replaces its predecessor. A QR may display the same public ID/pin but creates no second manual-entry contract.

States are Available (attempts may exist), Claimed, Approved, Staging, Active, and Terminal. Terminal cause retains Denied, Cancelled, Expired or Restarted with its original actor. Terminal after staging retains cleanup obligation/effect evidence until the exact receiver is fenced; physical results arriving later do not revive activation. Attempt receipts have Pending, Failed, Claimed or Superseded outcomes. Failed retains its typed InvalidProof, ConnectionClosed, HandshakeDeadline or VerifierUnavailable cause; a diagnostic string does not choose one. Their original admission identity, proved key, intent/generation and immutable input fingerprint remain retained. Approval is a state transition bearing its actor and exact target, not a boolean. Activation/replacement uses the desired generation. Events carrying external outcomes must refer to an earlier owned dispatch/target before they can change state.

The table's `test` column names required test cases. These are planned enforcers until the implementation/check report records them as passing. Tests run the same decision function for live transition and restored history, with accepted counterparts for rejected combinations.

| ID | Event / ordering | Owning decision and effect | Test |
| --- | --- | --- | --- |
| P01 | Create from a fresh owner session | Canonical Cedar credential.manage, exact read intent, registry revision CAS, expiry/budgets and audit commit before one-time code display | `create_requires_current_owner` |
| P02 | New attempt | Decode bounded KE1 and fingerprint its canonical library representation, then reserve immutable attempt/device/key/context fingerprint durably and charge before ServerLogin; one pending attempt at a time | `reserve_before_crypto` |
| P03 | Identical reserve retry / conflicting retry | Existing exact receipt/status; another payload or key refuses without crypto | `attempt_retry_is_immutable` |
| P04 | Wrong or malformed admitted proof | Failed charged receipt, no claim/credential/source | `failed_attempt_has_no_authority` |
| P05 | Fifth attempt pending / sixth reservation | Fifth remains eligible; sixth refuses; correct fifth before expiry can win | `last_attempt_remains_eligible` |
| P06 | Correct KE3 versus expiry/deny/cancel; Denied or Cancelled requested by another actor | First durable terminal/claim transition orders winner; later completion cannot replace terminal cause. Denied and Cancelled ask the original owner relation before deciding | `completion_vs_terminal`; `denied_and_cancelled_require_original_owner` refuses otherwise-valid System requests and accepts exact owner with original cause/actor |
| P06 direct expiry | Direct Expired event before and at its deadline | End(Expired) asks the existing expiry_due owner even when invoked directly: Available/System/time1005 refuses Invalid; time1100 accepts while the original stays unchanged | `direct_expiry_requires_due_time` |
| P07 | Two correct device claims | First exact key wins; other attempt superseded; no replace/second credential | `competing_claims` |
| P08 | Claimed device retry/status / code reuse | Fresh same-key TLS possession reads receipt; code cannot reopen an invite or create a grant | `claim_status_requires_key` |
| P09 | KE2 succeeds / client pending save fails | Authenticated server evidence not delivered KE3; zero KE3 writes, no claim | `pending_save_before_ke3` |
| P10 | Client saved pending / KE3 lost or client crash | Fresh pinned same-key status is Pending until original attempt settles; never final NotClaimed while KE3 can win | `lost_ke3_status` |
| P11 | KE3 completion and close ready together | One durable decision; retain original first cause; Pending read cannot create a cancellation | `same_ready_completion_and_close` |
| P12 | Claim committed / reply lost / client or server restart | Saved key/pin and durable claim recover same receipt without code/attempt recharge or duplicate receiver | `lost_claim_receipt` |
| P13 | Server restart before claim | Volatile PAKE setup loss settles attempt and invalidates available invitation before serving status | `restart_before_claim` |
| P14 | Client key/pin lost or corrupt | No fabricated recovery identity; typed failure and owner re-pair | `lost_private_key` |
| P15 | Provisional wire metadata | Public class/opaque intent identity only; private owner/org/resource grants absent | `provisional_metadata_is_public` |
| P16 | Full intent differs from bound ID/class/generation | Canonical auth owner requires the full intent resource to equal the composition-resolved gateway, then refuses mismatched disclosure/approval/activation before effects; no in-place change under an intent ID | `intent_disclosure_matches_class` |
| P17 | Owner approval | Fresh current authorization for exact claimed key/intent/generation; preserves current admin membership | `approve_exact_claim` |
| P18 | Owner revoked/member changed after approval | Final fresh Cedar result/revision CAS refuses stale issuance; cleanup retained | `approval_freshness_at_activation` |
| P19 | Begin activation | Save exact reserved credential and receiver dispatch identity before effect; credential stays unusable | `stage_before_receiver_effect` |
| P20 | Receiver effect succeeds / registry activation succeeds | Canonical original receiver evidence, coherent current receiver admission and final auth commit publish key credential/Active receipt together | `conditional_publication` |
| P21 | Receiver effect returns unrelated key/target/generation | Reject before replacement/publication; retain owned cleanup target | `receiver_result_correlates`; `cleanup_result_must_match_owned_dispatch_before_registry_completion` |
| P22 | Receiver response lost or restart after effect | Resolve exact earlier correlation/credential/receiver; ensure never dispatches a second receiver blindly | `receiver_resume_once` |
| P23 | Cancel during staging / delayed receiver success | Terminal cause remains; save late physical result and fence exact receiver before cleanup complete | `late_receiver_cleanup` |
| P24 | Generation replacement / old activation completion | Old generation cannot activate new intent; fence retained old stage; restart uses desired generation | `old_generation_cannot_activate` |
| P25 | Active response lost | Fresh key-proved status returns current same credential/receiver/epoch; no new issuance | `lost_activation_receipt` |
| P26 | Revoke/narrow | Commit intent, fence old receiver/epoch, revoke current key credential, retain partial outcomes; later read denies | `revoke_and_narrow` |
| P27 | Cleanup failure / audit failure after other owner commit | Preserve actual before/after/cause/actor and pending effect obligation, never fictional rollback | `cleanup_failure_is_retained` |
| P28 | Registry persistence/audit fails before commit | No authority publication or success; caller gets typed failure | `failed_commit_has_no_publication` |
| P29 | Trusted revoke contact versus old reply | Bound gateway/device/namespace/generation marks cache unusable and purges affected namespace; old reply cannot advance | `trusted_contact_purge` |
| P30 | Timeout/offline/generic auth denial | No trusted wipe cause; pending remains pending, offline cache remains per accepted policy | `untrusted_failure_preserves_cache` |
| P31 | Real TLS wrong key/proof / PAKE tamper / exporter substitution | Actual cryptographic rejection before claim/product dispatch; forwarding relay succeeds, terminating MITM fails | `native_proof_rejects_mitm` |
| P32 | Worker cancellation / slow or oversized frames | Ingress/egress/parser/worker permits bounded until physical release; close controls retain capacity | `physical_capacity_retained` |
| P33 | Restore contradictory evidence / valid counterpart | Same transition replay rejects cause/actor/key/intent/target/stage contradictions without repairing stored bytes | `pairing_history_agrees` |
| P01 policy refusal | Current valid owner session; manage or proposed-read policy denies, or its authority is unavailable | `AuthorizePairing` retains `Denied` separately from proof invalidity and `Unavailable`; no admission or registry change | `pairing_admission_current_access_policy_revision_and_deadline_neighbors` checks exact typed cases, current-session validity and the original allowed counterpart |
| P26 active cancellation | Owner cancels Active at millisecond110001; same current command is retried | Original atomic commit retains Cancelled plus Explicit revocation by that owner at second110; current-admission retry preserves the first evidence | `active_cancellation_preserves_original_revocation_and_retry` observes public commands, persisted bytes and reopened terminal record |
| P33 cancelled actor disagreement | Otherwise-valid canonical revocation uses LocalOperator or an existing different principal | Pairing transition agreement refuses the contradiction without replacing stored bytes | `public_reopen_requires_original_cancellation_actor` restores the exact cancelled original as its accepted counterpart |
| P33 cancelled time disagreement | Canonical Explicit command/after-state/metadata coherently advance to a different valid second | Pairing transition agreement requires original cancellation milliseconds / 1000 | `public_reopen_requires_original_cancellation_time` rejects coherent changed bytes and reopens the exact original |
| P33 cancelled cause disagreement | Valid Superseded revocation refers to an actual separately issued credential at that issuance revision | Pairing transition agreement requires Explicit for owner Cancel; independent canonical revocation keeps its original meaning | `public_reopen_requires_original_cancellation_cause`; `cancellation_revocation_matches_original_command` also isolates cause, target, actor and time through public domain values |
| P26 unpublished cancellation / independent revocation | Cancel before Active, or consume the original canonical credential-revocation event | No published-cancellation revocation is invented; canonical bridge retains its independent original cause and actor | `cancellation_revocation_matches_original_command`, `cancelled_stage_keeps_identity_reserved_after_restart`, `native_key_publication_and_canonical_explicit_revoke_agree` |
| P01 denial consumption | Typed policy denial reaches existing passive admission or browser invalidation mapping | Passive refusal is Forbidden before source access; browser invalidation has no removal cause | `passive_admission_preserves_unverifiable_authority_failures` and `passive_socket_preserves_retryable_authority_failures` include Denied; `every_automatic_invalidation_retains_its_typed_cause_without_an_initiator` checks the public mapping and a separately retained/reopened journal session, not an enrollment HTTP pipeline |
| P01 socket admission denial | Current identity or browser-presence authority reports Denied before record/catalogue parameters and source routing; the next read recovers | Socket consumes the same exhaustive ReadRefusal-to-record-wire mapping as record dispatch, preserving Forbidden independently of Unauthorized and Unverifiable | `passive_socket_preserves_retryable_authority_failures` covers the actual five passive methods and recovered response; the first 5a789 FIRST failed with unauthorized instead of forbidden and is retained as historical evidence |

A new ordering goes into this table before its implementation. Source-call counters remain zero for every unapproved/partially staged/refused state. Current per-request authorization retains the existing semantics that already admitted bounded work may finish after later revocation. A successful snapshot decision is not reused for future requests.

## Implementation acceptance and verification

Issue264 is complete only with real owner create/code/approve/deny/status/revoke commands, actual registry/Cedar key credential publication, one-use expiry/budget enforcement, exact receiver staging/fencing, and the native Rust client pairing against a separate gateway process using this selected crypto profile. Pure transition tests alone do not meet that finish line. The coupled issue265 proof/channel boundary is authorized here; sync record route and cache integration consumes their canonical owners rather than duplicating them.

Run focused formatting, Clippy, Rustdoc and tests with `CARGO_TARGET_DIR=target/pairing CARGO_BUILD_JOBS=2` while other tasks retain build capacity. Record actual lockfile/transitive MSRV, supported platform compilation, official OPAQUE vectors and native two-process adversarial cases. Break each touched rule in a temporary revert probe, print/confirm the mutation, prove its enforcing test fails, then restore before frozen verification. No independent audit of opaque-ke4.0.1 or this application profile is claimed. Hardware/OS/device compromise, invitation theft, online exhaustion and offline cache recall remain the stated limits.

### Current registry representation

The existing `verifier` member is one current proof sum: a bearer verifier string or an object `{ "kind": "devicePairing", "invitation": [16 bytes], "generation": n }`. The object references the exact enrollment; the enrollment owns its claimed key. No empty bearer verifier or proof flags accompany a key binding. The current registry has an optional empty `pairings` collection; absence means no enrollments. Registry schema and existing bearer identities stay current. Reopen validates every enrollment by replay and every key credential against its exact active enrollment, generation, and credential ID.

`StoredPairing::restore` refuses external history above `MAX_HISTORY_STEPS` (32) before replay; the existing registry byte bound applies before decoding. Live append asks this restore owner, then correlates privately minted transition evidence with its original before snapshot. The current validated state graph permits at most 16 legal steps: five Reserve events plus four Fail and one Claim produce at most ten; approval, staging, one receiver result, activation, one terminal decision and one cleanup add at most six. Terminal cannot return or end again; approval, stage, receiver and cleanup cannot repeat. This is provenance of the current private factory, not another publication limit. Exact duplicate application delivery returns its existing outcome without appending a step.

| Registry input / order | Owner and result | Evidence |
| --- | --- | --- |
| External persisted history exceeds MAX_HISTORY_STEPS | Restore refuses Capacity before replay; append consumes that refusal from the same owner | `public_reopen_validates_original_consent_and_history_bound` edits acknowledged bytes to raw33-step history, observes public reopen refusal, and restores the original Approved record |
| A valid privately minted transition follows restored current history | Restore and original before correlation govern append; omit the unreachable copied append>=32 predicate | Public domain `pairing_history_agrees` accepts each transition against its original before state and refuses stale before; the validated factory/state graph above supplies the private append bound. Public registry reopen separately restores original Approved history |

### Canonical credential revocation and enrollment agreement

The credential transition journal owns revocation meaning. A linked enrollment consumes that exact canonical transition, including credential ID, before/after lifecycle, revocation cause, and Principal or LocalOperator initiator. Its stored pairing event references the canonical transition sequence; it does not reconstruct authority from `revoked_at` or keep an independent revocation flag. Reopen resolves this reference through the same journal and replays the same domain event. Existing Admin membership remains unchanged.

| Row | Existing state / event | Ordered result | Enforcing evidence |
| --- | --- | --- | --- |
| P34 | Active device credential; explicit principal `credential.revoke` | Credential revocation and linked Terminal(CredentialRevoked) commit together; original Principal and Explicit cause remain canonical; receiver cleanup stays pending | explicit revoke identity/cause, atomic failed-write test, reopen |
| P35 | LocalOperator owner recovery on the current registry | Recovery replaces credentials carrying `credential.manage`; an exact read-only device credential remains Active. If a canonical LocalOperator revocation actually targets a key binding, the bridge preserves its actual cause/initiator | real recovery scope, Admin membership unchanged, reopen; pure canonical LocalOperator bridge replay |
| P36 | Same-second enrollment create/publication/revoke | Canonical seconds remain unchanged. Pairing `orderingTimeMs` is explicitly a **derived lower bound** `max(checked(seconds * 1000), createdAtMs)` for this bridge; it is not an observed millisecond timestamp | same-second replay, checked multiplication overflow |
| P37 | Repeated canonical revoke or reopen; final publication competes | No second credential transition or enrollment cause; stale Active publication fails current revision/phase; receiver fence failure keeps exact cleanup target and original evidence | idempotent retry, stale CAS, cleanup retry/reopen |

Ordinary pairing transitions use clock-observed milliseconds in `orderingTimeMs`. The bridge retains the canonical second-resolution event through its journal reference. Receiver cleanup remains an independent physical effect with a durable obligation; failure does not erase either committed history.

### Canonical receiver retries and physical cleanup

| Row | Receiver event | Result and evidence | Enforcing evidence |
| --- | --- | --- | --- |
| P38 | Exact repeat of pair/change with the same actual initiator and request correlation | Return the original canonical transition outcome. Changed credential/org/owner/receiver/cause under that correlation is a conflict; no second epoch/event | pair and revoke retries, changed-target collision, restart |
| P39 | Runtime cleanup of a Terminal enrollment, exact reserved credential and receiver | The canonical receiver accepts a System Revoke only for that exact currently active binding. A receipt retry returns the original fence; changed/regranted binding conflicts. Enrollment retains original terminal cause/initiator and cleanup obligation until physical confirmation is committed | exact-key fence, late receiver result, failed fence and retry, canonical before/after/System event |

The server pairing application owns a non-deserializable `CleanupAdmission` with private fields and constructor. Its factory reads the validated canonical Terminal enrollment through `PairingStore`, including the exact staged credential, claimed device key, immutable intent generation, org/principal, original receiver/epoch and stage correlation; it then reads the canonical receiver owner through an injected cleanup port. A stage cancelled before credential publication is proved by its retained canonical credential reservation and key claim, not by inventing an issued credential. For an issued key credential, registry restoration already enforces its exact enrollment/generation linkage. Product and transport DTOs cannot supply an admission or a verified flag.

| Row | Ordering | Required outcome | Enforcing evidence |
| --- | --- | --- | --- |
| P40 | Nonterminal/wrong credential, receiver, org, principal or older epoch at cleanup admission | Refuse before SQL write; exact Terminal reservation plus key claim is required | `cleanup_admission_rejects_unrelated_evidence` |
| P41 | Same key credential retained across policy epoch; cleanup admitted then epoch changes | Factory may admit current newer policy epoch; SQL rechecks its exact captured binding before fence. A competing epoch causes retry from canonical evidence | `cleanup_epoch_recheck` |
| P42 | Receiver already fenced, policy subsequently advances inactive epoch, or receiver regranted another credential | Return the original matching canonical fence receipt; preserve its original before/after/cause/initiator. No System relabel or fence of replacement credential | `settled_cleanup_keeps_original_receipt` |
| P43 | Fence commits and reply is lost, failure before commit, gateway restart and repeated cleanup | Same stable correlation returns exact original receipt. Failed write keeps both journals and cleanup obligation; reopen/retry settles once without another epoch | `cleanup_lost_receipt_and_restart` |
| P44 | Registration worker waiter dropped/cancelled, entropy fill panics, or blocking task fails before return | The sole permit moves into the actual closure and stays until completion/unwind. Panic and cancellation retain distinct typed unexpected-worker facts; cryptographic refusal is separate. No volatile setup or durable invitation/credential is published before verified return; valid retry after physical release succeeds | `cancelled_registration_keeps_physical_capacity_until_release`; `entropy_panic_has_typed_fault_and_valid_retry` |
| P45 | Competing creates, charged attempt on Available slot, prior claim/terminal then new create | Fresh-authority creation into occupied gateway slot returns typed AvailableSlotOccupied, preserving original history; stale policy/admission still refuses at its current owner. Reservation retains slot, claim/terminal releases it. Prior code/context cannot claim the replacement | `available_manual_slot_is_canonical` |
| P46 | Receiver revoke before coherent current read; policy epoch change; revoke after read before registry publication | Retain original physical receipt. Before publication, current SQL evidence must be active for the exact staged credential/receiver/org/owner, at an epoch at least the saved original and with no intervening fence of that credential; a replaced or regranted credential refuses. Ask fresh Cedar for the exact immutable gateway read intent, then registry revision CAS. Revocation before the read prevents Active. Revocation after admitted read may leave historical enrollment completion, but all later operations/status UI/restart derive authorization from current SQL and current auth, not Active alone | `activation_current_receiver_admission`; real combined-owner restart/revoke tests |
| P47 | Owner command prepares/creates or reads/decides; device status before/after claim | Preparation derives owner/member/org/audience from the authenticated session and fixed composition gateway, asks current Cedar, then collection owner; registration output cannot display a code before exact registry CAS. Owner reads/decisions ask fresh exact-intent admission. Device status requires actual TLS possession and exact admitted attempt: Pending does not settle work; preclaim failure/terminal discloses no private intent; only the same claimed key gets protected full intent/history. Active remains historical completion; current product access is checked independently | `owner_api_derives_scope_and_private_status`; `create_requires_current_owner`; actual runtime/client acceptance |
| P48 | Terminal after durable Stage, original dispatch pending/drained/restart; exact receipt present/absent/unavailable | Stop excludes a second runner; retain bounded original dispatch ownership until physical drain. Never start a new Pair after Terminal to discover history. Lookup-only exact Principal/Pair/correlation receipt after drain: present => retain actual receiver then canonical fence; absent => private no-effect proof tied to durable Stage/key/credential/operation/drained owner may complete cleanup without inventing receiver; unavailable/uncertain or live dispatch => Terminal cleanup stays pending with first cause/initiator. A late receiver result after proved no-effect cleanup contradicts the drained receipt and refuses. Restart establishes exclusive old-process dispatch exclusion before lookup | `caller_loss_retains_actual_sql_worker_and_terminal_stops_new_effect`; `stage_drain_absence_preserves_original_terminal_and_reopen`; `stage_original_receipt_loss_then_terminal_recovery_fences_once`; `stage_lookup_failure_and_wrong_receipt_never_complete_terminal`; `stage_no_effect_proof_is_bound_to_exact_registry_gate`; `stage_worker_panic_retains_lease_until_unwind_and_valid_retry` |

P48 physical exclusion is owned by the existing canonical registry handle. A
bounded per-invitation stage permit is non-cloneable and retains an `Arc` to that
handle, including its lifetime filesystem lock. The permit moves into the physical
receiver closure; caller loss does not release it. Only actual closure completion
or unwind releases it. A second coordinator using that same handle cannot acquire
the permit while the original dispatch is live; reopening the registry cannot
succeed while that old handle remains physically retained. No enrollment approval
or cleanup state is inferred from this resource permit.

| P48 ownership ordering | Admission / result | Enforcement |
| --- | --- | --- |
| Canonical Stage with claim; no physical owner | Acquire sole permit, re-read exact canonical Stage before new SQL effect | Registry stage lease + receiver transaction |
| Original closure pending, even if caller dropped or Terminal committed | Acquisition refuses capacity; retain pending cleanup | Permit lives in physical closure |
| Terminal with drained owner or reopened exclusive registry | Acquire sole permit; lookup only original exact Pair receipt | Private resolution factory holds permit through lookup/commit |
| Wrong canonical registry owner, key, stage, generation or returned lease | Refuse before cleanup commit; matching public fields do not replace gate identity | Private gate identity and canonical record correlation |
| Exact original receipt absent under that permit | Private no-effect proof; canonical System completion retains original terminal cause/initiator | Registry verifies proof identity/generation/key/stage then appends no-receiver cleanup |
| Exact receipt present | Retain original outcome and fence via existing CleanupAdmission | Original receipt owner; no second Pair |
| Fence closure pending after caller/composition loss | The same stage lease moves with private CleanupAdmission in owned CleanupDispatch. Repeated cleanup remains Capacity and registry reopen stays Locked until actual fence drain; then lookup the original durable fence and preserve first terminal cause/initiator | `fence_caller_loss_keeps_original_lease_through_physical_drain_and_restart` |
| Physical stage/lookup worker panics or is cancelled by runtime | Typed unexpected worker fault; its closure retains permit until actual unwind/drop. Publish neither receiver nor no-effect proof; unchanged canonical obligation admits valid retry | Physical closure owns StageOwnership; shared pairing worker fault mapping |
| Lookup fails, disagrees or caller drops | No no-effect proof/commit; cleanup remains pending | Typed refusal, permit release after actual lookup ends |


Active means enrollment completed at its admitted authority snapshot; it is not a guarantee that the key remains currently authorized. There is no transaction spanning auth registry and SQL. Registry revision CAS protects current auth mutations, while current receiver reads protect receiver operations. A policy-only epoch advance may retain the exact stage linkage; a prior fence cannot revive that original stage even if another regrant later uses the same credential ID. SQL's receiver binding owns credential/org/principal/epoch, not a resource or grant class: those stay in the immutable canonical ConsentIntent and fresh current Cedar admission. The receiver's original Pair receipt remains historical evidence and is not replaced by its newer policy epoch.

`LocalReceiverAuthority` alone performs the physical fence. It accepts only the private factory admission, rechecks receiver/credential/org/principal/current epoch inside its Immediate transaction, and asks the canonical receiver transition owner. Existing public principal change methods cannot request a System transition. The cleanup correlation derives from immutable invitation/generation/stage identity and is bounded by the receiver request contract. Registry cleanup completion consumes the actual original fence result; no cleanup boolean substitutes for physical evidence.

Physical cleanup is a runtime operation, recorded as System in the receiver journal. Its stable request correlation links it to the enrollment's original terminal evidence, including any canonical credential transition reference. It must not label runtime expiry/restart cleanup as a human command.

### Registry identity and publication agreement (A review correction)

The registry consistency owner validates these relationships on every durable
write and reopen. Early command admission asks the same identity owner before
creating a secret. Retained Stage history keeps its credential identity after
cancellation, expiry and cleanup; this revision has no history-compaction or
identity-recycling operation.

| Ordering / neighbor | Existing facts and required result | Source owner |
| --- | --- | --- |
| Stage before ordinary issuance, surface provisioning or owner recovery | An issued credential or any retained Stage reserves its identity. Unrelated issuance refuses before secret creation; exact Stage retry retains its original pairing. A distinct identity remains eligible | registry identity availability; stage/issue/recovery admission |
| Cancelled / expired never-activated Stage | Keep the reservation even after no-receiver or receiver cleanup. No issued credential may occupy that identity | registry write/reopen consistency |
| Activation and original issuance, in either link direction | The retained Stage identity names exactly one DevicePairing proof for this invitation/generation. Original Issued cause is DevicePairing, original actor agrees with Activate, and issued-at/transition time equal Activate milliseconds floored to seconds. Missing credential, Bearer substitution or foreign issuance evidence refuses without repair | StoredPairing publication agreement and registry write/reopen consistency |
| Active or formerly Active Terminal, including same-second revocation | Preserve original activation/issuance correlation while the existing canonical revocation owner verifies later termination. No new chronology or finer timestamp is inferred | existing replay and credential transition validators |
| Unix live name replacement / Windows live name pinning | Unix replacement invalidates the old lock/file binding. Windows refuses replacement of the mutation-capable lock and reconciliation uses the original Published object; restart reconciliation starts only after that publication handle is dropped | Public private-state retry/reopen fixtures and the shared private-storage handle owner; direct original-handle failure evidence is not supplied here |

### Current A consumer evidence

These rows exercise the current auth registry and application ports. They supply
no SQL dispatch, listener, product source or protected framing acceptance.

| A ordering | Required retained relationship | Auth fixture |
| --- | --- | --- |
| Unpublished Stage reaches expiry; absent original receipt under the original lease; reopen | Terminal Expired and System no-receiver completion retain original stage identity. Identity remains reserved after cleanup/reopen, distinct issuance remains eligible | `expired_stages_keep_reserved_identity_and_published_activation_is_not_expired` |
| Published Active reaches invitation expiry | Current conditional expiry returns unchanged Active; original issued key remains bound on reopen. No unpublished cleanup proof is inferred | Same fixture |
| Lookup returns ownership from a different real registry with otherwise equal public fields | Gate identity refuses resolution before cleanup; actual original lease retained by lookup still excludes another stage/reopen until explicitly dropped | `returned_stage_ownership_and_receipts_must_match_original_registry` |
| Original ownership returns wrong credential/request/generation receipt; unavailable lookup; matching receipt or coherent absence | Wrong/unavailable results leave canonical bytes and terminal obligation unchanged. Exact result returns only the application resolution; coherent absence privately holds exclusion through registry no-receiver commit | Same fixture and `absent_lookup_and_caller_loss_retain_original_stage_without_invented_effect` |
| Stage is still nonterminal when cleanup resolution is requested | Refuse before receipt lookup; original canonical bytes and stage obligation remain unchanged. Later actual cancellation admits the original cleanup path | `absent_lookup_and_caller_loss_retain_original_stage_without_invented_effect` |
| Waiting caller drops while substitute lookup retains original StageOwnership | No completion is inferred; actual held Arc/gate still owns registry lifetime lock. Ending that lookup owner releases the original gate, allowing actual absent lookup retry | Same fixture; this is A port lifetime evidence, not a SQL worker claim |
| Current access unavailable/stale; manage or read policy refuses; valid current Cedar neighbor | Refuse before new canonical effect. Both actual actions are evaluated with coherent snapshot; accepted delegate uses real current registry + Cedar | `pairing_admission_current_access_policy_revision_and_deadline_neighbors` |
| Admission minted, then registry revision advances or original proof deadline arrives before commit | Mutation owner refuses unchanged bytes against original revision/deadline. Fresh/current and before-deadline counterpart commits | Same fixture |
| Otherwise valid receiver outcome has epoch zero or generation zero independently; both one | ReceiverOutcome constructor refuses Invalid before the registry can consume it; positive neighbor preserves all five original fields | `receiver_outcome_refuses_each_zero_bound_and_accepts_positive_neighbor` |
| Current Cedar authorizes another opaque consent ID for the same original issuer, membership, gateway and generation; command targets the original claimed/approved/receiver-bound invitation | Current revision/deadline remain valid. Original invitation comparison refuses Conflict before decision, Stage or Activate/credential publication; freshly admitted original consent commits each counterpart and survives reopen | `authorized_pairing_commands_refuse_otherwise_valid_foreign_intent`; real registry API, no private-state construction |
| Original Stage retry uses freshly minted original admission but commit clock precedes the original issuer issuance; current-time counterpart | Current issuer validity refuses Invalid before idempotent Stage return, even though revision and proof deadline still agree; unchanged current-time retry preserves original stage and bytes | `stage_retry_checks_current_issuer_time_before_existing_result`; later transition validation does not run for the exact existing Stage |
| Actual valid KE1 is retained unchanged, then copied and padded to the crypto-message limit plus one | Fingerprint decoder refuses InvalidProof before a fingerprint is returned; exact original KE1 remains accepted with its canonical fingerprint. Arbitrary malformed bytes do not stand in for the valid prefix | `credential_request_bound_refuses_padded_valid_ke1` |
| Actual coherent original client/server login produces valid KE2; padded copy versus exact response on separately owned original attempts | Client finish refuses InvalidProof and never calls pending save for the padded copy; exact original response invokes pending save and releases KE3 which the original server confirms. Consuming an invalid attempt does not manufacture a reusable owner | `client_response_bound_refuses_padded_valid_ke2_before_pending_save` |
| Actual client pending save succeeds and releases coherent KE3; padded copy versus exact original finalization on separately owned attempts | Server finish refuses InvalidProof without returning ConfirmedAttempt; exact original KE3 confirms the same public correlation/device/input. No Claim commit or receiver effect is inferred from cryptographic confirmation | `server_finalization_bound_refuses_padded_valid_ke3` |
| Real TLS 1.3 raw-key peer signs the requested CertificateVerify using its original Ed25519 key, then corrupts the signature; exact signer counterpart | Gateway NativeTransport.accept refuses InvalidProof and returns no DeviceConnectionProof. Original same key/ALPN/cipher negotiation with unmodified signature returns the exact device key. Signer counter proves the invalid signature was actually produced, rather than an earlier configuration refusal | `native_tls_refuses_corrupted_peer_signature_and_accepts_original` |
| Correctly signed raw-key TLS 1.3 peer completes without ALPN; canonical ALPN counterpart | TLS handshake itself succeeds with protocol TLS 1.3 and no ALPN, then the original NativeTransport completion owner refuses InvalidContext before transport/proof publication. Correct canonical ALPN returns the same device key | `native_tls_refuses_completed_peer_without_alpn_and_accepts_original` |
| Another real registry at the same current revision authenticates credential X as owner; original target registry resolves X to a valid reader or has no X; admission intent remains the original target intent | Principal mismatch refuses Invalid and absent original issuer refuses Unavailable before a new Stage write. Same-revision equality does not identify the access backend. Fresh original target admission stages the exact identity; preserve both actual store authorities without forging registry state | `equal_revision_alternate_store_admission_requires_original_issuer` |
| Original Registry enters through validated open, or bootstrap/issue/recovery/revoke/pairing persist and publication; current issuer audience is compared with retained admission intent | Existing validate_registry owns credential audience == gateway for every published credential. Admission consumes that resolved issuer audience once; remove the derived duplicate gateway comparison. Retain cross-store issuer presence/principal, current validity/member and exact invitation checks | Registry publication-path provenance; admission consumes the resolved issuer audience |
| Alternate real gateway at equal revision admits its own valid owner/read intent using the same issuer text; create targets the original registry with a matching foreign record/admission | Current original issuer audience comparison refuses Invalid before any new invitation publication, independently of the record/admission equality. Exact original-gateway counterpart publishes and survives reopen. No duplicate gateway predicate or persisted access-origin ledger is needed | `equal_revision_alternate_gateway_admission_requires_current_issuer_audience` |
| Canonical consent generation zero, otherwise valid owner/resource; positive generation | ConsentIntent constructor refuses Invalid before an invitation can consume it; accepted consent publishes only the canonical conversation-read grant | `consent_generation_floor_and_read_class_are_owned_at_construction` |
| Invitation policy lifetime zero, attempt count zero or six; positive lifetime with one/five attempts | PairingPolicy refuses each otherwise-valid invalid input independently; accepted finite endpoints retain exact policy | `policy_refuses_each_invalid_bound_and_accepts_finite_endpoints` |
| Public correlation generation zero or expiry zero; both positive | PublicIntent constructor refuses each zero independently; opaque identities do not override the bound | `public_intent_refuses_each_zero_bound_and_accepts_positive_neighbor` |
| Otherwise admissible Reserve arrives before record creation; exact creation-time neighbor | Direct transition refuses Invalid before any attempt charge; creation-time reserve remains eligible | `direct_reservation_refuses_before_creation_and_accepts_creation_time` |
| Otherwise admissible Reserve actor is another device, a principal, operator or System; exact device neighbor | Direct transition refuses WrongActor without charging an attempt; exact device admits the original attempt and `attempt_status` returns Pending. Optional absent reservation remains owned by `reservation_status` | `reservation_actor_must_be_the_exact_admitted_device` (stale Option expectation corrected after first compiler failure) |

### Current A retained agreement and verifier evidence additions

| Actual input / ordering | Required relation and scope | Evidence owner |
| --- | --- | --- |
| Two individually replay-valid Stage records have distinct invitation/consent IDs but reserve the same credential; reopen | Shared registry consistency refuses duplicate retained namespace; one unchanged original remains accepted | Real persisted registry boundary |
| Never-activated Stage plus otherwise-valid ordinary issued credential at reserved ID, with all ordinary transition/receipt IDs coherent | Refuse early issuance on reopen; independent ordinary identity and original Stage are accepted | Same registry consistency owner |
| Active publication metadata has otherwise-valid wrong ID/principal/organization/audience/expiry/grant/resource/generation | Existing stored pairing comparison rejects each independent relationship; exact original metadata accepts | Borrowed immutable pairing binding query; this alone is not a live private verifier-state bypass |
| Actual authenticated TLS proof has another permanent device key; valid ordinary bearer credential or wrong audience presented to device verifier | Refuse on same current store before session construction; original native key/read grant accepts | Actual TLS factory plus current DeviceCredentialVerifier |
| Canonical Issued DTO has a nonempty predecessor | Original CredentialTransition mapping already refuses it before registry pairing validation; no forged private proof needed | Existing domain transition owner and mapping fixture; pairing beforeNone defensive duplicate requires disposition |
| Device proof invitation/generation/credential link contradicts valid stored pairing | Shared registry validates this before any actual verifier sees it; persisted corrupt-boundary negatives prove refusal, not reachability through valid live port | Original registry/StoredPairing agreement; defensive verifier comparisons are not credited as independently reachable guards |

### Current A single agreement owners

#### Committed admission during pairing mutation

Device proof admission consumes the same committed publication contract as
bearer authentication, access reads and revision reads in
[ADR0010](../../adr/done/0010-local-authentication.md). A mutation's retained
registry mutex owns persistence and publication; it does not own admission from
the preceding committed revision. The existing health latch remains fail-closed.

| Ordering | Required relationship | Existing owner and detector |
| --- | --- | --- |
| Active device and original TLS proof while a distinct invitation's expiry mutation is held inside its injected Clock | Verify the exact original credential from the committed snapshot before the mutation is released; the unchanged device remains valid after publication | `device_admission_reads_committed_snapshot_during_pairing_mutation` uses real TLS, public pairing mutation and the original Clock boundary after mutation-mutex acquisition |
| Clock entry or early verification observation times out | Release the original mutation and join both original threads before asserting; timeout is observation failure, not proof of mutation completion | The same fixture retains original store, TLS transports and thread handles through release and join |
| Successful expiry publication | Subsequent revision advances while the unrelated active device retains its original identity and proof | Existing registry persistence and published snapshot ordering; the same fixture checks the public revision and verifier |
| Actual device revocation is acknowledged and published | Refuse the original device credential after publication; prior completed admissions are not retroactively rewritten | Original canonical revocation/publication; the same fixture and `native_key_publication_and_canonical_explicit_revoke_agree` |
| Existing directory-sync failure and reconciliation leave health failed | All committed admission readers refuse Unavailable instead of admitting from the retained earlier snapshot; a reopened store validates the actual durable state separately | `with_published_registry` checks the original health latch before and after published read acquisition. Exact failed post-replacement synchronization plus failed reconciliation has no deterministic public fixture here; the former test-selector proof is withdrawn |
| A second private directory owner encounters the platform's lock contention | Return Locked for the raw error named by fs2, including Windows lock violation; unrelated storage failures retain their original mapping | fs2 `lock_contended_error` and `private_state_lock_excludes_second_handle`; supported-platform execution is separate from macOS evidence |

This fixture isolates the committed-reader ordering. It does not establish
listener, protected framing, native worker retirement or supported-platform
acceptance.

| Boundary / mutation provenance | One enforcing owner | Current evidence and limit |
| --- | --- | --- |
| Borrowed pairing binding query receives raw canonical issuance DTOs | Ask `CredentialTransition::try_from` for complete domain issuance shape; no local copy of predecessor/revoked/time shape. Pairing comparison retains its independent DevicePairing cause and original Activate actor/time agreement | `raw_pairing_binding_query_asks_domain_for_original_issuance_shape` refuses raw DTO before/after contradictions and accepts the exact original |
| Private registry initialized from disk or changed by bootstrap/issue/recovery/revoke/create/decision/pairing commits | `validate_registry` before open initialization and every `persist`; private cached state publishes only the same validated next value after successful persistence. Reconciliation also asks the same validator | No production caller inserts unchecked Registry or mutates published cached state; test-only private assignments do not establish public reachability |
| Bearer/device verification, access and revision readers consume the committed published registry | `with_published_registry` owns read acquisition and original fail-closed health observations. It invokes a synchronous borrowed reader that returns owned output; no mutation guard, policy evaluation, await or socket IO belongs in this seam. Device binding derives generation and credential link from original registry agreement and retains actual requested audience/revoked state, proof-kind and exact live TLS key checks | Actual wrong TLS key/audience/ordinary bearer and revoked neighbors remain load-bearing; persisted wrong generation/invitation refuses at original consistency owner, not invented invalid live state |

### Implementation checkpoint and acceptance limits

These native claims describe the preserved checkout `/Users/nessa/.codex/worktrees/device-pairing/nessa-agent`, HEAD `5d57b8cdced5014cc42f2f105a5504c747a4ae02`, pending merge index `b4e0e41805a5b61b2cd3fe16255ab5d9dfc32a00`. Its retained-cohort source manifest SHA256 is `85f76fdc3dc2bfefa3665e51e74d1306eef57fb96055cee3c4b243f0dc1ba45c`; the checkpoint report is `/tmp/nessa-native-producer-retained-cohort-author-checkpoint.md` and the earlier full runtime report is `/tmp/nessa-native-producer-current-runtime-checkpoint.md`. They are historical local evidence, not current A consumer acceptance. A contains auth contracts and private-state consumers, and contains no gateway routes, native listener or protected activation. The complete #264/#265 finish line and downstream gates remain pending. Later native composition sections describe that target and preserved work, not implementation present in A.

The historical native integration checkpoint has pure state replay, canonical registry pairing histories and typed credential proofs, real native TLS/OPAQUE transcript confirmation, current Cedar admission, conditional key publication, and credential-revocation linkage. The gateway application owns one Available slot and create/claim/status/decision APIs; receiver dispatch and lookup-only terminal cleanup retain physical ownership and original receipts. Focused real auth/SQL tests exercise these owners, including caller loss and reopening. This checkpoint does **not** satisfy #264 acceptance: whole native memory accounting, actual startup/listener and public owner route composition, complete lost-message/crash/public-surface workflow, and record-enabled separate-process client acceptance still remain. Separate-process Claim/explicit approval/pinned restore is exercised through the actual native listener; that does not establish product route composition. Native deadline/shutdown ownership and private-key/audit startup owners are implemented and tested, but that is not evidence of product composition. Product record routes retain their existing owners.

### Durable private state ownership

`PublicIntent` is the single domain-owned public invitation/attempt/consent/
generation/expiry value. It contains no private principal, membership, organization
or resource. The cryptographic context owner encodes that value, its fixed read
class, proved keys and local exporter. Application storage ports consume the domain
value and a non-cloneable redacted key-material owner; they do not import TLS or
OPAQUE adapter types. The private adapter owns bounded decoding and atomic file
publication. Existing pre-KE3 recovery rows remain authoritative: no KE3 is sent
until the exact seed/pin/attempt/intent has been durably acknowledged.


| Durable private ordering | Required result | Enforcing evidence |
| --- | --- | --- |
| First local gateway key publication | Publish one private seed with exclusive destination creation before invitation/listener acknowledgement. Exact retry acknowledges that same seed; a different seed conflicts. Missing key with canonical enrollment history refuses implicit regeneration | `gateway_key_is_exclusive_and_reopens` plus composition history admission |
| Valid KE2, no prior pending record | Publish seed, authenticated gateway pin and exact PublicIntent in one bounded private record. Only acknowledged file+directory persistence returns success to ClientAttempt.finish, allowing KE3 | `real_pending_save_precedes_ke3_and_survives_restart` |
| Publication fails before rename | Preserve old/absent record; no KE3. Do not infer a claim or erase a prior pin | `pending_publication_failure_sends_no_ke3` |
| Rename occurred but acknowledgement failed | Retain the published-file fact; reconcile exact bytes, retained directory binding, file sync and directory sync. If reconciliation cannot prove the same record, return typed Uncertain and send no KE3 | Exact failed live acknowledgement is unverified after removal of fault selectors/private rendezvous. Public exact-retry/reopen proves current-file acknowledgement, not this failure ordering |
| Client restart after save, before KE3 or after lost claim reply | Restore the same seed/pin/PublicIntent, use fresh strict-pin key-proved status. Do not persist KE3/session state or invent completed enrollment | `real_pending_save_precedes_ke3_and_survives_restart` plus combined lost-answer acceptance |
| Exact save retry or competing client handle | Lifetime private-store lock excludes another writer; exact record retry is idempotent, changed key/pin/intent refuses without replacement | `pending_exact_retry_and_conflicting_state` |
| Correlated terminal not-claimed receipt, fresh charged attempt and valid KE2 | Application permits retry only from the exact pinned receipt. Atomic local CAS replaces expected prior PublicIntent while preserving seed/pin/invitation/consent/generation/expiry. Changed gateway, key, intent, stale expectation or unavailable replacement capability refuses | `pending_retry_preserves_identity_and_requires_exact_prior` plus application receipt admission |
| Missing/corrupt/oversized/unsafe private file after earlier save | Typed refusal; do not silently create another identity or pin. Public pending save/load owns the bounded persisted representation and redacted returned value. Retain the original physical file while checking malformed/unsafe replacements, restore that original file and metadata, then release the store lock and reopen through the public API. No secret bytes in diagnostics | `private_state_is_bounded_and_redacted` |

Private storage is recovery metadata, never a grant owner. The key-material type
owns and zeroizes the Nessa seed copy and is neither cloneable nor serializable;
explicit codec buffers are also zeroizing. NativeIdentity consumes this same
owner rather than maintaining a parallel seed representation. The signing
library's internal copies remain outside that erasure claim. Fixed file names
and one fixed current binary encoding have no compatibility reader. A private
lifetime filesystem lock excludes concurrent client writers. Initial publication
uses the existing retained-directory authority on supported targets; replacement
uses that owner's Unix operation and returns Unsupported elsewhere rather than
claiming portability. This platform capability remains an acceptance boundary.


| Gateway first-key audit ordering | Required result | Enforcing evidence |
| --- | --- | --- |
| No key; canonical registry has zero retained pairing records | Registry guard excludes concurrent create while the injected key owner publishes. A secret-free durable intent mints one operation ID for System first-publication to the canonical gateway target before the key file effect | `first_key_publication_holds_current_history_admission` |
| Intent audit unavailable before key rename | Typed AuditUnavailable(NotPublished); no key file or listener acknowledgement. Existing intent, if already persisted, retains its original ID/target/cause on exact retry | `gateway_publication_failure_preserves_original_operation_and_effect` |
| Key file published; outcome audit unavailable or answer lost | Retain the exact seed. Typed AuditUnavailable(Published) or Uncertain is not an ordinary no-effect failure. Listener startup refuses until restore reconciles the same seed and original intent/outcome | `gateway_publication_failure_preserves_original_operation_and_effect` |
| Restart with intent but no key and no published outcome | The lifetime private lock proves old publisher drained. Coherent absence permits the original first-publication intent to continue; there was no published key to replace | `gateway_publication_failure_preserves_original_operation_and_effect` |
| Restart with key and missing outcome | Restore and sync exact material, derive its public key through the native key owner, and finish the original outcome. Outcome acknowledgement time is the observed confirmation time, not a reconstructed rename time | `gateway_publication_failure_preserves_original_operation_and_effect` |
| Saved outcome but physical key absent; original intent present or absent | One private outcome/key-presence relation is checked by a fresh outcome read at restore absence, before save publishes a missing intent, and before its key-file effect. Refuse Corrupt without creating either missing file; preserve original audit bytes. Move original physical key and optional intent aside, then restore those exact file objects by same-directory rename; preserve bytes and private metadata while proving canonical names absent before the same save and writable reopen | `gateway_audit_missing_key_refuses_save_before_effect` |
| Existing key and matching audit; canonical history nonempty | Restore existing evidence and key, without issuing a new first-publication operation. Missing key with any retained history refuses, including Terminal/cleaned histories excluded by pending_pairings | `missing_key_with_terminal_history_refuses` |
| Competing first publishers / wrong target or different existing material | Exclusive private lock and registry guard serialize publication. Only the exact canonical gateway target and matching material reconcile; no key replacement and no new grant | `gateway_key_is_exclusive_and_reopens`; actual competing registry-owner test |

The two immutable secret-free audit files belong to the private key publication
owner, not to credential/grant authority. Intent fixes operation, canonical gateway
target, System initiator and first-publication cause. Outcome retains that intent
and the public key derived from the actual seed. No audit timestamp is claimed as
an observed rename time. A missing outcome is a retained external-effect fact,
never permission to generate a replacement identity. The bounded current codec
and exact reconciliation refuse contradictory or unsafe audit records.


| Restored first-key contradiction | Required result | Enforcing evidence |
| --- | --- | --- |
| Individually valid outcome names another operation, public key or gateway; oversized valid JSON; key exists without original intent | Refuse restoration before listener admission, without synthesizing a new intent or replacing the key. Matching original records remain restorable | `gateway_audit_rejects_relationship_and_bounds_contradictions` |


| Native startup identity ordering | Required result | Enforcing evidence |
| --- | --- | --- |
| Existing key with any canonical history | Restore exact original audited seed through the private owner, then native identity; consume no fresh entropy and mint no first-publication operation | `startup_restores_exact_audited_key_without_entropy` |
| Actual key absence; canonical all-history empty | Generate through injected entropy in the owned startup closure, ask current registry first-publication owner, then return usable identity only after key/audit acknowledgement | `startup_restores_exact_audited_key_without_entropy` |
| Actual key absence; any historical enrollment | Typed GatewayKeyHistoryExists, no key publication/listener result | `startup_missing_key_with_history_refuses` |
| Startup waiter lost while private lookup/publication remains physical | Owned closure retains registry and private-store process locks until completion/unwind. Restart remains excluded; any later completed publication is restored through its original audit, not regenerated | `startup_caller_loss_keeps_physical_owners_until_drain` |
| Injected entropy panics before verified startup return | Typed unexpected Panic worker fault; no key/audit publication, no identity result. Valid retry uses the unchanged canonical history and available private owner | `startup_entropy_fault_is_typed_and_retryable` |

Startup runs before the listener and owner request surface. It uses the same
native identity/TLS owner, not a second key-proof adapter. Its blocking closure
retains the injected registry, key store and clock even when its waiter is lost.
This startup operation is composition work, not an externally callable bootstrap
endpoint. An error never starts the listener; restart asks the same restoration
owner and canonical history guard.


The immutable ConsentIntent/PublicIntent domain owner publishes the one fixed
read class. The crypto context and upcoming public wire projection consume that
publication, rather than each selecting a class spelling. The private pending
codec is the same current profile and stores its exact opaque correlation; it
does not introduce another mutable class or grant selector.


The canonical context codec has a published Nessa fixture in the auth pairing
adapter tests. It uses public structural key bytes and fixed IDs/exporter solely
to enforce the adopted length-prefixed field order/profile/class; it is not a
successful TLS proof or an OPAQUE official vector. The independent fixed byte
fixture catches changes at the published read-class owner and crypto codec.


## Protected native consent projection and wire correlation

The protected consent projection is transient: public attempt correlation plus
canonical audience, organization/resource and the fixed read grant, omitting
private owner/member IDs. It grants no access. The gateway compares the entire
projection against its retained canonical ConsentIntent and exact record/public
operation before encoding or another effect. The client first authenticates the
key/channel and matches opaque ID/generation/class/attempt to its durable pending
record. It cannot infer previously private selectors from that opaque ID. Once it
has received a projection, it retains it in the current client operation and
refuses a conflicting same-ID projection on later status/retry. After a client
crash, durable opaque correlation remains; its first restored disclosure again
relies on the pinned gateway's canonical projection enforcer. No independent
client proof of undisclosed selectors or persisted second grant store is claimed.

| Row | Wire/projection ordering | Required outcome | Enforcing evidence |
| --- | --- | --- | --- |
| P49 | Protected projection has individually valid foreign consent ID/generation/audience/org/resource/read action | Gateway whole-projection comparison refuses before output/effect; correct canonical projection accepted. First client disclosure checks key/channel and pending opaque operation; foreign opaque correlation/class refuses | `disclosure_correlates_whole_canonical_intent`; actual wire before-write test |
| P50 | First disclosure accepted, reply lost or same-operation retry; later same-ID private selectors conflict | Retain received projection for current operation; matching status/phase update accepted, foreign full projection refused. Restart retains exact key/pin/opaque correlation and gateway matches the same durable ConsentIntent before disclosure | `disclosure_retains_received_scope_on_retry`; actual client/restart acceptance |
| P51 | Public wire metadata malformed/unknown/trailing/oversized, wrong message phase or operation | Decode bounded frame before allocating domain payload; use domain constructors/profile publications. Refuse before attempt reservation or product admission; matching frame accepted | bounded native codec tests |

ConsentIntent publishes the fixed read action/class. The projection constructor
asks that owner for read-grant representation; no wire/client copy selects a
capability. Gateway comparison and client opaque/received-projection comparison
own different evidence: canonical authority versus locally received correlation.

## Physical native enrollment connection ordering

The native composition admits at most eight physical connections, refusing rather
than queueing excess ingress. Its owned blocking closure retains the connection,
registry/runtime and the sole connection permit through I/O, caller loss and
unwind. Shutdown excludes admission and waits for actual closure drain. The TLS
phase has an absolute ten-second deadline; the bounded enrollment exchange has
one absolute thirty-second deadline after complete TLS. Remaining time is applied
before each socket read/write, so a slow partial frame cannot extend either phase.
The registration owner remains the only gateway KSF worker; server login performs
no client KSF. This connection owner admits enrollment messages only; no native
record request is implicitly accepted by it.

| Row | Physical ordering | Required result | Enforcing evidence |
| --- | --- | --- | --- |
| P52 | Eight physical connections are blocked in TLS; ninth or repeated caller-loss replacement attempts arrive | Refuse capacity before another dispatch. Dropped waiters retain original physical permit/runtime. Shutdown remains pending until all closures drain; closed owner refuses later admission | actual connection capacity/caller-loss/drain tests |
| P53 | Initial request is Hello or pinned exact Status; Hello then matching Begin then matching Confirm | Bounded strict phase grammar. Wrong phase/public operation refuses before a new reservation; once admitted, malformed/lost confirmation settles that exact attempt with actual failure, preserving settlement failure separately. Correct canonical exchange emits protected status after claim commit | actual wire TLS enrollment/status tests |
| P54 | TLS deadline or absolute enrollment deadline expires; partial reads/writes keep arriving | Recompute remaining phase time before every socket operation. No deadline extension per frame/byte. Admitted attempt receives HandshakeDeadline; actual socket loss receives ConnectionClosed. A committed claim/lost receipt is never relabeled failure and fresh strict-pin status reads the canonical claim | actual partial-frame deadline and lost-message/reconnect tests |
| P55 | Physical worker entropy/IO implementation panics | Retain permit/runtime through unwind and return typed unexpected worker fault to a live waiter. No guessed terminal outcome or success. Current canonical attempt, if reservation preceded panic, remains pending until its real lifecycle/restart owner settles it | physical panic/drain test |

Ingress/egress byte limits and library buffer limits are distinct from a measured
whole-connection memory bound. The corrected 4 KiB TLS ingress/egress handshake
budgets and 4 KiB plaintext frame limit are enforced; final 128 KiB memory
acceptance still requires accounting for retained TLS/config/codec allocations.

| Row | Native client physical ordering | Required result | Enforcing evidence |
| --- | --- | --- | --- |
| P56 | Manual enrollment starts with no durable pending record | Own code, injected entropy and private store in the single physical client KSF closure. Generate device key/attempt, complete provisional mutual TLS, match Hello/Challenge correlation, verify KE2, atomically save exact key/pin/PublicIntent, then send KE3. Storage failure releases no KE3 and remains typed | real native client/gateway/private-store enrollment tests |
| P57 | Client restarts with a durable pending record, including uncertain earlier reply | Restore exact seed/pin/opaque attempt and use fresh strict-key TLS Status; do not generate a replacement key, reuse code or infer claim. Match opaque correlation; an optional previously received transient projection must match whole scope. Any concurrent KSF/status operation is refused while the original physical closure remains owned | actual client restart/status/caller-loss tests |
| P58 | Client pending publication fails or physical client worker panics/is detached | Preserve actual storage/worker failure internally; KE3 is unavailable before save acknowledgment. Owned closure retains key/code/private-store and sole KSF permit through drain/unwind. Canonical gateway claim may exist after lost committed reply, and fresh strict-key status is the only settlement path | actual pre/post-save failures and physical client fault tests |

This client owner exposes enrollment/status results only. An Active result is a
historical receipt, never a reusable product authorization token. Native product
record requests must later consume fresh current key credential, Cedar, receiver
epoch and #265 request/scope/nonce evidence at the existing admission owners.

| Row | Existing clock seam and nested physical work | Required result | Enforcing evidence |
| --- | --- | --- | --- |
| P59 | Native server/client deadline construction, socket read/write/flush, buffered input, KSF, pending save or pinned status | Consume the same injected `app::ports::Clock` monotonic elapsed owner supplied by RuntimeDependencies. The shared absolute deadline computes remaining time before each nested socket operation; buffered KE3 and post-KSF/storage/status boundaries ask that same owner. No direct clock reads or fallback timer authority. Checked deadline overflow refuses before dispatch. Actual socket timeout bounds blocked syscalls while injected clock substitution proves authority-boundary expiry | substituted-clock partial frame and buffered completion refusal; matching real-clock enrollment/status tests |

The P59 confirmation boundary is one private decision owner: after bounded decode,
it asks the retained shared deadline and matches the exact Confirm/PublicIntent
before releasing bytes to the existing PAKE finish owner. Its direct buffered-input
fixture intentionally supplies an already-decoded request, bypassing socket
checks; it proves expired input cannot reach PAKE finish. That fixture proves no
PAKE authentication itself. The actual TLS/OPAQUE expired-KE3 case independently
proves the combined connection refuses claim and retains the canonical cause,
but may be refused earlier by nested socket deadlines. Both paths keep the same
absolute clock/deadline decision and valid counterpart.

The buffered-completion probe must also consume actual selected-library valid KE3
and permanent-key TLS evidence into the real canonical registry claim owner.
Late input leaves the reserved record unchanged and produces no claim; on-time
input commits Claim once. This effect-level counterpart is separate from socket
expiry and from the pure decoded representation/phase checks.

| Row | Public runtime completion correlation | Required result | Enforcing evidence |
| --- | --- | --- | --- |
| P60 | Two actual TLS channels have the same gateway and permanent device key but distinct exporter; valid original KE3 is supplied with the newer channel | Runtime finish compares retained canonical PAKE context bytes to the current channel's context before PAKE finish or claim publication. Refuse InvalidContext with the canonical reserved record/cause/initiator unchanged; no invented claim, credential, receiver or fence. Original-channel valid completion remains accepted | actual same-key two-channel finish refusal and original-channel acceptance; owner mutation probe |

| Row | Explicit client retry ordering | Required result | Enforcing evidence |
| --- | --- | --- | --- |
| P61 | Durable pending exists; retry starts | Restore exact seed/pin/PublicIntent; fresh strict-pin Status for that original operation must return correlated canonical terminal NOT-CLAIMED attempt outcome. Pending, uncertain, claimed or malformed/foreign status refuses before new attempt entropy or CAS. Retain original receipt/cause; no caller-provided verified-outcome flag. Close original TLS channel before constructing fresh retry TLS state | actual original Pending/Claimed refusal and terminal failed retry tests |
| P62 | Exact original NOT-CLAIMED receipt; fresh strict-pin Hello | Generate a distinct new attempt only now. Fresh Hello must equal the original public operation with only that attempt replaced; foreign invitation/consent/generation/expiry/class refuses before KE1 reservation/CAS. Existing gateway reserve owner still asks current canonical phase/budget. Valid KE2 precedes expected-original pending CAS preserving seed/pin; only acknowledgment releases KE3. Return both original and new receipts | actual same-invitation retry, foreign replacement, storage CAS/lost reply tests |

Retry is one physically retained client worker. Its original status connection and
fresh enrollment connection each retain the fixed TLS/enrollment phase bounds;
there can be up to eighty seconds across those two sequential phase pairs. The
first TLS state is dropped before constructing the second, so retry retains only
one TLS/config/codec session at a time. Two caller-supplied raw sockets are owned
by the closure; this does not establish a whole-process/kernel memory bound.

PublicIntent's immutable `with_attempt` representation owns how exact enrollment
metadata is retained while selecting another attempt. Both native retry's Hello
comparison and retained pending CAS consume that representation; private storage
still independently requires exact expected prior record and unchanged seed/pin.
No representation operation is itself proof of NOT-CLAIMED admission.

| Row | Retry pending publication uncertainty | Required result | Enforcing evidence |
| --- | --- | --- | --- |
| P63 | Verified original NOT-CLAIMED receipt; expected-original CAS publishes new attempt but returns Uncertain | No new KE3 is released. Keep exact new pending attempt/key/pin, and original canonical outcome unchanged. Fresh strict-pin status settles the new attempt; later retry starts from that new original receipt and can succeed without changing identity or overwriting an older expectation | actual post-retry-CAS uncertainty/status/next retry test |

## Separate-process Rust example boundary

The example composes existing retained private storage, NativeEnrollmentClient,
production OS entropy and existing RuntimeDependencies clock. Its once-loaded
configuration selects numeric socket address, private root and enroll/status/retry;
manual code is read from a bounded stdin frame and owned bytes erase after parse.
It displays enrollment receipts only; no Active receipt is described as current
read authorization. The Rust client library owns protocol interpretation and
recovery. The example performs no registry retrofit, automatic owner consent or
product record-source substitute.

Production OsEntropy implements the already selected injected rand 0.8 crypto
interface. `try_fill_bytes` preserves fallible OS acquisition; required infallible
methods panic on acquisition failure, which the physical worker retains as typed
unexpected fault. It does not install another generator or claim fabricated
successful entropy. The existing injected entropy-fault tests enforce caller
ownership and failure meaning; an OS random sample is not a proof of randomness.

| Row | Separate executable and current consent | Required result | Enforcing evidence |
| --- | --- | --- | --- |
| P64 | Built Rust example runs as separate OS process; enrollment exits, owner approves the exact claimed permanent key, then fresh status process restores private state | Actual native TLS/OPAQUE commits only Claim before explicit current owner approval. Each process releases its retained private lock; fresh strict-pin status reads the same canonical enrollment and reflects Approved. stdout is one bounded public receipt, never code, seed, private selectors or an assertion of current record authorization | explicit built-example process acceptance test with actual registry/Cedar/native connection owner; normal configuration/stdin unit tests |

The executable acceptance test is explicitly invoked with a prebuilt example
path, so it never builds dependencies or starts nested Cargo work inside the
pairing test. Its listener is a loopback test fixture for the actual connection
owner; it does not establish product listener composition or record-route
acceptance.

## Measured native resource classes and handshake correction

The original proposed 128-KiB connection budget was separate from the single
64-MiB gateway creation KSF and single 64-MiB client login KSF. It never includes
those KSFs, runtime thread stacks, allocator metadata, process baseline or kernel
socket buffers. Cumulative wire byte ceilings and rustls outbound settings do
not prove any one of those heap/resident classes. The external allocation probe
of the existing 64-KiB handshake ingress accepted enough public ClientHello bytes
to allocate 884,458 extra heap bytes through an ALPN list before rejecting it;
that disproves using the old wire ceiling as the proposed connection heap bound.

| Row | Native fixed-profile TLS handshake budget | Required result | Enforcing evidence |
| --- | --- | --- | --- |
| P65 | Fixed TLS 1.3/RPK handshake ingress or egress reaches its cumulative 4096-byte ceiling | The existing stream budget owner refuses before reading/writing any further wire bytes. No custom TLS parser, dependency bump or alternate clock/proof owner. Actual selected native handshake and OPAQUE enrollment remain accepted. Malformed oversized ALPN/fragmented input refuses; near-boundary complete ALPN heap amplification is measured separately | actual accepted enrollment and external near-boundary/oversized allocator cases; public NativeTransport ingress refusal; egress-at-ceiling is not currently proved at the public boundary. Prior private budget probes remain historical implementation evidence |

The engineering correction initially sets the cumulative handshake wire ceiling
to 4 KiB in each direction; it leaves the 4-KiB application frame and separate
KSF cost unchanged. The final connection heap/resident accounting remains open
until the actual valid/hostile measurements and retained/cancelled worker classes
are recorded. [Pinned rustls buffer-limit documentation](https://docs.rs/rustls/0.23.45/rustls/struct.ConnectionCommon.html#method.set_buffer_limit)
confirms its outbound application-data setting is not a whole allocation limit;
its [pinned deframer owner](https://github.com/rustls/rustls/blob/v/0.23.45/rustls/src/msgs/deframer/buffers.rs)
independently grows input buffers while assembling handshake messages.

## Native listener physical ownership

| Row | Listener / result delivery ordering | Required result | Enforcing evidence |
| --- | --- | --- | --- |
| P66 | Once-bound native listener accepts a socket | Synchronous dispatch consumes the existing connection semaphore before starting the physical closure. The single private permit lease is retained by closure and noncloneable result handle; completed-but-undelivered results therefore retain capacity too. No unpolled request queue, second counter or approval owner. Occupied capacity closes the new socket before TLS/attempt admission | actual eight occupied peers and ninth socket refusal; capacity owner test |
| P67 | Stop, listener error, or listener caller loss while accepted sockets are physically blocked | Close listener/admission, retain each accepted closure/runtime/lease through completion/unwind. Explicit stop collects bounded original result handles then asks existing physical shutdown drain. Caller loss drops waiter handles, while original physical lease still excludes reopening until actual peer/deadline drain | actual stopped/aborted listener with held peers, refusal and joined physical drain |
| P68 | Physical closure completes before its result observer runs | Same permit remains retained until result handle consumes/drops its outcome. Dropped observer cannot release capacity while physical work remains. This conservatively bounds pending handles with the existing physical owner instead of adding a listener queue budget | synchronous dispatch completion-handoff and valid accepted native counterpart |

Product startup must restore and reconcile the existing audited gateway key before
binding this listener. Binding is injected from composition; the listener never
issues an identity or creates owner consent. It owns enrollment connections only;
product record routes remain held for the reviewed shared assembly.

## Current retained storage consumption (#324)

Approved owner plan: FilePairingState keeps the original published handle for live acknowledgement; restart uses writable existing-file acquisition and makes no original-native-identity claim. Shared storage PR339 owns rename, cleanup, native identity and supported-platform replacement. The following rows were recorded before consumer code changes.

| Row | Ordering | Result and retained authority | Planned enforcing evidence |
| --- | --- | --- | --- |
| P69 | Valid same-enrollment retry after strict-pin terminal not-claimed receipt; expected prior pending CAS matches | Shared replacement works on supported Unix/Windows; original key/pin/consent/generation stay; only attempt changes. No exclusive fallback | existing pending_retry_preserves_identity_and_requires_exact_prior plus actual Windows counterpart and separate-process retry |
| P70 | Live rename succeeds; acknowledgement fails; original handle/name/exact bytes still bound | Reconcile using original Published object under existing operation/OS lock; acknowledged once, no second namespace effect | Original-handle ownership remains source-inspected; deterministic failed live acknowledgement is not publicly reproduced by Auth after removal of fault selectors. |
| P71 | Live rename succeeds; destination replaced by otherwise-valid exact-byte foreign file before reconcile (where OS permits) | Exact bytes do not establish original identity; refuse Uncertain, preserve published fact; never delete/replace foreign destination | Actual public current-file retry/reopen is covered separately; exact foreign replacement during live acknowledgement is not reproduced without an IO-boundary seam. |
| P72 | Live wrong destination name, altered/truncated bytes, changed lock or retained binding | Refuse before successful acknowledgement; no fabricated claim/product authority or second publication | Public corrupt pending/key and lock fixtures cover reachable refusal; arbitrary private name/handle calls are not acceptance evidence. |
| P73 | Caller/result lost after pending save or acknowledged publication; process closes then restarts | Reopen ReadWrite existing canonical file; acknowledge exact saved bytes under current lock/binding; no reconstructed old native identity or KE3 fiction; strict-pin status determines claim | real_pending_save_precedes_ke3_and_survives_restart, reopen exact-retry, Windows writable flush runtime |
| P74 | Restart missing/different/corrupt record or expected-prior/key/pin/intent mismatch | Conflict/Corrupt/Uncertain according to existing owner semantics; do not create via OPEN_EXISTING, overwrite evidence, regenerate key or release pending | Existing public pending conflict/bound/redaction and gateway audit correlation tests; private missing-name reconciliation assertion withdrawn. |
| P75 | Publication fails before rename, possibly independent reservation cleanup failure | No Published fact; storage alone owns original cleanup; no live post-effect reconcile; retain typed primary and independent cleanup meaning | pending_publication_failure_sends_no_ke3 uses real canonical-name obstruction; shared storage retains its separately scoped cleanup evidence. |
| P76 | Audit intent fails before key; key effect succeeds then audit answer/outcome fails; reopen exact operation | No key on first failure; retain exact published key and original audit operation/cause on latter failure; listener cannot claim startup success until reconciliation | gateway_publication_failure_preserves_original_operation_and_effect uses real key/outcome obstruction through the existing clock observation; public restore/retry preserves original audit. |
| P77 | Save/reconcile physical worker retained while caller/composition lost or competing reopen attempted | Existing physical worker keeps Arc<FilePairingState>/OS lock through drain; repeated admission remains bounded; no new lock owner/cleanup flag | existing client cancellation/capacity plus blocked ordinary save/reconcile/reopen fixture, joined physical drain and valid later reopen |


### Compound storage failure orderings

| Row | Ordering | Required evidence / enforcing test |
| --- | --- | --- |
| P78 | Original reservation identity refusal plus independent original cleanup refusal | Shared storage owns original reservation cleanup. Direct Auth publication_failure mapping test withdrawn; no public consumer proof of this exact compound failure is claimed. |
| P79 | Native publication failure with no cleanup failure | Actual public gateway publication obstruction returns typed failure with original effect meaning; private mapping fixture withdrawn. |
| P80 | Published native effect followed by failed live reconciliation | Production original Published handle is retained; former private-helper/rendezvous evidence withdrawn, exact failed post-effect reconciliation remains unsupported here. |
| P81 | Audit-file failure before/after gateway key effect | gateway_publication_failure_preserves_original_operation_and_effect separates public key and audit-file effects and exact original operation. |
| P82 | Error reaches client/startup caller | Actual public publication failure diagnostics are checked for redaction; existing public pre-KE3 refusal remains. Native startup consumer proof remains separate. |

These typed error projections confer no cleanup or enrollment authority. Stage translation is exhaustive representation mapping at the storage adapter; primary unsafe storage differs from availability; original publication effect comes only from the storage-owned Published object. A successful live reconciliation settles the lost acknowledgement. An uncertain restart preserves current exact bytes under lock without recreating the historical Published object. Windows binding acknowledgement does not assert directory-entry power-loss durability.

### Retained client private-save orderings (P77 extension)

| Row | Ordering | Required result / test |
| --- | --- | --- |
| P83 | Actual KE2-authenticated enrollment enters substituted pending-save port; caller lost before real save; composition/shutdown waiters then dropped | Existing physical closure retains actual FilePairingState and sole permit. Busy/Locked and pending shutdown until release; real save may finish admitted enrollment and claim. After physical drain, reopen exact key/pin/intent and compare canonical outcome. `client_caller_loss_during_real_pending_save_retains_private_lock`. |
| P84 | Isolated client physical dispatch enters substituted pending-save port for exact saved bytes; caller/composition lost before real exact reconcile | Same physical capacity/lock retention. On release actual save_pending exact retry acknowledges current ReadWrite file; no new attempt, replacement or grant. Reopen same bytes after drain. `client_caller_loss_during_exact_pending_reconcile_retains_private_lock`. |
| P85 | Equivalent save/reconcile completes with caller still present | Actual result received, original authority retained, shutdown joins and later reopen accepted; no cancellation is fabricated. Same fixture's noncancelled counterpart. |

Substituted-port barriers identify test phases only. They do not claim an OS syscall is blocked or a native storage acknowledgement failed. Caller loss is not a server terminal decision; an already admitted valid KE3 may complete with its actual canonical claim. No duplicate cancellation cause or cleanup ledger is introduced.

## Product startup and owner consent integration

Approved configuration: optional `native.listenAddress` is a numeric `SocketAddr`; absent/null native is disabled, without private-state acquisition or a native listener. Explicit nonloopback addresses are allowed only for the existing TLS1.3/raw-public-key native profile. Browser bind policy is unchanged. Requested native key/audit/cleanup uncertainty fails current startup before listener/readiness success; no separate readiness state. The namespace-relative private target is `native-pairing/`, retained by the existing private state owner.

### Startup ordering table

| Row | Event / ordering | Canonical result and exact enforcer / intended fixture |
| --- | --- | --- |
| S1 | native absent/null | No native private-store acquisition/key creation/socket; ordinary browser service remains current. RuntimeConfig constructor and composition test `native_disabled_has_no_native_effect`. |
| S2 | explicit native configuration malformed/unknown fields/non-numeric address | Config owner refuses before private effects or listener bind; standard SocketAddr parser, not hostname/network lookup. `native_config_refuses_before_effect`. |
| S3 | first key absent, canonical all-history empty | Existing publish_first_gateway_key guard retained through key-intent → key rename → audit outcome; only then identity restore and bind. No enrollment/grant/receiver issuance. `native_startup_first_key_uses_canonical_guard`. |
| S4 | key absent with any retained enrollment history or key/audit contradiction | Refuse requested native startup; never regenerate a different key or erase evidence. Current key/registry owners, `native_startup_history_without_key_refuses_before_bind`. |
| S5 | existing exact key with unfinished original audit outcome | Original operation/gateway/cause retained; writable current-file acknowledge/outcome reconcile; no new first-publication grant. Failure prevents advertised native readiness. `native_startup_exact_audit_restore_before_bind`. |
| S6 | canonical Available record after gateway restart | GatewayPairing.open ends only matching resource Available with Restarted; admitted receipts settle through domain replay; stale code/KE3 cannot resume missing PAKE setup. `native_startup_restart_preserves_original_attempt`. |
| S7 | Terminal cleanup-pending, prior dispatch no longer live | Acquire existing StageOwnership/exclusion then original receipt lookup only. No receipt → private no-effect completion; receipt → original remember/fence/cleanup. No new Pair or invented absence. `native_startup_terminal_lookup_only`. |
| S8 | Terminal cleanup receipt/SQL/audit unavailable or physical outcome uncertain | Retain original terminal cause/initiator/target and cleanup-pending; refuse requested native service readiness. Do not claim cleanup or auth usability from a failed lookup. `native_startup_uncertain_cleanup_refuses_before_bind`. |
| S9 | Claimed/Approved/Staging retained and current invitation still eligible | Preserve current canonical stage; never auto-consent/auto-activate from startup. Existing owner status/activation command admits continuation; expiry uses existing domain Expired transition where allowed. `native_startup_preserves_owner_consent_boundary`. |
| S10 | preparation succeeds then native bind fails | Typed bind failure, original key/audit/history preserved. No listener success; no fabricated rollback/publication. `native_bind_failure_preserves_key_and_history`. |
| S11 | native bound and service running; expiry/current owner revoke | Existing clock/registry domain transition/credential bridge determines cause. Cleanup uses canonical proof/fence; later protected admission checks current auth/receiver. No timer-produced authority. `native_runtime_terminal_and_late_effect_agree`. |
| S12 | stop/caller loss while registration/connection/receiver closure retained | Stop native admission/listener; retain current permit/registry/private-store ownership until physical drain, join original closures before shutdown success. Existing listener P66–P68, stage/cleanup lease and new save fixtures; startup cannot reopen early. `native_shutdown_joins_all_physical_owners`. |

S7/S8 pre-bind reconciliation consumes every required terminal obligation within the existing configured receipt/file bounds, with one original stage lease per dispatched operation; native task collection cannot retain unbounded retries. If a failure leaves more obligations than can settle, return startup failure truthfully. A retry uses the same retained correlation/receipt. No broad automatic retry scheduler is proposed; running periodic expiry uses existing current-state clock interval and asks canonical transition owners, with its stop/drain documented before code.

### Current owner protocol wiring

Approved owner transport: **existing authenticated product `/session`**, with generated schema/DTOs owned by `protocol/product/manifest.json` and the existing generator; server adapters in product/socket.rs/feature module, use cases in existing device_pairing application/runtime. Existing online CLI auth/session source can invoke these methods; no CLI direct registry write. Native unauthenticated enrollment/status surface remains the current TLS/PAKE codec; owner consent never arrives there.

No request supplies owner/member/org/audience/resource/grants/expiry/approval booleans. Fixed read class and generation are published by canonical server intent. Invitation and consent locators retain their existing fixed byte-array representation; approve's exact32-byte device key through current DeviceKey constructor; generated DTO validation bounds arrays/unknown fields before domain use.

| Method | Exact params | Exact result shape / existing owner |
| --- | --- | --- |
| pairing.create | empty object | `{invitationId, expiresAtMs, code}` plus fixed public intent reference `{consentId,generation,class}`; GatewayPairing.create does current prepare/registration/CAS before code. Code shown once; no secret in logs or audit. |
| pairing.pending | empty object | bounded unfinished `{items:[PairingOwnerStatus]}` from existing pending_pairings collection and current exact-intent owner authorization for each result; no broad registry-history listing. Needed to discover an Available invitation after lost create answer without replaying stored secret. |
| pairing.status | `{invitationId}` | `PairingOwnerStatus` from current owner_status; canonical phase/terminal/cleanup plus exact claimed key and consent projection when present. |
| pairing.approve | `{invitationId,deviceKey:[32 bytes]}` | canonical status after exact OwnerDecision::Approve and existing ActivatePairing with server-generated credential/request reserved at registry owner. Intermediate Approved/Staging remains visible if physical activation fails; route must retain typed effect/cleanup outcome. |
| pairing.deny | `{invitationId}` | current owner decide Deny + exact cleanup coordinator where required; returns canonical status or typed pending cleanup failure. |
| pairing.cancel | `{invitationId}` | current owner decide Cancel + exact cleanup; preserve first terminal cause. Useful after lost create answer; a new create does not silently replace Available. |
| credential.revoke (existing) | existing `{credentialId}` | retain existing explicit revoke semantics; linked Active key enrollment terminalizes through canonical credential transition bridge and cleanup. No separate pairing.revoke algorithm. |

`PairingOwnerStatus` fields: `invitationId`, `publicIntent {consentId,generation,class,expiresAtMs}`, `phase` from existing PairingPhase, optional `claimedDeviceKey:[32 bytes]`, optional `credentialId`, optional canonical `receiver {id,accessEpoch}`, optional `terminal {cause,initiator}` through current audit actor projection, and `cleanupPending` **derived from record**, never separately persisted. Authenticated owner also receives the exact immutable full consent resource/action projection already owned by ConsentIntent. No new enum renames/synthetic Connecting state. Status is historical enrollment plus cleanup fact, not current product-read authorization; current authorization is reevaluated per operation.

Create answer loss: code is not durably replayed/reconstructed. pairing.pending reports actual invitation; owner explicitly cancels then creates a new one if code was lost. A retry of pairing.create while Available remains gets current AvailableSlotOccupied; no same-name replacement or second retained request ledger. Code JSON representation is transient private owner response; choose generated bounded-string DTO, zeroize owned mutable code/serialized buffers where actually supported, never claim all serializer/native copies erase. Inspect current response/log path before code; secret must not become ordinary diagnostics. No protocol/version bump or older contract reader.

### Owner ordering table

| Row | Event / ordering | Required original result / intended fixture |
| --- | --- | --- |
| O1 | authenticated current owner creates into empty slot | registration → canonical create commit → code response; no code on stale policy/CAS. `owner_route_create_orders_code_after_commit`. |
| O2 | competing create / response lost | exact Available slot preserved; pending/status current owner can discover it; explicit cancel/new create only. `owner_route_lost_create_answer_keeps_slot`. |
| O3 | foreign org/resource or non-owner session asks pending/status | current exact AuthorizePairing refuses before private disclosure; no request-chosen resource or membership. `owner_route_status_admission_precedes_disclosure`. |
| O4 | individually valid foreign device key/intent/generation at approval | current immutable record/key comparison + registry admission refuses before stage/receiver/credential effects. `owner_route_approve_exact_claim_before_effect`. |
| O5 | same correct key approved; receiver response lost/restart | existing durable Stage/request/credential rejoined; original Pair receipt and epoch retained, current SQL/Cedar + registry CAS before Active; no duplicate issue. `owner_route_activation_retry_uses_original_stage`. |
| O6 | current SQL revoke before publication read versus after admitted read | before read no Active; after admitted read historical completion may publish, later product reads denied. Canonical receipt/cause retained. `owner_route_current_receiver_admission`. |
| O7 | deny/cancel/revoke versus in-flight registration/KE3/stage/fence | current transition owner retains first cause/initiator; late physical receipt remembered/cleaned, never recreates owner authority. `owner_route_terminal_preserves_original_cause`. |
| O8 | caller loss after owner commit or SQL effect before response | status/restart shows actual canonical stage; owned physical dispatch retained/drained; failure does not claim rollback. `owner_route_caller_loss_retains_effect_and_cleanup`. |
| O9 | owner recovery / explicit credential.revoke | existing recovery scope preserves read-only key and Admin membership; actual explicit revoke terminalizes exact key link, cleanup retains real initiator. Existing combined fixtures plus route counterpart. |

Route caller dispatch should reuse the existing product request/control capacity and physical owner, not add another Connect semaphore or control path. Pairing create's KSF continues sole RegistrationWorker permit. Cancellation must remain admissible according to current product controls; queued/prepoll versus admitted/dropped owner calls need row-derived evidence on actual route. Actual protocol limits derive owner-published identity/key/intent bounds; do not hand-copy constants into schema and adapter. If generator cannot represent those existing bounds, publish them at their existing owner rather than invent stricter selectors.


### Published owner wire dimensions

The existing pairing value owner publishes the identity/key widths in
`nessa-auth/src/domain/pairing/value_objects/wire-values.json`. The product
generator derives compiled domain constants, a small generated byte-array schema,
and Rust DTO array expressions from that data. Domain code reads generated
constants, never JSON. This is build-time contract data, not runtime policy; the
Ed25519/OPAQUE suite remains the adopted fixed suite. The authored product schema references that generated schema. Existing generator `--check`
checks all artifacts. The owner-data mutation fixture must change this data and
observe every derived output; no independently authored schema width or text
codec is introduced.

Receiver SQL is acquired once in local composition when agents or native are
configured; both receive the same concrete Arc and existing policy revision.
Native without agents creates no conversation metadata/provider. Existing typed
dataset acquisition mapping is portable for that current receiver owner. Native
startup failures retain finite private publication/identity/enrollment/cleanup
meaning through RunError::Native, using the current authentication exit reason
and retry policy without a new readiness state.

## 9. Current product admission drain and combined shutdown proposal

Approved before lifecycle/admission source. Current pairing owner routes use only ProductRouteState.requests (128) and controls (32), with deny/cancel selected as controls. The socket dispatch owns those existing permits in detached Tokio tasks. Product deletions (8), upload-begins (16), and nested installs (1) are different owners; no native pairing operation selects them. Existing conversation shutdown still owns its provider/deletion/storage retirement; this proposal does not replace those owners or broaden their resource claims.

Extend the current request/control admission representation, not NativeService capacity: retain one Arc<Notify> in ProductRouteState, wrap the actual existing OwnedSemaphorePermit so drop first releases it then notifies. Derive stopped admission from Semaphore::is_closed; no additional closed bool, request counter or ledger. Constants REQUEST_CAPACITY/CONTROL_CAPACITY live beside existing constructors and drain compares returned permits against those same constructor publications. Notify is armed before checking both capacities, avoiding lost wakeups. Admission and permit movement happen in product/socket.rs before spawn; dropped socket leaves the detached task holding the same permit. A producer/panic unwinds that permit and wakes the drain. No route can mint successful cleanup through a dropped permit: canonical registry/receiver proofs remain unchanged.

| Row | Ordering | Required result / fixture |
| --- | --- | --- |
| D1 | Stop begins, route already admitted or queued task unpolled | Close current requests/controls immediately. Already admitted task retains original permit; later attempts refuse from closed semaphore. No new native registration or owner effect is admitted. `owner_shutdown_excludes_new_admission`. |
| D2 | Admitted conversation/provider request waits for runtime, native request registers, listener physical peer waits | **Before awaiting product drain**, signal native listener stop and begin gateway registration.shutdown plus existing ConversationService.shutdown concurrently. The conversation future sets its actual retirement/stops signal before its own admission wait. Native connections and registration close through their current owners. `owner_shutdown_signals_before_waiting_for_requests` includes a provider waiter positive counterpart. |
| D3 | Caller/socket disappears while approved stage/cleanup SQL closure is held | Detached request holds original shared permit; physical stage/cleanup lease retains registry and exact effect correlation. Product drain remains pending until actual route completes; native drain remains pending until physical closure releases. `owner_shutdown_waits_for_detached_pairing_effect`. |
| D4 | Native listener/registration and conversation shutdown have independent failures | Run every original stop future, collect all outcomes, never short-circuit another stop because one failed. Report carries independent optional ConversationError and NativeShutdownError; unreported remains distinct. No relabeling original terminal cause/initiator. `owner_shutdown_retains_independent_failures`. |
| D5 | Both shared product capacities drained and original native listener/registration physically drained | Only then run bounded original Terminal cleanup lookup/fence reconciliation. Configured max_receipts/max_registry_bytes and the original StageOwnership recheck remain enforcers; terminal cleanup-pending is not limited to32. Preserve Claimed/Approved/Staging without automatic consent/activation. `owner_shutdown_reconciles_after_original_dispatch_drain`. |
| D6 | Reconciliation unavailable/uncertain, or shutdown waiter lost/panics | Keep canonical cleanup pending and original physical owners retained by owned shutdown task. Existing callback report stays Unreported until actual result; no success or private-store release claim. `owner_shutdown_loss_keeps_original_physical_owners`. |
| D7 | All stops/drains/reconciliation confirmed | Release retained private-state/runtime/registry composition after original result; current browser serve shutdown reports confirmed. Subsequent open succeeds with original state, not regenerated identities. `owner_shutdown_confirmed_allows_exact_reopen`. |

Implementation locations: product/state.rs capacity constants/Notify/owned-permit wrapper plus close_and_drain operation; product/socket.rs wraps admitted original permits and classifies pairing deny/cancel as existing controls. composition/native_pairing.rs owns prepared listener/private-state/runtime and original joined task, including bounded pending cleanup coordinator. composition/root.rs launches native lifecycle only after both binds/startup/endpoint preparation, records combined shutdown report before invoking stop, polls original stop futures concurrently with product drain, and invokes final native terminal reconciliation after drain. core/error.rs exposes finite NativeShutdownError and combined shutdown error retaining the existing ConversationError; core restart/exit-code still use existing shutdown classification. Current callback `ShutdownReport::Unreported` remains the report state, not an added Connect ledger.

Normal peer failures are observed with redacted finite facts. Requested native listener accept failure triggers the existing whole-gateway stop promptly. Its finite serving failure is retained independently alongside conversation/native shutdown failures; browser health cannot continue advertising a configured native service after that task has silently ended. The original shutdown report remains Unreported until the owned shutdown task reports its actual combined result.

A cancelled root shutdown future must leave the *owned original shutdown task* running; merely dropping join handles is not sufficient to claim drain. Existing physical owners already retain themselves on caller loss, but the combined shutdown observer must also retain private state until terminal reconciliation settles. No new approval/authority flag is introduced; task ownership plus current semaphore/registry evidence is sufficient. All test barriers bounded and physical children joined before evidence is accepted.

### Fixed manual-code output publication

The same compile-time owner data publishes the adopted eight code symbols.
ManualCode consumes that generated constant for owned bytes, parsing and display
length; the generated response schema derives the grouped display length. This
is not runtime configuration or a changed profile. The existing exact
normalization fixture retains the eight-symbol alphabet, grouping and accepted
encoding; generation mutation evidence must derive the display schema and
compiled constant from the same publication.

Native listener serving failure is reported once from its converged loop outcome,
before the first physical-drain await. That finite ErrorKind notification stops
the whole gateway; channel receipt/closure is not physical drain confirmation.
The original listener result remains owned through peer/connection drain. A
private error-branch ordering fixture and source bridge support this boundary;
real retained-peer drain remains separate evidence, without a deterministic OS
accept-failure claim.

### Owner code response ownership

The one-time code DTO owns a mutable string only until conversion to the current
response payload; that original string is erased after conversion. The existing
response payload representation owns the JSON value through queueing, delivery
loss and drop. Its transparent wrapper redacts Debug, exposes immutable access
only, and recursively erases owned JSON string values on Drop; cloned payloads
retain that same owner. Serialization/wire shape is unchanged. Events and error
representations remain outside this change.

The serialized String handed to the existing WebSocket framework, its immutable
framework buffers and kernel/network copies have upstream lifetimes; this code
does not promise their erasure. Partial allocations made by an arbitrary failing
serde serializer are also outside the wrapper's completed-value Drop contract.
The actual native create lost-answer route must preserve canonical Available,
permit pending discovery/cancel, erase its owned DTO/queued JSON copy, and expose
no code in Debug/audit. Owned JSON erasure and redacted Debug have owner fixtures;
full create/queue loss acceptance belongs to the actual owner route fixture.

### Combined serving and shutdown outcome

The root lifecycle retains independent browser serving and cleanup outcomes.
A confirmed shutdown plus browser failure keeps the original `Serve(io::Error)`.
An independently unconfirmed shutdown plus browser failure returns the original
browser error and the combined shutdown evidence together; an unreported stop
remains distinct from a reported conversation/native failure. The report is
consumed once and leaves `Unreported`, so a repeated read cannot claim success.

| Ordering | Required result / enforcing fixture |
| --- | --- |
| Browser serving fails, all original shutdown owners confirm | Original browser `Serve`; `the_process_result_carries_both_serving_and_what_shutdown_reported`. |
| Browser serving fails, conversation or native stop reports a failure | `ServeAndShutdown` retains original browser error and combined independent stop evidence; same fixture and native counterpart. |
| Browser serving fails, stop never reports | `ServeAndShutdown` retains original browser error with unreported cleanup, never inventing a successful stop; `serving_failure_preserves_unreported_shutdown`. |

### Original shutdown panic retention

The root installs private `ShutdownRetention` before any stop await, including
before polling the owned stop task. Its owned values are the same product state,
conversation service and `PreparedNative`/private directory. If that task panics,
is never polled, or is cancelled, Drop closes current admission and asks the same
idempotent stop/drain owners concurrently. It retains the private lock until both
product request/control permits and native connection/registration physical
permits return. Drop performs no terminal reconciliation and writes no report;
`Unreported` and canonical original cleanup-pending remain truthful. Normal
shutdown takes this retained ownership only after its canonical reconciliation.

| Ordering | Required result / enforcing fixture |
| --- | --- |
| Original stop panics with a gated detached product request | Native physical drain alone cannot release private state; exact `FilePairingState` reopen stays `Locked` until original product permit returns; `shutdown_panic_retains_private_until_original_product_drain`. |
| Unpolled original stop is dropped with a gated request | The preinstalled retained owner uses the same backstop and no confirmation; same fixture's unpolled counterpart. |
| Original normal stop settles both drains and reconciliation | Guard is consumed without another effect; exact private reopen succeeds and original failure facts remain; normal counterpart fixture. |

An exact-key repeated owner approval that returns canonical `Active` is a
historical enrollment receipt. The route consumes that already admitted owner
result directly before generating IDs or requesting a new stage. It does not
infer current read authority from `Active`; all later reads ask current owners.
`owner_route_approval_uses_exact_claim_then_actual_registry_receiver_publication`
enforces this settled counterpart of O5.

A repeated panic from the original conversation storage/provider shutdown must
not unwind the retention-only parent task. That parent launches the existing
conversation stop in an owned child task and observes its typed join fault while
after reader/product/native physical drains have returned. It publishes no confirmation
or terminal effect. `shutdown_storage_panic_cannot_unwind_retained_product_drain`
uses a real `ConversationService` with its existing substituted storage port,
an observer fault during original storage shutdown and a gated product permit.
Retention first rejoins retirement/readers/product/native; its repeated storage
panic is contained after that permit returns. Private reopen stays `Locked`
before release, and the original retirement actor stays. This observer fixture
does not claim storage starts before product drain in the new N3 pipeline.


### Pending discovery candidates and current authority

The immutable `ConsentIntent::matches_owner` relation owns exact principal,
membership, audience, organization and composition gateway correlation. Candidate
selection grants no access: the selected record still passes current session and
Cedar admission through `AuthorizePairing`. The same relation replaces its existing
identity comparison; direct foreign status continues to refuse.

| Row | Ordering | Required enforcing fixture |
| --- | --- | --- |
| O10 | Two valid gateway resources retained in one registry | Select only current owner/resource candidates, then authorize; own pending survives, foreign state remains unchanged and direct foreign status refuses. |
| O11 | Same gateway contains different owners' invitations | Each current owner discovers only matching candidates; no caught IdentityMismatch-as-absence rule. |
| O12 | Selector matches but current session revoked/policy denies | Full admission refuses, no private history disclosure; matching selector alone is not authorization. |

The repeated storage-panic fixture observes an actual bounded reopen future while
the original product permit remains held, rather than inferring completion from
scheduler yields. That same reopen future succeeds only after permit release.
The normal stop counterpart drops the fixture's own prepared reference before
checking the lock, so the shutdown owner supplies the retained ownership.


### Serial maintenance and current expiry

One original serial maintenance task uses the existing current-state interval and
processes all current-gateway obligations within configured receipt/registry bounds.
Terminal cleanup-pending receipts do not consume the live-phase32 limit. Its owned
physical sweep retains the prepared private owner until the original sweep drains.
Stop is signaled before sleeping/draining and joins the sweep before final terminal
reconciliation. Independent maintenance failure remains distinct from listener
failure and requests whole gateway stop; neither notification confirms drain.

The invitation aggregate's `expire_if_due` shares its private expiry eligibility
predicate with explicit End validation. The store conditional method consumes it
under existing `commit_pairing`, sampling the actual clock inside the registry guard.
A stale due candidate becoming Active/Terminal/not due returns its current record
unchanged; no error-name allowlist or snapshot authority. Existing explicit end
callers keep their current semantics.

| Row | Ordering | Enforcer / runtime evidence |
| --- | --- | --- |
| M1 | Current eligible record not yet expired or Active after invitation expiry | Current aggregate preserves record, no approval/activation from time. |
| M2 | Due snapshot races Active/Terminal transition or backwards clock | Locked conditional expiry returns Existing unchanged; due eligible counterpart persists original System/Expired event once. |
| M3 | Original Stage/fence still owns physical dispatch | Cleanup StageOccupied remains pending, first cause retained; no second dispatch or invented absence. |
| M4 | More than32 terminal cleanup-pending receipts | Process entire receipt-bounded collection serially; no truncation or live-capacity reinterpretation. |
| M5 | Stop before tick or during sweep; waiter lost/panics | Wake original loops before wait, retain physical sweep and private lock until joined; no Drop reconciliation/confirmation. |
| M6 | Maintenance/listener independently fail | Preserve finite independent facts alongside conversation/browser shutdown failures. |

Hello uses volatile AvailableSetup.id solely as candidate selection. Exact canonical
resource and phase validation precede conditional expiry; a foreign candidate is
neither disclosed nor terminalized. Reacquire the original setup ID before public
projection. Reserve/finish remain full canonical proof owners after projection.

| Row | Ordering | Required outcome |
| --- | --- | --- |
| H1 | Exact current Available setup/resource/not due | Public Hello accepted without whole-history pending restoration. |
| H2 | Setup replaced/cleared during candidate read | Refuse original candidate, never silently select replacement. |
| H3 | Candidate now Claimed/Active/Terminal | Preserve current state, refuse Hello without expiry fiction. |
| H4 | Due snapshot versus backwards locked clock | Conditional owner preserves valid not-due state; projection rechecks current expiry. |
| H5 | Foreign resource candidate | Refuse before any conditional expiry effect or disclosure. |


#### Original physical overlap is a distinct storage outcome

`StageGate::acquire` publishes `PairingStoreError::StageOccupied` only when its
existing compare_exchange finds original physical ownership held. It no longer
publishes generic domain capacity for that physical fact. The same gate/lease and
Drop release remain the owners; no capacity rule or retryability changes elsewhere.

| Row | Ordering | Required result/evidence |
| --- | --- | --- |
| M7 | Original Stage/fence physical lease held, sweep asks exact cleanup | StageOccupied propagates from acquire_stage; only this typed case stays pending for next tick. Existing physical stage/SQL fixtures retain lock and original terminal cause. |
| M8 | Original physical lease drops | Exact current stage can be reacquired; no new approval or altered correlation. |
| M9 | Genuine Domain(Capacity), unavailable, identity/conflict or worker fault | Sweep returns finite failure and whole-stop notification; no generic retry/caught-error-as-absence rule. |

Wire/client finite refusal projections consume the same original storage error;
StageOccupied grants no admission or absence proof. Existing cleanup retains
canonical cause/initiator while overlap is pending.

#### Periodic sweep physical lifetime evidence

| Row | Ordering | Enforcer / fixture |
| --- | --- | --- |
| M10 | Actual periodic expiry blocks at injected locked clock; original maintenance waiter is lost | Physical sweep owns PreparedNative and private lock until the blocked closure returns. A bounded exact reopen remains pending before release and succeeds afterward; no Drop reconciliation/confirmation. |
| M11 | Original listener and maintenance tasks independently report finite failures | NativeRun drain joins both original tasks and preserves both facts. Private branch fixture covers outcome aggregation; it does not claim a real OS accept failure. |
| M12 | Periodic current expiry cannot publish to the actual temporary registry target | One finite maintenance notification requests whole stop; joined maintenance outcome retains failure and original Available record. Ordinary fixture replaces its own file target with a directory, restores it after physical drain, and makes no native failure internals. |

The ordinary M10 clock substitution blocks only the already-injected current-clock
port, after real invitation creation. It neither constructs a Published failure
nor grants pairing authority. All gates have bounded release and children drain.

### Approved protected-read composition sizing decision

Directly consume the current reviewed protected ReadRecords/ReadCatalogue
admission, lease, core page, encoder and client owners. Enrollment keeps its 4096-byte
payload phase; protected framing will consume the existing protocol request bound 65536 / generated response bound 131072
publications at the existing transport boundary. No streaming/fragments solely to
preserve the provisional 128 KiB target. This is a policy-sizing decision, not a
new claimed heap cap. The exact reviewed/merged gateway handoff precedes source
adoption. The explicit maximum-valid and retained-owner inventory below and existing hostile evidence
must support a revised finite composition ceiling before resource acceptance.
Raw/base64/JSON/TLS/parser/current-auth copies attributable to each connection
are included; KSF/stacks/baseline/kernel are separately reported. Native eight permits
retain completed-undelivered results as well as physical closures.

## 10. Combined native, passive-read and MCP shutdown

Approved before source integration against native5d57 and mergedc2f3. The following current assembly contract supersedes section9 D2 direct storage-shutdown chronology; original ownership/evidence tests remain their historical scope.

## One assembly and report owner

Existing `composition/shutdown.rs` owns combined orchestration, retention and serve-result composition; root wires original owners and signals Axum. Existing incoming `core/shutdown.rs` becomes the single typed evidence/aggregate owner; native duplicate ShutdownFailure struct in core/error.rs is removed into this owner. Do not preserve two shutdown protocols or duplicate passive_cleanup. Original ReaderWorkers/native/conversation/MCP owners continue implementing their own cleanup; assembly records their observed results.

One private ShutdownReport carries the actual evidence. Pending states distinguish not-started, awaited-but-unknown, observed outcome and deliberate skipped storage; None/absence is explicit successful no-owner case rather than an unknown result. PassiveReaderOutcomes keeps private fields and diagnostic deadline. Retirement/storage outcomes retain original typed ConversationError or contained PairingWorkerFault separately, without cloning/moving errors into a local future across the next await. Native serving/maintenance/worker/terminal errors remain separate existing NativeShutdownFailure fields; known observed serving failure is retained from stop initiation. Actual StoppedNative retains PreparedNative through reconciliation; an outcome snapshot never replaces physical ownership.

The private report publishes these phases through `ShutdownReport::Pending` and `ShutdownEvidence`: DrainPending{readers, retirement, native, product}; StoragePending{completed drains, retirement}; ServersPending{completed drains, retirement, storage}; ReconcilePending{completed drains, retirement, storage, servers_returned}; final Confirmed/Failed. Evidence is transferred synchronously in the original report mutex before each next await. Partial original drain evidence stays in the pending Draining phase; a report enum transition does not erase it. Core aggregate has interrupted-stage plus retained evidence, or nonempty completed failures; no publicly constructible all-success failure. Retirement states distinguish absent/not-observed, Ok, typed error and original child fault; storage distinguishes not-started due failed retirement, pending, absent, success and typed/fault outcomes. Native pending versus absent/observed joined outcome are distinct; product drain returned is an explicit fact. Exact enum names may follow existing module vocabulary; these meanings are the contract.

## Combined ordering table

| Row | Trigger / ordering | Evidence and physical ownership before next await |
| --- | --- | --- |
| N1 Stop initiation | Stop signal, original native serving failure or caller loss selects original owned shutdown task. Close product admission, signal Axum immediately and request native stop; start original owned conversation retire child plus reader/native/product drains. | Arm retention first, publish the pending Draining phase and original observed native cause before polling. Retire publishes original context/retired/provider stop before its first await. No storage shutdown yet. |
| N2 Parallel drains | Poll record/catalogue physical drain, original retire child, native shutdown and product request drain independently. Ready completions win diagnostic reader deadline; deadline records failure but retains/polls physical owners. | Each result actually observed by assembly is synchronously published before another await. Native-owned listener/maintenance/runtime facts are not invented before native owner reports; if that owner surfaces partial outcomes, publish them at that original reporting seam. No success inferred from dropped observer or timeout. |
| N3 Storage gate | Both physical reader drains, original retire result, native physical shutdown and product drain have returned. Only original successful retirement (or absent service) allows existing ConversationService.shutdown rejoin then storage shutdown. | Preserve completed reader/native/product/retire facts in the pending Storage phase before await. Failed/panicked original retirement leaves storage explicitly NotStarted; never retry it to replace first failure. Private native/read/MCP Arcs remain retained. |
| N4 MCP stage (NEW) | After storage returns, or is intentionally NotStarted from retirement failure, stop the injected original MCP servers; do this even conversations are absent, or retirement/storage have typed failures. | Publish ServersPending containing every known reader/native/product/retirement/storage outcome BEFORE McpServers.stop await. Pending MCP panic/cancel preserves these exact facts in interrupted aggregate. Unit stop return proves call returned under existing SDK contract, not separately typed physical MCP success. |
| N5 Native final reconciliation (NEW) | MCP stop has returned; actual native StoppedNative then performs existing original terminal reconciliation. | Publish ReconcilePending with servers_returned before await. Retain original private state/receivers through actual canonical CAS. Reconcile error/panic/cancel retains all earlier facts; retention fallback never writes terminal confirmation. |
| N6 Final report | All awaited stages return and original terminal reconciliation completes (or native absent). | Synchronously derive Confirmed only from successful required typed drains/retirement/storage/reconcile plus MCP call-return; otherwise a nonempty aggregate preserves independent failures. Disarm retention only after final publication. Browser serve error remains alongside shutdown aggregate through native ServeAndShutdown. |
| N7 Caller/task loss | Original task still owns cleanup after observer loss; if task/child faults, armed retention re-drains original owners with contained child faults. | Drop never confirms cleanup or performs terminal reconciliation. Preserve observed report facts; fallback may rejoin idempotent original retire/physical drains but never start storage before reader joins and original retirement success. Include MCP in retained owners. Repeated fault retains physical/private ownership rather than fabricating confirmation. |

N3 conservatively waits all initial physical drains before storage; it retains approved readers+retirement preconditions and avoids reporting native completion later than its physical join. Original admitted response/source permits, native8 connection retention and requests128/controls32 remain their own ledgers. No new capacity or authority state.

## Required combined regression contrasts

Actual held reader + blocked retirement: retirement begins at Stop, storage remains untouched until both retire and readers settle. Original retirement error/panic with physical reader late return: first failure retained, storageNotStarted, MCP still attempted. Successful retirement/readers with failed storage then pending MCP: typed storage error survives MCP panic/cancel. Absent conversations plus pending MCP keeps report pending. Known native/reader error + successful storage then pending MCP retains every known fact. MCP returned + pending native reconcile keeps private reopen pending and report unconfirmed; reconcile error retains previous failures. Original browser error remains alongside cleanup failure. Retention observer/task loss and repeated child panic cannot publish terminal confirmation or release retained physical state early. Existing physical read leases, allfive passive busy routing/deadlines and positive delivery stay covered separately.

## Representation and phase framing API decision

The approved current producer direction keeps `NativeTransport<S>` as the TLS/private possession-proof owner and moves complete application framing to one server native codec, removing the old auth-crate frame implementation in that same change. Enrollment frames remain 4096 and cumulative TLS handshake ingress/egress accounting stays unchanged. The codec consumes the existing protocol request bound65536 and generated response bound131072 for directional protected checks before input allocation/output. Only a private successful authentication factory yields the gateway protected channel; no caller maximum or authority flag. The client changes phase only after correlated ready over its pinned live TLS transport. A raw Read/Write adapter does not perform a WebSocket HTTP upgrade on the native profile.

| Phase / state | Entry and output ownership | Cancellation, failure and next transition |
| --- | --- | --- |
| TLS pending | Original admitted native physical closure owns the same actual transport, handshake deadline and one of eight permits. | Signature/ALPN/pin/deadline failure returns typed failure through that original completion owner; caller loss does not return the permit before physical work and retained completion end. |
| Enrollment | The single native codec allows only bounded enrollment/status messages under original cumulative enrollment deadline; successful pairing publishes its original credential/CAS facts. | Enrollment success grants no protected action. Error settlement stays with the original pairing owner. No deadline reset can replace an observed attempt failure. |
| OpenProduct selection | Bounded unauthenticated first-message selector in the existing native wire union enters the existing product challenge exchange; it changes neither ceiling nor authority. Enrollment Hello/Status keep their own path. | Selector alone cannot construct protected state. Server establishes one finite auth-pending deadline and close/wake policy; message trickle does not renew it. Client carries one absolute handshake budget across TLS, selector, challenge, correlated authentication and ready. Malformed/oversized selector and refused/mismatched/replayed nonce have no source effect. |
| Product authentication pending | Read the bounded authentication envelope on this same live TLS transport. Registry.device_verifier borrows this transport.device_proof plus the registry through AuthenticateSession.execute await; current access/clock/audience remain injected. | Failure retains typed cause and closes without protected construction. The borrow ends before transport is moved; no static verifier, copied proof DTO or public transport-plus-identity constructor. |
| Gateway protected | Private factory consumes the exact transport and the actual AuthenticatedSession returned from authentication, then writes ready. Single codec permits published request ingress/response egress bounds. | Identity grants no action; all five passive methods retain fresh current-session/action/source admission, exact scope/epoch/origin, physical source lease and existing refusal/delivery ownership. |
| Client ready pending | Client owns pinned TLS, actual credential evidence and one correlated authentication attempt. It validates generated ready plus exact identity/origin/capabilities before protected construction. | Wrong correlation/shape/identity/refusal/deadline closes the attempt and preserves first failure. Raw gateway key or caller success flag cannot construct the protected client session. |
| Finite application pass | One public transport-neutral application session port owns immutable typed requests/responses and the only message reader. Existing source/authorizer facades borrow it for one begin/finish pass. | The published client operation floor spans server read and delivery deadlines. Cancelling the pass ends caller interest; it does not assert remote physical completion. A live connection may serve later passes only under fresh admission. |
| Live protected connection | Reuse the original eight native physical connection permits and retained completion owner for the full protected connection lifetime; a persistent connection consumes a slot, so other accepted sockets receive actual capacity refusal when all eight are occupied. | No new permit ledger or unbounded connection admission. Connection lifetime is separate from a finite operation deadline. Idle blocked reads must be explicitly awakened on original shutdown/revocation/close before this phase is activated. |
| Closing / draining | Original owner closes admission, requests actual transport shutdown, retains connection/source tasks and reports only observed outcomes under N1–N7. | Caller cancellation or a diagnostic timeout cannot return a live physical permit. Original completion/terminal reconciliation decides outcome; Drop cannot confirm it. |

The current enrollment implementation only closes its semaphore and waits for physical closures bounded by enrollment deadlines. It has no explicit wake for a persistent protected read. Protected implementation must therefore add actual socket-close ownership to that SAME native lifecycle before replacing the enrollment deadline with a live connection state; otherwise shutdown could hang indefinitely. Close/wake is a physical effect, not a second capacity ledger. Its cause (admission retirement, explicit close, revocation or transport failure), attempt result and independently observed physical join remain separate typed facts in the original connection/native outcome owner; successful wake is never a join or terminal CAS confirmation. Its held-read, cancelled-observer, failure and later-drain regressions are required before activation. Retained native serving/worker failures stay in the existing native outcome/report owner.

The native live adapter consumes the existing generic product `run_authenticated` Stream/Sink boundary and its writer/admission/reaping implementation. It does not implement a second dispatch or deadline loop. The adapter owns one TLS stream with nonblocking physical readiness, one bounded in-progress prefix/body and one bounded output offset/flush state. It must attempt buffered TLS progress before waiting for physical readiness; a blocking read holding shared TLS ownership cannot gate ready delivery. The original native physical closure owns this adapter until the existing loop and all actual retained work drain. Directional ceilings are checked before input allocation/output. The auth pairing pure contract publishes MAX_ENROLLMENT_MESSAGE_BYTES once. Raw OPAQUE message length and the encoded enrollment whole-envelope length each consume this value at their actual owner; fitting raw bytes does not waive JSON/base64 envelope overhead.

The native mapping consumes manifest-published `session.terminated` with the existing generated SessionTermination shape: write that ordinary event under the directional output deadline, then physically close even if delivery fails. Browser CloseFrame presentation remains current. The same native client reader validates this event and records typed Closed(reason); real EOF without a valid termination retains honest disconnected/unknown meaning. Authentication failure uses the enrollment/auth-pending ceiling and cannot construct ready. Termination does not authorize deleting cached data or infer current scope revocation. Duplicate/malformed termination, every valid reason, delivery failure before actual close and plain EOF require distinct fixtures. WebSocket bytes are never written to this profile.

The calling-side application port lives in `read_only_sync/application/client_session.rs`; product continues owning wire/routing and selectively publishes the same generated DTOs/codec. The port owns the one current GatewayError/Cancellation/attempt outcome meanings; no native duplicate error or renewed per-RPC operation budget. Generated DTOs remain untrusted wire values. Public request/response wrappers validate through current codec/scope owners and retain private fields with immutable access; exposing a generated public-field DTO alone is not this application contract.

| Consumer ownership state | Borrow and evidence owner | Finish, drop and next pass |
| --- | --- | --- |
| Session connected | Private authenticated transport factory owns one live reader, ready identity/origin/capabilities and injected cancellation/clock/policy. | No public raw transport-plus-ready constructor. Connection may outlast a finite pass, with current server admission on every request. |
| Begin pass | `ProductReadPass` takes the sole mutable session borrow and creates one original attempt/deadline. | Reject a concurrent pass; never reset deadline/event budget for an RPC. Begin failure yields no live pass and preserves actual transport cause. |
| Paired source/authorizer views | Both core facades borrow the same pass for their full lifetime and share its private single reader/outcome; internal interior mutability is not an exported Rc/RefCell session. | Views cannot outlive the pass or allow finish while still used. Shared RecordSource and ScopeAuthorizer still refer to that same original scope and deadline, not independent requests/ledgers. |
| Request/response | Immutable validated request wrapper encodes through the actual generated codec; correlation and immutable response decoding precede existing scope/epoch/origin/schema validators and any core/cache effect. | First transport/protocol/auth/source cause remains in original attempt. Partial or foreign correlation closes the original transport; a later pass cannot consume that buffered reply. |
| Explicit finish | Consume the pass only after its borrowed views end, returning its original operation/outcome. | Reuse connection only if no unresolved response or failure remains. No successful empty outcome can erase first failure. |
| Cancel/drop/panic | A dropped unfinished pass settles its original attempt as cancelled unless a prior cause exists and closes its actual transport, including partial read/write/correlation state. | No implicit successful finish, inherited pending reply, new budget or remote physical completion claim. Server retains actual source/connection leases until joined. A later pass requires usable actual transport or a new authenticated factory. |

Public acceptance requires actual extraction/adapter tests proving the paired core facades borrow one pass plus compile/lifecycle evidence for finish/drop; the names/table alone do not prove this contract.

Public application port publication must state finite pass versus connection lifetime, single-reader correlation/event bounds, immutable payload and close/error meaning. Client and gateway constructors own their respective authenticated phase proof; private Rc/RefCell WebSocket Session is not a public/native API by assertion.

ResponsePayload transparent Deserialize plus immutable decode<T:DeserializeOwned>(&self) owns erasure representation. Session rpc/response_payload carry owned wrapper; ready shape borrows then decode; source facade typed decode precedes existing scope/epoch/origin/schema validators and cache effects. Events retain current Value path. Same envelope decoder owns unique keys/presence/correlation before conversion; no intoValue escape or copied validation. Y1–Y6 and valid neighbors apply. Absolute deadline/caller-interest cancellation remain current owners; timeout/drop does not free physical source/connection leases.

Protected128KiB whole-owner acceptance is explicitly impossible under current full131072-byte frame plus liveTLS/parser/wrapper/DTO/auth owners. Canonical device-pairing already approves revised finite policy after the explicit actual maximum-valid/retained ownership inventory and platform evidence. No guessed larger ceiling, shared wire lowering, omitted overlapping allocation or streaming solely for128. Pairing evidence remains its own scope; protected activation waits this resource decision/evidence. Gateway watches are separately owned capacity policy, not native budget allowance.


### Protected resource policy ownership inventory (source audit)

The measured cohort is evidence for its actual owners, not a maximum policy.
A revised policy must distinguish the bounded current producer lifetime from
caller-owned outputs retained across successful passes and shared storage
state. Composition cannot infer a hard bound for these three different
lifetimes from the same eight native connection permits.

| Owner | Actual published bound / admission | Retained representation and release | Remaining maximum-valid evidence |
| --- | --- | --- | --- |
| Native physical connections | Original `NATIVE_CONNECTION_CAPACITY` eight; same enrollment/protected owner | TLS, parser, output and completion stay owned until original physical join; wake result is separate | Cohort observes all eight; provider/OS allocation is outside Rust tracker |
| Native framing | Enrollment raw message4096; protected incoming65536/outgoing131072; actual directional codec | Raw encoded body plus sequential strict decode, decoded wrapper/DTO and one pending encoded output can overlap; cap checks are not allocation-total checks | Method-valid maximal input, malformed correlatable envelope and serializer transient must be sampled with current writers |
| Generic product dispatch | Original per-connection ordinary16/control4/passive1; global request128/control32/physical-read4 | Actual task, decoded params, response and real slot are retained through their original completion; passive physical lease can outlast caller response | Read-grant route denial is verified; maximum allowed ordinary response and deferred/control/refresh overlaps remain distinct |
| Writer and refusals | Existing ordinary/control channels16/4, generic refusal channel1; passive channel/pending/active positions with original absolute deadline | Encoded cap applies after ordinary serialization. Permit-free refused responses retain inbound-bounded unvalidated correlation IDs; do not count them as slot-owned | Maximum correlatable refusal plus active/pending/channel and simultaneous control/ordinary writer states remain required |
| Authority | Composed LocalCredentialStore uses injected max_registry_bytes (default4MiB), max_credentials1000, max_receipts2000; native device verifier restores an active DevicePairing credential | Original registry read/decoded registry/restore and current snapshot allocations are separate from frame bounds. Persisted pairing validates exactly one grant; generic Credential/AccessReader alone has no grant-count bound | Configured registry maximum and independent deferred-input/periodic/passive checks must be charged together; do not treat generic unlimited grant construction as native-admitted |
| SDK physical records | MAX_STORED_RECORD_BYTES1MiB; source original physical read owner and bounded pages | Physical source allocations, retained old caller outputs, encoded response and decoded client outputs may overlap; lease ends only after actual work/response ownership ends | Actual cohort/held worker strengthens this overlap, not production catalogue maximum |
| Shared committed-view cache | Source owner publishes64 entries and256MiB admission/eviction threshold, queue64 and remembered heads64 | Threshold is checked for cache admission; an existing pinned/current fold may grow beyond it. Source tests explicitly cover retained bytes above threshold. This is not a256MiB absolute heap ceiling | Maximum current fold/shared storage must be bounded by its own domain/source contract, not omitted or assigned per-native connection |
| Caller-owned outputs | Public immutable response output has published individual wire/method bounds; there is no maximum number of successful historical outputs a caller can retain | A caller can keep prior outputs while beginning later successful passes. Physical connection/pass permits bound in-flight work, not that historical collection | Whole application acceptance requires the actual calling owner and retained-output policy; a producer-only ceiling cannot claim arbitrary caller retention |

Source owners. Implemented native enrollment: server
device_pairing/infrastructure/connection/wake.rs and
infrastructure/enrollment_channel.rs. Existing files whose native protected-read
role is proposed, not built: server product/socket.rs and protocol/json.rs. Auth
adapters/local/registry.rs and registry/pairing/{device_verifier,projection}.rs;
SDK infrastructure/session_storage/{record,record_source}.rs. The global
requested-live Rust tracker measures actual cross-thread owned allocations
within its declared scope; C/provider/kernel/stack and pre-scope baseline remain
separately accounted. Neither a peak measurement nor the cache admission
threshold supplies the missing maximum contract. No new numeric policy, wire
ceiling, route restriction or capacity ledger is introduced by this audit.

### Protected producer composition ordering

The existing native listener receives its immutable product route state at construction, after local product composition finishes. Enrollment and `OpenProduct` share the original eight-slot owner. `OpenProduct` reuses the product challenge and authentication-envelope validator; the native factory supplies only the verifier borrowing this same channel's TLS proof. After authentication, the same current-identity/ready owner runs before the generic product receive/writer loop. Ready remains an enrollment-sized envelope until its correlation/shape (and native credential ID) has been validated; only then may the client accept the published protected response ceiling. The selector starts one auth-pending deadline bounded by the already-owned enrollment deadline and product handshake setting, never renewed by later messages.

| Adapter state | Actual retained owner and transition | Physical readiness / cancellation |
| --- | --- | --- |
| Auth pending | Enrollment codec and original absolute enrollment deadline retain the live TLS channel; selector and every auth byte consume that deadline. | Original socket retirement wakes this owner even while blocked. Selector cannot change the ceiling. |
| Ready publication | Successful private proof factory retains the channel and authenticated session; current identity is rechecked before ready. | Failed authentication/current identity emits the shared correlated typed error under the enrollment ceiling when delivery is possible, then closes without protected dispatch. Timeout or failed error delivery makes no invented typed reason; ready write failure also closes without dispatch. |
| Protected IO | One TLS stream, one incremental frame parser and at most one pending output frame feed the existing generic product loop. Original native slot remains held. | Set the actual socket nonblocking only after auth; poll TLS buffered progress before registering read/write readiness. Partial prefix/body/output survives WouldBlock. Each poll permits finite physical TLS ingress/egress work derived from the existing directional frame publications; exhausting that scheduling quantum explicitly self-wakes and yields, rather than pretending a socket edge is absent. Continuously readable TLS control traffic cannot monopolize one synchronous poll. The real rustls fixture interleaves up to 32 actual KeyUpdates with one partial application-frame byte: the library rejects a KeyUpdate-only flood, but application data resets its update counter. Quantum exhaustion retains the partial parser/TLS state and self-wakes; another writer, deadline or retirement branch must progress before the partial frame completes. The quantum changes no frame acceptance ceiling. No renewed connection deadline. |
| Product close | The existing schema generator emits the termination wire validator from its current finite enum and retry-delay bounds; close-policy metadata remains with that same generator owner. Existing termination payload becomes `session.terminated`, then the original physical socket closes after the final attempted flush. | Flush failure remains an IO failure; an EOF without a valid termination conveys no typed reason. Original wake/drain and final CAS remain separate evidence. |


The existing native entry owns authenticate_product separately from its subsequent generic product loop: selector admission, nonce, actual same-proof verification, current identity refresh and correlated ready occur once in this helper. It returns the same sealed channel and session only after those effects; serve_product consumes that result in the existing dispatcher. Owning polling fixtures retain the original registered native closure/permit while consuming the actual successful factory result, rather than constructing proof-bearing state.

The client socket sum type retains exactly one boxed WebSocket or boxed native TLS owner; those actual box allocations belong to its retained/maximum-valid measurement. An authentication refusal boxes the original pending server channel only for its bounded reply, preserving the same proof/transport ownership rather than copying it. NativeAuthentication groups the borrowed actual identity, saved exact pin, credential selector and client metadata at the private adapter factory; it cannot select authority or a frame ceiling.

The native client producer derives its identity and exact gateway pin from the original durable pending owner, then authenticates the caller-selected credential ID on that pinned live TLS connection. A credential ID supplies no possession evidence. The existing one-client physical permit and private state owner remain retained by the returned consuming session; enrollment and protected connection creation cannot race another client operation. Client shutdown closes that original admission and synchronously wakes its registered physical endpoint before awaiting the same permit drain. The fixed original endpoint owner retains every attempted wake syscall outcome, successful or failed, with the original peer target and admission-retirement cause. The native application lifecycle data contract owns the finite sweep representation and the connection-capacity publication; infrastructure performs the actual syscall. Composition copies that immutable sweep snapshot into the existing ShutdownEvidence synchronously before any cleanup await; successful cleanup retains the SAME evidence box in Confirmed, while Unreported means no observed cleanup; failure aggregation derives from the same snapshot. A completed sweep with no active endpoints is explicit, and neither successful wake nor an empty sweep confirms physical join. Client exposes the same original-owner snapshot independently of later permit drain. Dropping/cancelling a consuming pass physically closes its original stream; no helper renews the handshake or pass budget. `ProductClientPolicy` is an immutable public value containing the existing private physical `GatewayPolicy` and requiring the published passive request floor. The private physical owner still supports shorter configured budgets, as its existing infrastructure deadline fixtures exercise; no test-only constructor creates an otherwise forbidden public policy. Public pass lifecycle fixtures over that private transport prove borrow/first-cause/closure behavior, not public budget acceptance.


### Allocation evidence owner

The process-wide test allocator is shared by the existing cache cumulative measurement and the native retained/peak measurement. The native mode observes successful Rust allocations/reallocations and their actual deallocation across all threads of one isolated filtered run; fixed instrumentation bookkeeping is separate from the measured native heap. Tracking overflow invalidates the sample. A scope begins before constructing its measured TLS/client/private/queue owners and snapshots while those owners are retained, then observes release. This tracks requested live Rust allocation sizes, not allocator metadata or an internal System realloc copy that is not exposed as two live pointers. Platform/resident evidence and conservative owner overlap accounting must cover those limits. This does not measure C allocations, stacks, process baseline or kernel memory; those classes require their own source/platform evidence. Neither the old thread-local cumulative byte counter nor nominal frame serialization establishes a native whole-owner ceiling.


### Explicit protected allocation inventory

The earlier unenumerated “eight classes” planning count is superseded by these actual owners. Maximum-valid representation measurements and retained-cancellation measurements are separate scenarios; a nominal malformed frame at the wire ceiling is not a maximum-valid DTO.

| Actual owner / variant | Direction and overlapping representations | Maximum-valid and retention witness |
| --- | --- | --- |
| TLS channel, pending handshake | Original stream, rustls ingress/egress buffers, key/proof/exporter; cumulative handshake publications remain 4096 each. | Actual accepted handshake plus bounded failure; Rust allocator and separately identified provider/native allocations. |
| Enrollment | Raw OPAQUE message, complete JSON/base64 envelope, canonical context, parser and durable pending/claim receipt owner. | Valid adopted crypto maxima within the whole enrollment envelope, not only raw-message length. KSF workspace separately owned. |
| OpenProduct / challenge / authenticate / ready | Unauthenticated selector and auth-pending frames stay 4096; real channel proof verifier borrows registry, current access snapshot and ready/error envelope overlap. | Largest valid current shape and actual expiry/refusal. Client phase changes only after correlated ready and credential identity validation. |
| Protected input | Four-byte prefix and complete request ≤65536, UTF8/frame unique parser, generated params and admission selectors. | Largest valid admitted request, including escaping/unicode overhead; partial body/caller loss retains its actual parser. |
| recordsHead | Encoded response ≤131072, decoded scope/head and validator copies. | Largest valid current scope/head; foreign scope/epoch refuses before effects. |
| recordsPage | Physical SDK/core page/raw payload, base64 conversion, encoded response ≤131072, parsed payload and immutable typed/core page. | Largest valid current page and record boundaries; source-held, queued output and client decode overlaps are charged. |
| catalogueHead | Encoded response, catalogue scope/head and current authority. | Largest valid current head and current selector validation. |
| catalogueManifest | Current descriptor list, domain page, JSON strings/array, encoded response, unique parser and typed manifest. | Largest valid manifest allowed by both descriptor cardinality and encoded ceiling; no invented full-size malformed substitute. |
| catalogueResolve | Current resolved descriptor/projection payload, domain result, JSON/base64/typed decoded representation. | Largest valid resolved entry and actual too-large refusal; current correlation/origin/schema remain enforced. |
| Current authority | Device verifier borrows original registry; each current access snapshot and current grant/receiver selectors remain actual admission owners. | Configured registry/receipt/credential bounds, largest valid current snapshot; shared baseline distinguished from per-connection retained copies. |
| Admitted source / record delivery | Original global read lease plus original socket slot, source closure, response channel/pending/active writer and deadline ownership. | Physical source held after typed timeout, result undelivered, output backpressure, dropped caller and later original join; every live representation charged once. |
| Busy refused delivery | Permit-free record lane active/pending/channel positions under original inbound/response limits; physical admitted source can remain held independently. | Refusal queued alongside retained physical work; finite overflow/closure contract and transient serialization included. |
| Native lifecycle / consuming pass | Eight original native slots including retained completed results; real client TLS/reader/private owner/client permit, one finite pass and paired borrowed core facades. | Eight authenticated connections held through original retirement, dropped waiter/replacement drain, pass cancellation and buffered partial response; successful wake is distinct from physical completion. |

Any final finite ceiling must include all applicable overlapping attributable owners above, measured at their actual maxima and retained transitions. The isolated live Rust allocator sample includes all participating Rust threads in its selected process; C/provider allocations, stacks, common baseline and kernel buffers are explicitly separate evidence classes. Overflow invalidates the Rust sample. No resource acceptance follows from merely running a fixed number of fixtures.

Allocation instrumentation serializes each actual System allocation/reallocation/deallocation and its fixed pointer-table observation under the same fixed nonallocating measurement guard. macOS standard Mutex lazily allocates its pthread owner and therefore cannot be called by this global allocator; an AtomicBool guard exclusively borrows the static UnsafeCell table, with release on every guard drop. This guard serializes instrumentation and its System calls; its interference excludes execution-performance claims. This prevents a cross-thread pointer reuse from racing successful realloc bookkeeping. The table uses no allocation; the scope includes new successful Rust allocation requests on all threads, with pre-scope owners and C/provider/kernel/stack memory explicitly outside it. Instrumentation is isolated allocation evidence, not an execution-performance measurement.

### Actual native socket fairness fixture ordering

The owning fixture calls the same private `authenticate_product` used by
`serve_product`, after the original connection slot and wake endpoint are
registered. Only its successful sealed channel constructs the socket. The real
TLS client prequeues more KeyUpdate ciphertext than the published physical read
quantum, interleaving application bytes to reset rustls' consecutive-update
limit. A marker follows the actual writes, before the server polls the socket.
The first socket poll must yield with its original partial frame intact and
self-wake; a separate writer flush and runtime deadline can then progress. The
client completes that same frame afterwards and the original physical worker
returns before its permit is released. This fixture observes the owning adapter,
not fresh source authorization or the generic product dispatcher's deadlines.
Its test-only peer module is attached once at the crate test root so both the
private owning fixture and the real enrollment fixture use the same TLS producer.

The listener fixtures retain idle peer sockets through successful retirement:
real syscall wake, rather than peer drop, now ends blocked TLS work. A separate
listener-settlement fixture holds the actual completed connection object (and
its original permit) behind a fixture-owned completion gate. Failure reporting
must precede that original completion return; opening the gate permits its join
and release. This gate delays test completion ownership, not production TLS or
shutdown, and does not claim the physical worker is still executing after wake.

### Passive payload/count allocation fixture

The next isolated fixture installs source-port producers behind the existing
actual receiver authority and a real stored conversation owner, then completes
normal enrollment/approval/native authentication. It returns correlated valid
record head/page and catalogue head/manifest/resolve values at their published
payload/count ceilings. Fixed authenticated gateway/owner facts come from that
live session; this is not a claim that every identity field is simultaneously
maximized. Source-port producers prove application/codec/client representation
ownership; production SDK/source-worker sizing remains a separate evidence
class. Samples hold the original client/TLS/pass plus returned record page,
manifest and resolved bytes, then observe release. JSON/base64/decoded/TLS
transients are captured by the same process-wide peak instrument, with existing
provider/OS/pre-scope exclusions. The caller must assert every requested method
actually succeeded before reporting its allocation sample.

The SDK physical sizing case separately writes a valid large accepted input
through public `SessionStorage::save_changes`, obtains the actual storage
identity, and reads its published maximum tagged physical piece through
`NessaRecordReadSource`. Holding the actual returned `RecordReadResponse` retains
its original opaque lease after the SDK worker has joined. A second existing
identity-gated physical worker loses its caller while retaining that same lease;
its source shutdown remains pending until the owning gate releases. This fixture
uses the existing private owning gate, not a native/socket authority constructor.
Its scope is actual SDK/source ownership and requested live Rust allocations,
not simultaneous authenticated native codec ownership; integration overlap still
requires a combined conservative bound or an actual combined fixture.

The sizing fixture retains a typed clone of its original
`Arc<LocalReceiverAuthority>` beside the two application trait views. Pairing
activation and passive admission therefore consult the same originally composed
instance and database; no second authority object, caller-selected binding or
cast from an unrelated port is introduced.

### Raw protected route owner inventory and trace correction

The public finite pass exposes the five passive methods; raw authenticated native
transport feeds the full existing product loop. Its original per-socket owners
are 16 ordinary slots, four control slots, one passive slot, one deferred input
with its independent authority check, and one periodic refresh. The loop stops
reading later input while that one deferred input waits; those bytes/decoded
params must still be charged. Admitted task state and retained response state use
the same existing slots, not an additional native ledger. Refused passive
responses retain the canonical bounded active/pending/channel positions.

An initial sizing trace incorrectly treated `conversation.list` as admitted by
the native read grant. Actual `action_for_method` maps both `conversation.list`
and `conversation.read` to `conversation.write`; only the five passive methods
map to `conversation.read`. Cedar forbids missing credential grants regardless
of the owner's admin role. Their 500-row list bound is not a reachable response
class for this read-only credential. The raw fixture must observe correlated
`forbidden` for both before conversation dispatch; source absence would instead
return `conversations_not_configured`, so it is not substituted for authorization.

Reachable ordinary raw replies include `auth.session`'s current ready publication,
`already_authenticated`, `unknown_method`, malformed/invalid request refusals,
and missing-grant `forbidden`. The same control admission can retain rejected
write methods. Transport Ping/Pong message variants are ignored as before;
there is no native ping RPC inferred from them. Native response framing consumes
the same published record ceiling, while the original ordinary encoding owner
still enforces its smaller ordinary ceiling. Final whole-connection sizing must
account for these raw/decoded/current-auth/task/serialization owners together
with protected passive work; the five successful pass methods alone do not prove
that complete inventory.

### Combined SDK physical response and native codec sample

The next simultaneous sample replaces only the record source-port producer with
`NessaRecordReadSource` over actual `RecordStorage`. Public SDK save operations
publish a valid large accepted input; the authenticated native public pass reads
one maximum tagged physical piece after the initial storage records. Catalogue
head/manifest/resolve remain the explicitly identified payload/count source-port
fixture. The original SDK source, native listener/TLS session, finite pass and
returned decoded record/catalogue outputs remain live in the same allocation
scope and at the held sample. Server native drain precedes SDK source drain and
storage shutdown; clients and decoded outputs remain held through those awaits.
This tests their actual overlap, not the sum of separate allocation peaks. It
does not substitute for a simultaneously running cancelled SDK worker, maximal
production catalogue metadata, all eight loaded native connections, or retained
ordinary/control authority tasks. Those owners remain in the finite sizing
inventory and must be accounted for before publishing an accepted ceiling.

The simultaneous owner inventory used to close this gate is source-backed:

| Retained owner | Existing finite owner / representation | Remaining sizing distinction |
| --- | --- | --- |
| Native TLS/parser/output | Original eight connection permits; 65536-byte inbound frame, 131072-byte outbound frame and four-byte prefix; `FrameRead` plus at most one `Output` | Parser/current plaintext, outgoing frame and provider buffers can overlap; payload-only counts are insufficient |
| TLS buffers | Locked rustls 0.23.45; native `complete` applies its existing 64-KiB send buffer limit; rustls separately owns the received plaintext and deframer limits | Send limit is not an ingress or total allocation cap; buffer capacity/provider metadata and handshake retention remain charged |
| Ordinary/control admitted frames | Original 16 ordinary and four control socket slots; shared route state has 128 request and 32 control permits | Raw `RequestFrame.params` remains a bounded decoded JSON value while original authority/dispatch work waits; rejected write/control methods are still retained work before denial |
| Deferred input / authority | One `pending_input` and one independently polled input authority check, plus one periodic refresh | Reading is disabled while that deferred input exists; current access snapshots and decoded params remain charged |
| JSON decoding / correlation | Native inbound ceiling and existing `unique_value`/`unique_envelope` item budget in `protocol/json.rs` | Strict decoding and failed-frame correlation construct their trees in sequence, not two simultaneously retained trees; strings/arrays/maps allocate beyond encoded byte length; the item budget is a backstop, not a heap ceiling |
| Passive physical work / responses | Original four shared record permits and one per-socket slot, retained by the original opaque physical lease and response owner | Caller loss and delivered timeout do not free physical work; SDK body/page and undelivered application/encoded representations can overlap |
| Refusals / writer | Existing ordinary/control response slots and ordinary refusal channel; passive active send, pending response and one channel position | Permit-free passive refusals retain bounded unvalidated IDs under inbound frame limits; encoding may allocate before the ordinary encoded ceiling is checked |
| Client pass / returned values | Original single reader/correlation owner and one finite-pass budget; application output values are retained by their caller | A caller can retain completed outputs outside subsequent passes; samples state exactly which outputs are held, rather than imposing a new caller memory policy |
| SDK / catalogue storage work | Actual SDK stored-record ceiling and bounded physical page; existing production catalogue metadata/admission owner | Source-port catalogue payload maxima and real SDK physical pieces are separate classes; real production metadata and source-held cancellation require their own overlap evidence |

These are ownership facts, not a sum declared to be a heap cap. The isolated
allocator scope also includes fixture database/enrollment setup and actual
client login KSF. Those measured peaks cannot be divided by connection count or
attributed to only the protected connection. Final sizing records those classes
separately and keeps the already stated C/provider/OS/stack/instrument exclusions.

### Protected connection cohort allocation scope

The next eight-loaded SDK/output cohort starts the same global Rust allocation
scope after actual enrollment and activation, before constructing any protected
client session, server TLS channel or physical read for that cohort. Registry,
receiver/repository, listener, saved enrollment and SDK storage/source setup are
explicitly pre-scope baseline owners and remain separately charged; the earlier
whole-fixture scope is preserved rather than renamed. Actual protected TLS,
reader/pass, physical read transients and decoded caller outputs created after
scope start are tracked across their original threads.

| Cohort stage | Actual ownership / order | Evidence limit |
| --- | --- | --- |
| Start | Actual enrollment/activation finished; original listener and published credential exist; start the sole instrument before protected construction | No division of the earlier KSF-inclusive peak and no claim that baseline/C/provider/OS allocations were measured here |
| Load eight | Each authenticated client performs the same SDK physical page plus identified catalogue source-port maxima; observe its completion marker before constructing the next protected peer | Original eight connection slots are used; sequential completed reads do not claim eight physical read permits or concurrent source work |
| Hold | All eight original TLS/session/pass owners and decoded outputs remain behind the structural release gate | Caller output retention is explicit; production catalogue metadata and retained raw authority tasks remain separate inventory classes |
| Retire / drain | Original admission retirement wakes and joins all native owners; original SDK source/storage drain returns while all caller outputs and clients remain held | Wake is not physical join; the source operations already completed for this cohort, not a running-cancelled SDK worker claim |
| Release | Release and join all original client threads, then sample actual output/session drop | Requested live Rust cohort allocation evidence only; full finite/platform acceptance still requires all charged owners |

### Retained SDK work after original client retirement

The owning SDK test module may provide one crate-private, `cfg(test)` fixture
wrapper around ordinary `NessaRecordReadSource::new` and its existing private
before-identity gate. It installs that same original gate, opens it for the
initial SDK reads, and structurally releases it on fixture drop. This is a
controlled external IO wait on the actual retained OS worker; it adds no
production constructor, authority flag, source result switch or capacity ledger.

| Retained cohort stage | Original owner / ordering | Required meaning |
| --- | --- | --- |
| Load | Same eight real authenticated peers retain valid SDK/catalogue outputs; all initial record leases returned | Published native capacity is consumed by actual connections; setup baseline and catalogue source-port scope remain explicit |
| Hold one SDK read | Close the existing owning gate, then one original public pass issues another real physical page; observe the actual worker entry | One original global/per-socket read lease remains held before SDK identity work; it is not a completed maximum payload claim |
| Retire original client | Call that client's original `shutdown`, which wakes its actual endpoint; consume the pass's existing `Closed(None)` (unknown EOF) or `Transport` outcome and drop its session while decoded outputs remain held | These are actual physical closure observations, not authority revocation, typed cancellation or remote-source completion; the original client lease must return |
| Pending source join | Original SDK source shutdown remains pending behind its gate; drop that waiter while the original join remains owned; native retirement/drain may finish independently | Physical native IO return does not release the SDK read lease; no successful source join is published yet |
| Release / join | Release the original gate, await replacement source shutdown, observe the original global permit returned, then close storage | The actual SDK page executes after release and its original physical/source callbacks settle; result disposal after caller loss is charged with the held decoded outputs |
| Drop outputs | Release and join the original caller threads | Caller-retained outputs and any still held client objects actually drop; the same scoped instrument observes release |

The held-before-identity marker proves a running original worker and lease, not
that the SDK has already decoded a maximum physical result. The subsequent
actual SDK execution supplies the physical page/serialization transient while
the earlier caller outputs remain retained. The separate after-operation
response-held SDK evidence keeps its distinct meaning. Native wake reporting
counts actual live targets/calls; a client already physically returned must not
be fabricated into another retirement wake or physical confirmation.


### Current admission selectors and sole crypto owners

The c4 compile and eleven focused
baselines are historical evidence; they do not
credit rule removals or current full gates. The previous 77-group planning map
also had misassigned constructor and handshake-budget ranges, plus one nested
helper mistaken for a test. The corrected source map preserves those mistakes
and binds each candidate to its actual owner before further probing.

| Input / order | Original enforcement and typed result | Detector / valid counterpart |
| --- | --- | --- |
| Oversized otherwise-valid KE1 arrives at `ServerInvitation::start` after invitation/gateway selectors match | The same `decode_request` owner used by public fingerprinting refuses `InvalidProof` before ServerLogin; remove the copied outer size comparison. This intentionally routes oversized crypto input from the previous `InvalidContext` to the decoder's `InvalidProof`; selector mismatch still refuses `InvalidContext` | `credential_request_bound_refuses_padded_valid_ke1`, plus `server_start_uses_original_request_decoder_refusal` |
| Peer chooses a signature scheme other than Ed25519 | Original mandatory raw-key signature verification compares the parsed fixed Ed25519 SPKI algorithm with the selected ring signature algorithm; original verifier advertises only Ed25519. Remove the copied local scheme predicate, retain actual signature verification and raw SPKI/pin validation | `native_tls_refuses_corrupted_peer_signature_and_accepts_original`; constructor/verifier provenance, not a private fabricated completed transport |
| Original native transport reaches completion | Only `accept` and `connect` construct its private connection, and both configure TLS1.3 exclusively. Remove the copied completed-protocol equality; completed ALPN remains independently checked | `native_tls_refuses_completed_peer_without_alpn_and_accepts_original`, plus the original mutual TLS fixture |
| Original native transport borrows its verified peer certificate | Mandatory original certificate verification refuses empty input upstream and nonempty intermediates in `RawVerifier::verify_key` on both client and server before completion. Remove the copied completed certificate-count equality; use the verified first peer and retain raw SPKI validation | Original raw-key/pin TLS fixtures and exact constructor/verifier source provenance |
| Another actual public store authorizes the same issuer/owner/org at equal revision but a different membership id | Current commit's membership-id comparison refuses before create; all other current issuer relations hold. Exact original member admission creates and reopens | `equal_revision_alternate_admission_requires_current_membership_id` |
| Alternate actual public store's owner membership id names a reader membership in the target; current issuer still belongs to the original owner | Current membership principal comparison refuses before create. Public target owner membership and its original Cedar admission create and reopen | `equal_revision_alternate_admission_requires_current_membership_principal` |
| Coherent multi-organization registry bytes are restored through original public `open`, using original actual bootstrap organization/member/credential/lifecycle records | Current membership organization comparison refuses a foreign admission selecting an owner membership in the other organization; current issuer remains in the intended organization. Exact original membership creates and reopens | `equal_revision_alternate_admission_requires_current_membership_organization` |
| Actual public reader credential remains valid while its non-owner membership is disabled in otherwise-valid restored bytes | Current membership-active comparison refuses before create. The same original issuer/policy admission and valid active restore accept and reopen; owner membership remains active in both | `equal_revision_alternate_admission_requires_active_membership` |
| Current credential is in organization two, while another real store admits the same credential id/owner/member/audience in organization one at equal revision | Current credential organization comparison refuses although target has the exact active intended membership too. Original target issuer/Cedar authority in its own organization accepts and reopens | `equal_revision_alternate_admission_requires_current_issuer_organization` |

The restore fixtures consume the real serialized process-boundary contract and
must first reopen successfully through `LocalCredentialStore::open`, including
its original journal and registry validators. They do not construct or mutate a
private in-memory `Registry`, invent an admission, bypass Cedar, or add an access
backend identity. Combining original bootstrap fixture records into a valid
restore input is a boundary-input test, not a supported production merge command
or a claim that public issuance can add an organization. Each negative preserves
original target and authorizer bytes; counterparts retain their actual authority
and original lifecycle evidence.


### Original stage capability, fixed context and numeric publication owners

The original StageOwnership has private immutable fields and no Clone or serde
constructor. Its sole production constructor is LocalCredentialStore.acquire_stage:
it reads a replay-validated record and retains that same original store and its
private per-invitation StageGate permit. A trusted lookup can return another real
store's ownership, so original pointer identity remains required; it cannot mutate
an ownership or manufacture the same original gate with another record.

PairingRecord initializes id and intent once. Claim is selected once from Available,
Approve preserves that claim, and Stage is admitted only from Approved. Receiver,
terminal cleanup and fencing preserve the original claim and Stage credential/request.
StoredPairing append/reopen replay those same domain constructors and events; no
publication replaces an existing immutable invitation identity/intent. The original
registry's lifetime lock and loaded mutation owner persist before publishing current
state. NoReceiverCleanupProof is private and is created only from the original
lookup's absent result while retaining the original gate. Its final commit proves
that same original store/gate again.

| Input / ordering | Sole original enforcement; derived copy removed | Retained dynamic detector |
| --- | --- | --- |
| Acquire original Staging or Terminal+cleanup_pending snapshot | Legal Claim→Approve→Stage and cleanup_pending's private stage relation already imply claim and stage presence; acquire_stage retains phase eligibility, original read/store and physical gate | `expired_stages_keep_reserved_identity_and_published_activation_is_not_expired`, `absent_lookup_and_caller_loss_retain_original_stage_without_invented_effect` |
| Original ownership re-reads before terminal lookup | Original private store/id capability plus one-shot immutable intent/claim/stage and replay/publication provenance establish static correlation; remove its copied four attribute comparisons. With the original permit held, Terminal and absent receiver also imply cleanup_pending, as proved below | Current Terminal and absent receiver remain required before physical lookup; actual premature/caller-loss and already-recorded-receiver fixtures |
| Lookup returns original ownership or a different legitimate store owner | Original gate pointer establishes the exact private capability; remove copied returned id/intent/claim/stage comparisons after that identity check | `returned_stage_ownership_and_receipts_must_match_original_registry` still refuses an actual other store gate and each conflicting receipt field |
| Private absent-result proof reaches final current commit | Same original gate/store plus immutable record provenance establish intent/claim/stage; remove final copied static comparisons. Its successful original Terminal re-read and monotone replay also establish Terminal at the final commit | Current receiver and cleanup_pending remain checked before applying the domain event; actual absent/caller-loss, foreign-proof/idempotent completion and terminal expiry fixtures |
| NoReceiverCleanup reaches a legal private stage | Claim→Approve→Stage already implies claim presence. Stage publishes receiver/epoch together as None; Receiver publishes both Some; fencing preserves the receiver, and restore replays those rules. Remove later claim-presence and epoch-presence copies | Domain event still requires System, original generation/credential/request, Terminal, uncleaned and absent receiver; actual `no_receiver_cleanup_retains_terminal_cause_and_rejects_contradictory_late_result` and paired cleanup fixtures |
| Construct context from completed original native transport | Twelve fixed typed/static parts plus their u16 prefixes total 335 bytes under the current wire-value/profile owners; remove the unreachable whole-context2048 runtime condition. No generator/schema/configuration field publishes a variable context ceiling in this producer; the actual profile/vector owns the fixed encoded length. Keep u16 representation conversion | `public_native_context_preserves_profile_and_same_channel_binding` checks actual peer agreement and the documented fixed fields; transcript/channel mismatch and real mutual TLS/PAKE fixtures remain |
| Each numeric owner publication is invalid while all other fields are literal valid values | Existing derivePairingValues is the original safe-integer and positive-width owner. No numeric source guard is deleted | Actual Node test for each field: NaN, fractional, both infinities, unsafe integer, negative and zero refuse; positive1 and MAX_SAFE_INTEGER derive exact constants/shape |
| Direct array owner lookup receives inherited property names | Direct exported lookup remains an independent original entry point; retain its own-property check, rather than treating compiled schema provenance as a public type | Named direct inherited-name refusal test plus exact InvitationId/ConsentIntentId/AttemptId/DeviceKey positives |

These are constructor/replay proofs for absent private states, not runtime credits
from forged fixtures. Deleted derived copies, retained original rules and actual
detector registrations remain distinct evidence categories; historical receipts
retain their original source attribution.


### Original OS entropy error conversion

The calling-side entropy seam remains injected `RngCore`; no second public
entropy constructor or worker interface is needed to test adapter conversion.
The production adapter's actual `getrandom::fill` result maps through one pure
`entropy_error` converter. Its former direct private test is not public evidence
of OS refusal or physical worker cleanup.

Public `OsEntropy::try_fill_bytes` is exercised for successful acquisition.
Deterministic OS-error conversion is not publicly reproduced here; previous
private converter assertions remain historical implementation evidence only.
Injected entropy-fault worker fixtures retain their separate public contract.

### Further private publication and cleanup provenance

The credential verifier reads the private `LocalCredentialStore` registry, not
an injected `PairingStore`. Initialization and commit reconciliation validate the
loaded registry; every production mutation persists through that same validator
before assigning the mutation slot or published snapshot. The eight production
publication paths are bootstrap, issue, owner recovery, revoke, create pairing,
owner decision, authorized pairing commit and ordinary pairing commit. The only
additional direct assignment is a test helper, not a production publication
path. `Registry`, its fields, `persist` and `publish_snapshot` are private.

For each published DevicePairing proof the validator asks the original projection
for its credential binding. That projection permits Active with an unrevoked
credential, or formerly Active Terminal with a revoked credential whose replayed
transition also passes the domain cancellation/revocation agreement when the owner
cancelled Active. The canonical external-revocation bridge retains its separate
original cause/actor/time. Consequently,
after the verifier's current revocation refusal, its later Active-phase comparison
is a derived copy. The requested audience, proof mechanism and actual native TLS
key comparison remain dynamic.

`StageOwnership::new` is crate-private and has one production caller:
`LocalCredentialStore.acquire_stage`. Although its stored port is an
`Arc<dyn PairingStore>`, an external trait implementation cannot construct a
capability with itself as that port or replace the private immutable field.
It can delegate to a real store and return that store's actual capability.
A lookup can likewise substitute another actual capability, so the original
gate pointer check remains necessary.

The sole NoReceiverCleanupProof constructor consumes the original StageOwnership
into an absent-result proof retaining that same non-cloneable permit. Cleanup
can complete while the permit remains held inside the proof; the original
StageOwnership has already been consumed and no ownership extractor can produce
another lookup capability. A second acquisition cannot succeed until that permit
is dropped. Receiver cleanup preserves the receiver, whereas absent cleanup
requires this original proof. Thus an original ownership re-read that is Terminal
and has no receiver cannot also have completed absent cleanup. Its additional
cleanup_pending comparison is derived. Terminal and receiver checks remain.

The final finish_no_receiver commit likewise cannot observe a nonterminal record
through that proof. The original prelookup read established Terminal; the proof
retains that exact store and id. The collection refuses replacement or duplicate
invitation ids, and domain replay has no Terminal-to-nonterminal transition.
Reopen cannot replace the live store under its retained lifetime lock. A custom
trait implementation cannot substitute the proof's private original store.
The final host Terminal comparison is therefore omitted as another derived copy.
The prelookup Terminal check remains dynamic, because StageOwnership may have
been acquired while Staging. The public domain NoReceiverCleanup event still
enforces Terminal independently; callers can construct that event for Staging.
The host's current receiver check and idempotent cleanup_pending branch remain.

| Input / order | Original owner and expected result | Actual fixture / counterpart |
| --- | --- | --- |
| Published device credential is unrevoked | Original Registry validation implies Active; omit later phase copy and retain audience, proof kind and native key verification | Existing real verifier and canonical revocation fixtures; no forged private Registry |
| Original Terminal ownership has a remembered receiver before lookup starts | Refuse Conflict before calling receipt lookup; preserve current durable bytes | `terminal_stage_refuses_recorded_receiver_before_lookup` uses the public original remember_receiver commit and zero lookup calls, then an actual absent terminal counterpart |
| A legitimate other store's absent proof has the same invitation and bindings | finish_no_receiver refuses the foreign original gate and preserves both registries | `no_receiver_proof_refuses_other_registry_and_retains_original_completion` obtains both capabilities from the actual original stores; no forged proof |
| Original absent proof completes, then the same proof is retried | Return the same cleaned record without another transition; the proof still retains the original store until drop | Same fixture checks exact durable bytes on retry and actual reopen Locked until the original proof is dropped |
| NoReceiverCleanup is attempted on otherwise-valid Staging | Domain phase owner refuses Conflict; original Terminal counterpart succeeds once | `no_receiver_cleanup_retains_terminal_cause_and_rejects_contradictory_late_result` |
| NoReceiverCleanup is repeated on the already-cleaned Terminal record | Domain cleaned-state owner refuses Conflict; host retry is owned separately by its Existing path | Same domain fixture preserves original terminal cause and tests the completed record directly |

The exact 93a0 compiler, 22 baselines, 19 Rust mutation pairs and six JavaScript
pairs remain evidence of their historical source epoch. The query delegates to
the existing CredentialTransition issuance validator. The three issuance-shape
conditions are owned there, rather than repeated in the query.

### Authenticated confirmation and late receiver evidence

The `94411463` author window compiled the two real-store cleanup fixtures and
the strengthened domain fixture, then distinguished the query, original gate,
three receipt fields and fixed transcript order with six individual removals.
The following additional rows describe independent boundaries that those
results do not establish.

`ConfirmedClaim` comes from the original TLS-bound PAKE server completion.
That constructor authenticates its public transcript; it does not establish
that its consent, generation and expiry match a separately stored invitation.
The canonical commit owns that relationship. A mutually authenticated exchange
can carry otherwise-valid public values that differ from that invitation while
retaining its exact invitation id, attempt, device key and canonical KE1 input.

| Input / order | Original enforcement and retained facts | Actual fixture / valid counterpart |
| --- | --- | --- |
| Real original TLS/PAKE confirmation carries a different consent id | `confirm_claim` refuses Conflict before changing the original pending attempt; original admitted key and KE1 input match | `confirmed_claim_requires_original_consent` uses the real exchange, unchanged durable bytes/reopen, then the canonical authenticated fixture |
| Real original TLS/PAKE confirmation carries another positive generation | The same commit refuses Conflict; it does not derive generation agreement from cryptographic validity | `confirmed_claim_requires_original_generation` with positive generation2 and canonical generation1 |
| Real original TLS/PAKE confirmation carries another positive expiry | The same commit refuses Conflict even while the original attempt remains eligible | `confirmed_claim_requires_original_expiry` changes only expiry, with canonical positive counterpart |
| Another public store grants the same intent at the same revision, but the target has no original issuer id | Current issuer selection refuses Unavailable before Stage; existing owner/member/audience/time relations remain valid | `equal_revision_alternate_store_admission_requires_missing_original_issuer` isolates the actual missing-id branch from the earlier present-reader case and preserves both stores |
| A genuine absent proof is retained, then the original public receiver result arrives before completion | The same final owner refuses Conflict and preserves the retained receiver; successful prelookup absence cannot decide later receiver state | `no_receiver_completion_refuses_receiver_arriving_after_absent_proof` retains the proof-held original gate, then follows the real receiver through canonical fencing/cleanup and reopen |
| The receiver then reaches canonical fencing cleanup while that original absent proof remains held | The host still refuses absent completion before its already-cleaned Existing branch can return receiver cleanup as an absent completion | The same fixture's post-fence assertion independently distinguishes the host receiver guard; the earlier pending-receiver refusal is also protected by the domain event and receives no separate host removal credit |

The absent issuer fixture uses public bootstrap/issuance/admission and the real
current store. It does not remove an in-memory registry entry or construct an
admission privately. The present-reader and absent-id cases get separate test
registrations so one case's first assertion cannot mask the other.

An absent proof retains physical dispatch exclusion; it is not a prohibition
on the canonical receiver-result API. Consequently, receiver arrival remains
a genuinely dynamic fact at final completion even though Terminal is monotone.
Its original cause and actor survive receiver retention and fencing cleanup.

### Original file acknowledgement after synchronization

The original `PublishedPrivateFile` remains owned through synchronization and
final name validation. Auth public same-save/reopen tests observe current-file
acknowledgement. They do not establish the exact interleaving between original
sync and final identity validation. The former owner-local rendezvous and direct
private-helper tests are withdrawn; no production test switch replaces them.

### Private transition minting and live previous-state correlation

`PairingRecord::transition` is the sole constructor of `PairingTransition`.
Its before, after, event, actor and time fields are private and immutable;
there is no raw or serialized transition constructor. Persisted history is
restored by `StoredPairing::restore(canonical)` through `PairingRecord::new` and each
public domain transition. Consequently, re-running a transition to compare
its own minted evidence repeats the original domain decision. Live append
must still correlate the transition's original before snapshot with the
caller's current record; private construction does not establish that external
relationship.

| Input / order | Original owner and disposition | Actual detector / valid counterpart |
| --- | --- | --- |
| An immutable transition is checked against its original before snapshot | Keep the exact before equality in `PairingTransition::verify`; omit deterministic replay and evidence equality derived from private minting | `pairing_history_agrees` accepts reservation against initial and claim against reservation, but refuses claim against initial and reservation against the later claim |
| Receiver result arrives with a stage and no earlier receiver | Stage alone is minted only by Approved→Staging; Active requires a receiver already present. Terminal preserves the original stage. The existing stage/cleaned/receiver guards therefore imply Staging or Terminal; omit only the final copied phase predicate | Existing staged and late Terminal receiver counterparts use public transitions; no malformed private record can be manufactured to distinguish the derived predicate |

Receiver actor, generation, stage presence, cleanup, credential, request,
positive epoch and absence of an earlier receiver remain independently checked.
The prior masked removals of before correlation or implied Receiver phase are
not runtime proof. After replay removal, the existing stale-previous fixture
is the discriminating detector for the retained live correlation owner.


### Domain input neighbors for retained attempts and cleanup

These cases use public immutable records and events. They distinguish caller
input from private construction consequences; they do not establish PAKE,
current authorization, receiver effects or native transport behavior.

| Input / order | Domain owner and expected result | Valid counterpart |
| --- | --- | --- |
| Same reservation id/fingerprint but another device key; repeat the original Reserve as a transition | Reservation lookup refuses Conflict; a receipt replay remains a read and cannot charge another attempt | Original key reads Pending, and after original failure a fresh reservation can be admitted |
| Fail or Claim names a missing attempt, another original key, a wrong event actor or an already settled attempt | Original attempt lookup, key, actor and Pending owner refuse before changing immutable state | Exact original device settles Failed or Claimed, preserving the charged count |
| Stage is requested while Claimed with otherwise-valid owner, generation and time | Stage requires Approved before creating dispatch identity | Exact Approve then Stage retains the original credential/request |
| Receiver result has wrong actor/generation/request, zero epoch, no stage or an earlier receiver already retained | Original stage owner refuses the input; a duplicate result is not a new receiver publication | Exact first result succeeds; a late result may attach to the same cancelled Terminal stage |
| Cleanup has wrong actor/phase/credential/request, a nonadvancing fence or an already-cleaned stage even with another advancing fence | Original Terminal receiver/fence owner refuses; no new cleanup cause is invented | Exact receiver with strictly newer epoch finishes once and preserves first cause/actor |
| Expired or Restarted uses a non-System actor; Denied targets Active; ending follows both Failed and Pending attempts | End enforces the original phase/cause/actor relation; Failed remains Failed and only Pending becomes Superseded | System expiry/restart and owner cancellation use their eligible original phases |
| Canonical credential revocation names another credential, a non-Active invitation, issuance evidence or another ordering time | Pairing bridge consumes the public validated credential transition and checks its relation to the original active stage | Exact original revocation at its retained canonical lower bound preserves its initiator and cause |
| Positive invitation lifetime overflows its creation timestamp | PairingRecord constructor refuses Invalid through checked addition | Exact maximum representable expiry succeeds without saturation or wraparound |

A public CredentialTransition's Revoked cause already implies a retained
revocation timestamp through its construction owner. The pairing bridge therefore
omits the copied timestamp-presence predicate and retains its cause check. An
Issued transition is a valid public input to the bridge; its refusal case uses
the actual canonical ordering lower bound so the time check cannot mask the cause
rule. No malformed private transition fields are assembled.

| Original evidence / order | Sole owner and bridge disposition | Actual detector / valid counterpart |
| --- | --- | --- |
| Valid Revoked CredentialTransition reaches the pairing bridge | CredentialTransition::new already requires the retained revocation timestamp; remove only the bridge's copied timestamp predicate, retaining cause/phase/credential/actor/time relations | Existing real revoke and supersede counterparts remain valid |
| Otherwise-valid Issued transition targets the active original credential at its canonical lower bound | Pairing bridge refuses Conflict because issuance is not revocation | `canonical_revocation_requires_original_active_credential_cause_and_time` uses issuance at second1 and the actual lower bound1000, alongside exact original revocation at2000 |


### Credential binding asks the credential owner

The binding bridge consumes raw stored metadata through the existing Credential
constructor before comparing it with the original intent. Credential owns grant
organization/lifetime validity; the bridge retains the original credential ID,
intent organization, cause, actor, time and generation relationships. Registry
restoration ordering and typed refusal are unchanged.

| Actual persisted input / ordering | Owner / public evidence |
| --- | --- |
| Acknowledged Active credential changes one original metadata relationship or generation | `public_reopen_refuses_conflicting_credential_metadata` edits actual stored bytes and observes public reopen InvalidRegistry; exact original bytes restore Active. This does not independently isolate every internal predicate. |
| Original issuance acquires an invalid before/after/time shape | `public_reopen_refuses_malformed_original_issuance` observes public refusal and exact original restore. CredentialTransition remains the sole shape owner. |
| Grant organization is invalid, or credential and grant form a valid foreign organization | `public_reopen_refuses_invalid_and_foreign_organization` refuses at public reopen; original Active credential reopens. |
| Second genuine DevicePairing issuance is substituted for the original credential | `pairing_binding_requires_original_identity_even_with_another_real_issuance` observes public registry refusal and original restore; the second registry is untouched. The public detector does not claim that no other agreement owner participates. |
| Formerly Active Terminal has an invalid revocation timestamp | `public_reopen_refuses_revocation_before_original_issuance` observes public refusal and exact original Terminal restore. |
| Approval advances revision before staging a second genuine issuance | Capture a fresh AuthorizePairing result after approval, preserving original StaleRevision behavior. |

Private transition minting/replay implies retained Claim presence through legal
Approve, Stage, Activate and later Terminal events. Its removed copy remains a
source-provenance disposition, not a fabricated malformed live state. Historical
raw-query mutation pairs are implementation evidence; they are not relabeled as
current public-boundary proof.

### Public approval replay and completed receipt capacity

| Ordering / input | Owner / decision | Evidence |
| --- | --- | --- |
| Exact Claimed approval produces Approved, then another domain approval uses original key/owner/generation before expiry | PairingRecord permits the first transition and refuses the second Conflict; application exact approval retry returns the original receipt instead of appending | approve_exact_claim uses real Approved followed by otherwise-valid repeat; original Claimed success and exact claim remain |
| MAX_LIVE_PAIRINGS+1 unique invitations reach Active through public claim/approval/stage/receiver/activation, then each is legally cancelled to Terminal | Collection retains completed receipts without charging unfinished enrollment capacity; unique identity and unfinished ceiling remain enforced | collection_completed_receipts_do_not_consume_unfinished_capacity asserts Active and Terminal cohorts Ok beside the existing live-ceiling case |


### Renewed public-boundary evidence correction

The prior ordinary and structural review results remain historical. The renewed
human request authorizes correcting their remaining evidence defects; it does
not reset those rounds or grant native consumer acceptance.

| Actual input / ordering | Original owner and observation |
| --- | --- |
| Temporary fixture parent has inherited Windows permissions | Create a new private root through nessa-local-storage::create_directory, then create the pairing directory beneath it. Never repair an unsafe existing directory or weaken its ACL check. |
| Acknowledged registry history or credential metadata is changed on disk | Drop the original store, reopen LocalCredentialStore, and observe typed refusal with unchanged altered bytes; exact original bytes reopen successfully. Private projection/helper results are not public acceptance. |
| Real public pending save succeeds, caller repeats or process reopens | load_pending and exact save retry preserve original key, pin and intent. No fault selector supplies a lost acknowledgement. |
| Real pending destination is obstructed | Public save/OPAQUE finish refuses before KE3; remove the actual obstruction and retry the same public operation. |
| Gateway clock observation occurs before intent, key or outcome publication | A fixture may obstruct the actual filesystem name through this existing injected callback; public save reports the actual OS publication failure, retains original intent and key-effect meaning, and public restore/retry completes after obstruction removal. No result is fabricated inside FilePairingState. |
| Original storage acknowledgement has a failure between sync and final identity validation | The production original-handle checks remain. Exact deterministic Auth reproduction is unsupported without an original IO-boundary seam; former cfg(test) rendezvous and private-helper pairs are withdrawn as acceptance evidence. Shared storage evidence is not relabeled as this consumer's proof. |
| Public TLS accept receives too much real peer input | A counting external IO adapter records actual accepted bytes and refusal through NativeTransport, without constructing HandshakeBudget. Actual successful TLS peers remain the success counterpart. |
| Public OS entropy acquisition succeeds / OS refuses entropy | Real successful OsEntropy acquisition is observed. Private error-converter assertions are removed; deterministic OS refusal remains untriggered here, while injected crypto entropy refusal has its separate public owner. |

### Public cryptographic fixture ownership

| Input / ordering | Existing owner and meaning | Public evidence scope |
| --- | --- | --- |
| Private-state fixture creates and reopens its original root | Shared storage creates the private directory; Windows uses the normal TempDir drive path and Unix canonicalizes its anchor to resolve `/var` symlinks | One retained fixture anchor feeds creation, retries and reopen. No verbatim Windows prefix or production path/ACL relaxation is introduced; actual hosted Windows acceptance remains required |
| Actual bounded loopback TCP peers complete native TLS | Public NativeTransport accept/connect own handshake, key proof and exporter; each peer calls pairing_context on its own completed channel | Same-channel contexts agree, a different real channel refuses OPAQUE binding, and pending-state refusal produces no KE3. No caller supplies an exporter or private context constructor; this fixture is portable TCP, not listener activation |
| External raw-public-key TLS peer signs correctly, corrupts its actual signature, or omits ALPN | Public ring/rustls peer setup supplies the external input; NativeTransport validates the resulting handshake | Valid and invalid peer cases retain actual signature and channel observations; no private Nessa certified-key or verifier constructor is used |
| Remember real trusted-adapter receiver outcome, then publish pairing | Original PairingStore receiver/publication operations retain their admission and recorded result | Activation fixture setup directly publishes its original receiver. Former fail_before_replace setup is withdrawn; it is not evidence of physical server receiver creation or pre-publication failure |
| Publication replacement succeeds but synchronization and reconciliation fail | Original registry persistence owner marks health failed; committed readers check that existing health latch | Exact lost-acknowledgement proof from fail_directory_sync is withdrawn. Pre-replacement filesystem obstruction does not imply this health transition |

The unresolved original stage-gate integration question remains a native consumer
risk rather than a confirmed producer defect. These fixture changes confer no
listener, protected route, SDK receiver, or whole-resource acceptance.


## Native enrollment wire format (B0)

The server's enrollment codec is `device_pairing/infrastructure/wire/`, and the
status it encodes is `device_pairing/application/status.rs`. It is pure: no
socket, clock or file. Auth's domain constructors validate every decoded value;
the codec issues no grant.

| Input or order | Owner and result | Test and accepted neighbor |
| --- | --- | --- |
| Raw KE1/KE2/KE3 versus encoded envelope | Auth's `MAX_ENROLLMENT_MESSAGE_BYTES` bounds raw crypto input. The wire module's `MAX_ENROLLMENT_ENVELOPE_BYTES` separately bounds the complete JSON envelope at 4096 bytes. Byte payloads are JSON numeric arrays. Neither value is a connection memory limit. | `selected_crypto_messages_fit_the_native_envelope` encodes real KE1/KE2/KE3 from the public TLS/OPAQUE APIs. `envelope_limits_apply_before_decode_and_during_encode` accepts exactly 4096 bytes and refuses 4097, decoding and encoding. |
| Each enrollment phase tag | Encoded JSON names the phases `pending`, `unclaimed`, `claimed`, `approved`, `staging`, `active` and `terminal`. A matching encoder/decoder rename would not show the spelling changed, so tests compare literal tags. | `public_projection_contains_only_its_declared_fields`, `unclaimed_projection_preserves_outcome_and_cause_without_private_scope`, `current_enrollment_messages_preserve_their_representation`. |
| Unknown, extra or duplicate fields, malformed arrays, a second JSON document | Private serde DTOs refuse; Auth constructors own widths, generation, expiry, class and disclosed grant. | `enrollment_syntax_is_strict_and_domain_values_are_validated`; every current request and reply round-trips in `current_enrollment_messages_preserve_their_representation`. |
| Pending, failed or superseded attempt | Pending and Unclaimed carry the public operation and the attempt/terminal outcome, without owner or grant selectors. | `unclaimed_projection_preserves_outcome_and_cause_without_private_scope`; `public_projection_contains_only_its_declared_fields`. |
| Status for a valid operation taken from another record | `encode_status` compares invitation, claimed attempt, consent, generation and expiry with the Auth record before encoding a claimed status. | `canonical_status_refuses_foreign_operation_before_encoding`; Claimed/Approved/Staging/Active/Terminal records from real transitions are accepted. |
| A later status that substitutes different consent | `NativePairingStatus::correlate` asks `DisclosedConsent`; a status cannot replace consent the device already received. | `received_status_cannot_replace_prior_scope`; the exact prior consent is accepted. |
| Frame interrupted, oversized, or answered with a refusal | Not the codec's concern: framing and the `refused` reply belong to the B1 rows below. | See P32 and P53 in the B1 table. |

Encoding borrows payloads and writes into one bounded buffer, so an oversized
payload is refused without being copied first. Decoding refuses by length before
serde runs. An Active status is historical enrollment, not product access.

The codec's DTOs are hand-written, not generated product models. Auth's
`wire-values.json`, through the existing generator, remains the one publication
of identity and key widths.

## Native enrollment consumer (B1)

### What this slice is

`device_pairing` is the server-side consumer of the Auth pairing producer. It
holds:

- `GatewayPairing` (`infrastructure/runtime.rs`): owner create/pending/status/
  decide and device hello/begin/finish/status. It reaches Auth only through the
  `PairingStore` and `AccessReader` ports (the credential registry in real
  composition), with Cedar policy, the wall clock, the gateway key store and a
  gateway identity restored by `restore_gateway_identity`.
- `NativeEnrollmentConnections` and `NativeEnrollmentListener`: up to eight
  blocking connection workers per connection owner, serving the enrollment exchange over Auth's TLS
  transport, with a ten-second TLS deadline and a thirty-second enrollment
  deadline on the injected monotonic clock.
- `NativeEnrollmentClient`: the device side — enroll, pinned status, and retry
  after a failed attempt — saving its key, gateway pin and attempt before KE3.
- `EnrollmentChannel`: length-prefixed framing that keeps partial read and write
  progress across `WouldBlock`.

It ends at **Approved**. It stages no receiver, issues no credential and
reaches no Active state. It is **not mounted** in the default gateway.

### Why it lands unmounted

Mounting needs three things this slice does not have, each already designed
above and each its own change:

1. An owner surface. Codes are created inside the gateway process, because the
   PAKE setup is volatile. The approved surface is the product `/session`
   methods in [current owner protocol wiring](#current-owner-protocol-wiring),
   which need generated schema and a client. Without it a mounted listener
   could only refuse every Hello.
2. Startup composition: the `native.listenAddress` configuration, key/audit
   reconciliation before bind and joined shutdown
   ([startup ordering table](#startup-ordering-table), S1–S12).
3. Receiver staging and Active publication (P19–P25). Without them an approved
   device never receives a credential, so a listening socket would be attack
   surface with no product use.

The Auth producer it consumes landed the same way: as a library whose public
behavior is proved by public tests. Everything here is reachable through public
constructors, and the tests below drive real TLS sockets, the real registry,
real Cedar and real private storage.

### Owners

```text
Auth invitation:  Available --(KE3 on the same TLS channel)--> Claimed
                  Claimed   --(owner approves the exact key)--> Approved
                  any open  --(expiry / cancel / deny / restart)--> Terminal
Device store:     empty --(KE2 authenticated, save acknowledged)--> pending
Connection permit: held from admission until the worker ends
                   (a shutdown wake is not the end)
```

Arrows are causal handoffs between separate owners, not one stored state. Auth
owns invitation, attempt, expiry and key proof; the device's private store owns
its key, pin and attempt; the semaphore permit owns capacity. Framing offsets
are IO progress, not an enrollment phase.

### Rows this slice implements

Each row names the accepted-target row it implements from the tables above and
the test that enforces it here. A row not listed is not implemented by this
slice.

| Row | Ordering | Result in this slice | Test |
| --- | --- | --- | --- |
| P01 | Owner creates from a current session; policy allows or denies | Admission, then code registration, then registry commit, then the code. Denied keeps the session valid and the registry unchanged. | `native_create_claim_approve_and_reopen_status`; `native_owner_policy_denial_preserves_session_and_registry` |
| P02, P04 | Device presents a wrong code | Begin charges the attempt before ServerLogin. The device's PAKE fails at KE2, so it saves nothing and sends no KE3; the gateway settles the attempt as ConnectionClosed. The invitation stays open. | `native_wrong_codes_are_charged_within_the_attempt_bound` |
| P04, P54 | Admitted attempt ends without a claim: connection closes before KE3; KE3 bytes fail PAKE; a request other than Confirm arrives where KE3 belongs | Settled as ConnectionClosed, InvalidProof and InvalidProof respectively (a non-Confirm request is treated as a failed proof). VerifierUnavailable is the cause for any other crypto failure; no public path produces one, because an Available invitation always has its setup. | `native_attempt_failures_record_their_cause` |
| P28 | The store refuses a write: reserving the attempt, settling a failed attempt, committing the claim | Reserve: the connection fails with the store error, no PAKE runs, nothing is charged. Settlement: the connection reports the primary failure and the settlement failure together, and the attempt stays Pending. Claim: `Refused`; the attempt stays Pending and the device keeps its pending record. A Pending attempt holds the invitation's one attempt slot, so Auth refuses any new reservation (Capacity) until the owner cancels or the invitation expires. | `native_failed_store_writes_stay_visible` |
| P05 | Four wrong codes then the right one; five wrong codes then the right one | The fifth attempt can still claim. With five charged, Begin is refused (`AttemptsExhausted`) before PAKE and the record is unchanged. | `native_wrong_codes_are_charged_within_the_attempt_bound` |
| P06 direct expiry | Hello one millisecond before expiry; any request at expiry | Before: Hello answers. At: Auth's `expire_pairing_if_due` ends the invitation as Expired, nothing is charged, the device gets `Refused`. | `native_expired_and_used_codes_are_refused_without_a_claim` |
| P06, S11 | An invitation passes its expiry and nothing touches it until the owner creates again, lists, reads or decides | Owner create, pending, status and decide ask Auth's `expire_pairing_if_due` only after current authorization admits the caller, inside the same blocking owner worker; a refused owner changes nothing. Create offers every unfinished record of the gateway; pending, status and decide offer the records the caller is admitted to. An expiry advances the registry revision that an admission is bound to, so create and decide take a fresh admission after settling expiry and commit against that one. The old invitation ends Expired by System, not Cancelled, and the new create succeeds. Each owner command, and device status, ends by reading the open invitation's record and discarding the volatile setup if it has ended. There is no timer: an untouched record stays Available in storage until one of these paths, a device path below, or a restart settles it. | `native_expired_code_is_settled_before_the_next_create`; `native_refused_owner_changes_no_enrollment`; `native_owner_store_work_runs_off_the_async_thread`; `native_ended_invitation_setup_is_discarded` |
| P06, P31 | Device hello and device status at or after expiry | Hello has no caller identity, so it asks `expire_pairing_if_due` for the open invitation before answering. Status first matches the operation and the device key against the stored attempt (WrongActor otherwise, with no change), then asks `expire_pairing_if_due`, then reports. | `native_expired_and_used_codes_are_refused_without_a_claim`; `native_restart_expires_due_claimed_enrollments`; `native_refused_owner_changes_no_enrollment` |
| P06, P10 | Valid KE3 or Begin arrives at or after expiry; or Auth refuses a past-due reservation for another reason (a conflicting replay of the attempt) | Begin and finish do not expire first: they read, then reserve or claim, and Auth decides. After any Auth domain refusal of a reservation or claim, the runtime asks `expire_pairing_if_due`, which changes nothing unless the record is due, so the invitation ends Expired, the pending attempt becomes Superseded and the setup is discarded. A store failure (not a domain refusal) settles nothing; the next path that reads the record does. The device gets `Refused`; its pinned status then reads Unclaimed (Superseded, Expired), not Pending, and the next create succeeds. | `native_claim_losing_to_expiry_is_settled_and_recoverable`; `native_past_due_conflicting_reservation_settles_expiry` |
| P06, S6, S9 | Gateway restarts holding a Claimed enrollment, or an Available invitation, past its expiry | `GatewayPairing::open` asks `expire_pairing_if_due` for every unfinished record of this gateway first, so a past-due one ends Expired, its first true cause. Only a record still Available afterwards ends Restarted. | `native_restart_expires_due_claimed_enrollments`; `native_restart_records_expiry_before_restart` |
| P08 | A second device presents a code after it was claimed | Hello finds no open invitation; `Refused`; the claimed record is unchanged and the device saves nothing. | `native_expired_and_used_codes_are_refused_without_a_claim` |
| P09, P56, P58 | KE2 succeeds, the device's pending save fails | No KE3 is sent and no claim exists; after the gateway settles the attempt, a fresh enrollment claims. | `native_pending_save_failure_sends_no_claim_and_retry_recovers` |
| P56 | A gateway answers Hello for another attempt, or a Challenge for another operation | The device refuses (Phase) before PAKE finish; nothing is saved and no KE3 is sent. | `native_client_refuses_a_gateway_that_changes_the_operation` |
| P12, P57 | Claim committed, reply lost, gateway restarted | Fresh pinned status with the saved key reads the same claim, without a second attempt. | `native_claim_reply_loss_preserves_pinned_status` |
| P54 | Claim committed; reading the store afterwards would fail | The reply is built from the record the claim commit returned, and the setup is discarded from that record, so no read follows the commit. A failure to send the reply is a typed `ClaimReply` failure, which is never answered with `Refused`. | `native_committed_claim_is_reported_without_a_later_read` |
| P13, S6 | Gateway restarts with an Available invitation | `GatewayPairing::open` ends it as Restarted before serving; the gateway key is restored, not regenerated. | `native_restart_ends_available_setup_and_preserves_key` |
| P17 | Owner approves another valid key, then the claimed key | Conflict for the other key; Approved for the claimed one, with no credential. | `native_create_claim_approve_and_reopen_status` |
| P31, P60 | Valid KE3 on another TLS channel with the same device key; a channel accepted under another gateway key | Refused as InvalidContext before PAKE finish; the record is unchanged. The original channel claims. | `native_finish_refuses_another_channel_without_claim` |
| P31 | Status request from another device key | Auth refuses (WrongActor) before disclosure; `Refused`; the original key reads its status. | `native_status_refuses_another_device_and_accepts_original` |
| P32 | Frame prefix announces 4097 bytes; exactly 4096 | Refused before the body is read or allocated; 4096 accepted. Encoding refuses 4097. | `native_framing_refuses_oversize_before_body_and_accepts_exact` |
| P32 | Partial prefix, body or output, interrupted by `WouldBlock` | The channel keeps its offsets and resumes; a different envelope cannot replace pending output. | `native_framing_retains_partial_io_and_output` |
| P52, P66 | Eight connections held, a ninth arrives, waiters dropped, shutdown | The limit is per `NativeEnrollmentConnections`; composition must build one per gateway, which is not enforced by construction. Ninth refused for capacity; dropped waiters leave permits with workers; shutdown wakes each socket once and waits; later admission refused. A connection takes its permit under the lock `close` takes; this ordering is structural, and no public seam can pause between the two to test it. | `native_shutdown_keeps_original_physical_capacity` |
| P53 | Begin without Hello; Begin naming another attempt; refusals | Refused as Phase before any attempt is charged. A gateway decision on a readable channel is answered with the redacted `Refused` reply; physical failures are not. | `native_out_of_order_requests_are_refused_before_an_attempt_is_charged`; `native_expired_and_used_codes_are_refused_without_a_claim` |
| P54, P59 | KE3 arrives at the enrollment deadline; one millisecond earlier | At: refused, attempt settled HandshakeDeadline, no claim. Earlier: claims. Both on the injected clock. | `native_late_confirmation_is_settled_as_deadline_without_claim` |
| P55 | Entropy panics inside create, registration or client work | Typed `WorkerFault(Panic)`; capacity released; nothing published; the next operation succeeds. | `native_worker_faults_preserve_type_and_allow_new_work` |
| P61 | Retry while the original attempt is Claimed | Refused (`OriginalNotRetryable`) before a new attempt. | `native_create_claim_approve_and_reopen_status` |
| P62 | Retry after Unclaimed, same invitation; retry when Hello names a different invitation | Same invitation: new attempt, pending record replaced only with the attempt changed. Different invitation: refused (Phase) before Begin; pending record and new invitation unchanged. | `native_client_observer_loss_keeps_pending_save_and_operation_owned`; `native_retry_refuses_another_invitation_before_a_new_attempt` |
| P67 | Listener stopped with peers in flight; one peer fails | Admission stops, every admitted result is collected, then drain. A failed peer does not end service. An accept error ends service and is reported to the `failed` callback. | `native_create_claim_approve_and_reopen_status` (failed peer then enrollment on one listener). The accept-error path has no test: no public seam induces an accept failure. |
| P68 | Registration finishes before create publishes | The registration permit stays with its output until commit or refusal. | `native_registration_output_retains_original_capacity` |
| P77, P83 | Create waiter or client waiter dropped mid-operation | The worker keeps the runtime, private store lock and permit until it ends; shutdown waits for it; reopen succeeds after. | `native_create_observer_loss_keeps_original_owner_until_drain`; `native_client_observer_loss_keeps_pending_save_and_operation_owned` |
| O2, O3 | Owner lists unfinished enrollments and reads one; the session has expired | Each listed record and each status read asks current session and policy; an expired session is refused and the record is unchanged. Terminal records are not listed. | `native_owner_discovers_unfinished_enrollments_through_current_policy` |

Not implemented here, by row: P03 identical-retry receipt through the native
client, P07, P11, P14, P18–P27, P29, P30, P63, P65 (Auth's TLS budget, not this
slice), M1–M2 periodic expiry (expiry is settled when a path reads the record,
not by a timer), O1/O4–O9 as product routes, and S1–S5, S7, S8, S10, S12.

### Open design items for mounting

- **Accept failures (P67).** P67 ends service when `accept` fails. On
  BSD-derived systems, including macOS, `accept` can return `ECONNABORTED` when
  a peer resets before it is accepted, so a remote peer can end enrollment. The
  MCP relay listener instead logs and retries after 100 ms. The path has no test:
  there is no seam that makes `accept` fail. Decide when the listener is mounted.
- **Who closed the connection.** When the gateway's own shutdown wakes a
  connection waiting for KE3, the attempt is settled ConnectionClosed with the
  device as actor, because Auth accepts only the device as the actor of a failed
  attempt. The gateway caused it. Recording a System cause needs an Auth
  producer change.
- **One connection owner per gateway.** The eight-connection limit belongs to a
  `NativeEnrollmentConnections`; nothing stops composition building two for one
  `GatewayPairing`. Mounting composition must build exactly one.
