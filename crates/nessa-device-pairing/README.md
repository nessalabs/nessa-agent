# Device pairing

Native device enrollment: an owner creates a one-time code, a device claims it
over TLS with a PAKE, and the owner approves the exact device key. Approval
carries the enrollment through to an issued device credential. The crate also
holds the local-network announcement, and the tunnel: the public URL a paired
device reaches this gateway through, whose ingress hands each upgraded
connection to the same listener.

```text
nessa-server ──▶ nessa-device-pairing ──▶ nessa-auth, nessa-protocol, nessa-client-core
                                     └──▶ nessa-local-storage, nessa-timer,
                                          nessa-agent-install (TLS backend), shepherd
```

Arrows are compile-time dependencies. nessa-server imports it as
`nessa_device_pairing`. The receiver adapter over the conversation context's
`LocalReceiverAuthority` stays in the server (`device_pairing/conversation.rs`),
as do the owner product methods and composition. After a pairing cancellation
commits, the runtime asks the deletion inventory through
`nessa_protocol::conversation::fence_capture::FenceCapture`.

Device pairing and the tunnel are one crate because each needs the other: the
ingress feeds the enrollment listener, and native linking records the tunnel's
mode and token.

| Path | Responsibility |
| --- | --- |
| `src/application/` | Owner use cases, activation, cleanup, the receiver port, linking and the device status query. |
| `src/infrastructure/` | The runtime, connection workers, the listener, the protected product phase, owner commands, announcement and the linking audit. |
| `src/tunnel/` | Tunnel settings, secrets, the Cloudflare provider (pinned in `data/tunnel-releases.json`) and the ingress. |
| `tests/` | Unit tests, included with `#[path]`. The `test-support` feature lets nessa-server's composition tests substitute the invitation generator. |
