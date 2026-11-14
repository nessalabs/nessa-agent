---
id: gateway-peer-discovery
title: Announce a paired gateway on the local network
kind: operation
status: implemented
summary: A paired gateway announces a dial address and a rotating token, and
  does not enroll from that announcement.
parent: gateway
sourceRevision: "82ce929bf9127c2a927f85c2db20aa081d62f06e"
sources:
  - crates/nessa-peer-gateways/src/application/discovery.rs
  - crates/nessa-peer-gateways/src/application/discovery_authority.rs
  - crates/nessa-peer-gateways/tests/discovery.rs
  - crates/nessa-peer-gateways/tests/discovery_records.rs
  - crates/nessa-peer-gateways/src/infrastructure/records.rs
  - crates/nessa-auth/src/adapters/local/registry/pairing/discovery.rs
  - crates/nessa-server/src/composition/peer_discovery.rs
  - crates/nessa-client-core/src/pairing/client.rs
  - crates/nessa-device-pairing/src/infrastructure/connection.rs
  - crates/nessa-protocol/src/pairing/wire/mod.rs
  - docs/design/auth/peer-gateways.md
model:
  spec: tla/services/gateway/PeerDiscovery.tla
  config: tla/services/gateway/PeerDiscovery.cfg
  steps:
    Init: No discovery key, phase Active, credential current, nothing announced.
    Key: The dialer stores its private key before any public half is sent.
    Offer: The peer public is stored for the same generation.
    Install: The MAC key replaces the private key.
    Expire: The credential is no longer current and the phase stays Active.
    Announce: Announce only an installed field with a current credential.
    Quiet: Send nothing.
diagramLinks: {}
---

# Announce a paired gateway on the local network

A gateway with a paired peer whose discovery field is installed and whose credential is current may announce a dial address, a port, and one rotating token for that peer. The token is an HMAC of the time window, the address, and the port, keyed by the discovery MAC both sides stored beside the pinned key. The announcement does not carry the pinned key, a fingerprint, or another stable gateway id. Hearing it does not enroll anyone. The connection is still the pinned credential, and pairing still requires the one-use code.

With no such peer, or after definitive observation that its credential is revoked or expired, nothing is sent. Expiry does not change the phase and does not remove the discovery key. A peer with no discovery key is left out.

```mermaid
stateDiagram-v2
    [*] --> None
    None --> Keyed: new enrollment dialer commits private key
    None --> None: legacy enrollment remains silent
    Keyed --> Offered: peer public for this generation
    Offered --> Keyed: public or shared secret rejected
    Offered --> Installed: both sides stored, MAC replaces private key
    Installed --> Installed: retry delivery over pinned session
    Installed --> Quiet: credential not current
    Installed --> Announced: credential current
    Announced --> Quiet: credential not current or no installed peer
```

## Restart and re-pairing

New dialer enrollments retain discovery eligibility when their pending record is saved. Older records without that field stay silent after upgrade. Once Active, an eligible enrollment retries the pinned exchange even when its own MAC is installed: a saved local key does not prove the peer received the acknowledgement. Token refresh runs independently of retry progress. Stop withdraws the announcement before joining the current attempt, and the scheduler does not start another peer. Network phases share 5-second connect, 10-second TLS, and 30-second exchange budgets; physical storage and worker drain are joined rather than relabelled complete.

The pinned discovery request carries the full enrollment intent and its non-secret credential ID. The granting gateway validates both against the actual TLS key before returning any credential evidence. Only a definitive NotCurrent answer for that exact enrollment stops its announcement; unsupported discovery, mismatched history, storage failures and lost connections remain uncertain and retryable. Installed dialers read the answer too. Every delayed local commit compares the same enrollment identity under the record lock, so an old attempt cannot write into a replacement pairing.

The exact NotCurrent observation is retained atomically beside the private peer record, with its full enrollment and credential identity, grantor-system initiator, current-to-noncurrent outcome and injected observation time. Eligibility derives from that observation, not a separate flag. Repeated responses and later pairing revocation preserve the first evidence. Restore rejects mismatched identities and illegal pending-state evidence. Storage rejection is returned to the caller; no successful durable observation is claimed. If that rejection occurs after the exact noncurrent authority decision is known, the LAN task signals its own permanent stop for this process, withdraws its registration, and joins the current attempt without trying another peer. Unknown or stale enrollment errors and generic transport failures do not cause this shutdown. An observation that could not be persisted cannot survive a crash: after a later process restart it is unknown, not falsely reported as recovered or durably remembered.

## Model coverage

The configured safety rules are `AnnounceOnlyInstalled` and `ExpiryKeepsPhase`. The model keeps the phase Active while the credential expires, and it allows an announcement only for an installed field with a current credential. It does not model the X25519 derivation, the HMAC bytes, or the DNS-SD registration. Those are held by the discovery tests. The model also excludes legacy enrollment eligibility, scheduling retries, and selection among historical enrollments; the record and registry regressions cover those boundaries.

## Verification limits

The model does not execute the implementation. Window arithmetic, address canonicalization, reconnect of an offered record, and the refusal of `openDiscovery` from an unpaired key are in `crates/nessa-peer-gateways/tests/discovery.rs` and the grantor registry test.
