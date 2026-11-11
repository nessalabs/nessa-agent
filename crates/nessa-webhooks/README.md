# Webhooks

Outbound webhook delivery of committed conversation records: registrations,
the private durable outbox, the exact signed message bytes, and the bounded
HTTPS sender.

```text
nessa-server ──▶ nessa-webhooks ──▶ nessa-auth, nessa-protocol, nessa-sync
                               └──▶ nessa-local-database, nessa-local-storage,
                                    nessa-timer, nessa-agent-install (TLS backend)
```

Arrows are compile-time dependencies. nessa-server imports it as
`nessa_webhooks`. The parts that meet the conversation context stay in the
server's `webhooks` module: admission against current credential and grant
facts (`application/admission.rs`), the principal share (`share.rs`), the
canonical record source (`source.rs`), and the runtime that drives a delivery
(`infrastructure/runtime.rs`).

| Path | Responsibility |
| --- | --- |
| `src/domain/` | Delivery, destination and registration lifecycle values. No clock, network or storage reads. |
| `src/application/` | Message bytes, registration and stop ports, the outbox port and its limits, and `Target`, the identity a registration delivers as. |
| `src/infrastructure/outbox/` | The private SQLite outbox: state and causal audit in short transactions. |
| `src/infrastructure/` | HMAC signing, the record message, and the HTTPS sender with its DNS checks. |
| `tests/` | Unit tests, included with `#[path]`. The `test-support` feature gives nessa-server's product delivery tests the in-memory recipient CA (`tls_fixture`), the isolated POST seam and the admission pauses. |
