# 1150. A tunnel request proves a paired device before it takes a permit

## Purpose

A stranger who knows the public tunnel address must not be able to hold
the tunnel's four connection permits. A paired phone that is away from
home must be able to learn a new quick-tunnel address. This records both
follow-ups of [#1145](https://github.com/nessalabs/nessa-agent/issues/1145):
[#1150](https://github.com/nessalabs/nessa-agent/issues/1150) and
[#1146](https://github.com/nessalabs/nessa-agent/issues/1146). The phone
app that dials lives in nessa-ios; this repository is the gateway, the
relay, and the device client the phone calls.

- **Date:** 2026-11-01
- **Status:** accepted

## Context

The ingress took a tunnel permit before the WebSocket upgrade and held it
through the upgrade deadline and the TLS deadline. Four open handshakes
from anyone who knew the address kept the owner's phone out. A quick
tunnel's address also changes when `cloudflared` starts again, and a phone
that is away could learn the new one only by reaching the gateway, which
it could not.

Nessa is alpha. Devices pair again to receive a secret. There is no
migration of credentials already issued.

## Decision

Pairing issues each device one secret. The device sends it on every tunnel
request, in the `x-nessa-tunnel` header, or as `/native/<secret>` when its
WebSocket client cannot set a header. The ingress compares it, in constant
time, with the secrets of devices whose credentials are still active, and
only then takes a permit. Anything else is refused with no permit taken.
Unpairing the device or revoking its credential forgets the secret. Issuing
and revoking the secret are audited: the credential, a fingerprint of the
secret, the cause, and the initiator. The secret itself is not in the audit.

The gateway publishes the current tunnel origin on its #266 relay
registration, signed with its paired key. A reader asks the relay for that
origin. The relay challenges the reader; the reader answers with an HMAC of
its tunnel secret. The gateway accepts the mac only for a secret it still
holds, and only then does the relay send the origin. The phone checks the
signature against the key it pinned before it dials, and only then sends
its tunnel secret. The relay sees the mac, not the secret.

Issuing and revoking write an intent first, then the secret file, then a
publication that confirms the change. Both records of one change share one
operation id. A failed file write is not yet a publication, and a retry
reuses that id.

## States

| Situation | Outcome |
| --- | --- |
| Pairing becomes active | One secret is issued for that credential. The audit records the intent, then the publication, under one operation id. A later status returns the same secret and writes no second intent. A file write that fails leaves the intent unconfirmed, and the retry confirms that same id. |
| Connect with a valid secret | The ingress takes a permit and upgrades. The connection is then the same tunneled native session as before. |
| Connect with a missing, wrong, or revoked secret | Refused. No permit is taken, however many such connections are held open. |
| Unpair or revoke | The secret is forgotten and the revocation is audited. The credential is already ended, so a leftover secret file still does not admit. |
| The tunnel address changes | The gateway publishes the new origin on its relay registration, replacing the previous one. |
| A paired phone is away and its saved address fails | It asks the relay while the direct dial is still in progress, both on the one handshake deadline. It dials the origin it is told only after the gateway's signature checks. |
| A stranger asks the relay | The relay does not send the origin. |

## Invariants

| Guarantee | Enforcer |
| --- | --- |
| A request that does not present an active device's secret takes no tunnel permit. | The ingress checks the secret, then takes the permit. `strangers_holding_connections_cannot_keep_a_paired_device_out`. |
| A revoked or unpaired device's secret no longer matches. | The store drops it, and admission also requires the credential to still be active. `a_revoked_device_is_rejected_and_takes_no_permit` and `issuing_and_revoking_a_secret_is_audited_and_a_revoked_one_is_refused`. |
| Issue and revoke each leave an intent and, once the secret file matches, a publication. Both name the credential, the cause, the initiator, and one operation id, and not the secret. | The file store writes the intent before the file and the publication after it. `issuing_and_revoking_a_secret_is_audited_and_a_revoked_one_is_refused` and `a_failed_publication_is_retried_as_the_same_operation`. |
| Only a paired device is told the published tunnel origin, and the phone sends its secret only to the gateway that signed it. | The gateway checks the lookup mac before the relay sends the origin. The phone checks the signature before it dials. `an_away_phone_is_told_the_new_tunnel_address_through_the_relay` and `a_relay_that_names_an_attackers_origin_is_never_sent_the_secret`. |
| A direct dial that hangs does not use up the handshake deadline before the relay is asked. | Discovery starts at the direct head start and shares the one timeout. `a_hung_lan_dial_and_a_stale_address_still_connect_in_time`. |

## Alternatives considered

- Cloudflare Access service tokens in front of a named tunnel. A second account and a second check, and it does not cover a quick tunnel.
- Shorter pre-TLS deadlines and a larger pool. A stranger can still hold every permit; the deadlines and the pool size stay as they are.
- A named tunnel recommended in Settings. The address would be stable, and a phone on a quick tunnel would still be stuck when it changed.
- Publishing the origin on the relay for anyone who knows the gateway key. The key is not a secret. The lookup would tell a stranger the address.

## Consequences

A paired device can be kept out of the tunnel only by other paired devices,
up to the same pool of four. A phone that already knows the relay learns a
new quick address without coming home. The cost is one secret at pairing,
sent on every tunnel request, and one lookup on the relay when the saved
address fails. Re-pairing is how a device that paired before this build
receives a secret.

The iOS app calls `rememberRelay` with the relay it already uses, and sends
the device on a product dial. The Rust session attaches the stored secret
and, when that dial fails, asks the relay. A WebSocket stack in the app
that does not go through this session sends `x-nessa-tunnel`, or
`/native/<secret>` if it cannot set a header.

## Review brief

Review the branch `cursor/tunnel-paired-admission-8983` against `main`.
The tree under review is the committed head. Intended behavior is the
state table above.

Scope: the tunnel ingress secret check, the device secret store and its
audit, the relay publish and lookup frames, the Active pairing status
that delivers the secret, and `published_address` plus `rememberRelay` on
the device client. Deadlines and the pool of four are unchanged. Cloudflare
Access is not added. Settings does not gain a named-tunnel recommendation.

The phone app in nessa-ios is not in this diff. What it calls is in
Consequences.
