# Peer gateways

The dialing side of a peer gateway: this gateway enrolls, with its own native
key, into another gateway's peer invitation, keeps one private record per
peer, audits every change, and polls each kept peer for what it granted.

```text
nessa-server ──▶ nessa-peer-gateways ──▶ nessa-client-core, nessa-protocol, nessa-auth
                                    └──▶ nessa-local-storage, nessa-timer
```

Arrows are compile-time dependencies. nessa-server imports it as
`nessa_peer_gateways`; the product methods (`product/peers.rs`) and the
composition that starts and joins the poller (`composition/native_pairing.rs`)
stay in the server.

| Path | Responsibility |
| --- | --- |
| `src/application/` | The evidence the owner's commands hand to audit, and the `PeerAudit` and `PeerConnector` ports. |
| `src/infrastructure/records.rs` | One private record per peer, and the enrollment client's store for one enrollment. |
| `src/infrastructure/commands.rs` | The owner's enroll, list, forget and relay, and each peer's turn. |
| `src/infrastructure/audit.rs` | One private file per audit record, written in order by one writer. |
| `src/infrastructure/connector.rs` | The TCP adapter that dials a peer, racing its address with its relay. |
| `src/infrastructure/poller.rs` | Reads each kept peer's status and grants into a retained cache. |
| `tests/` | Unit tests, included with `#[path]`. The `test-support` feature lets nessa-server's composition tests watch read cycles and slow the audit writer. |
