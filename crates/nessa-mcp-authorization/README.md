# MCP authorization

One remote MCP server's consent, token generation, refresh and revoke
(ADR 392). The domain chooses each transition and names its effects; the
owner performs one effect at a time; a token is never written to a log or to
`config.json`.

```text
nessa-server ──▶ nessa-mcp-authorization ──▶ nessa-sdk, nessa-agent-install (TLS backend)
                                        └──▶ nessa-local-storage, nessa-timer
```

Arrows are compile-time dependencies. nessa-server imports it as
`nessa_mcp_authorization`; the product methods `mcpServers.authorize` and
`mcpServers.revoke`, the MCP server settings that fence a URL change through
the owner, and composition stay in the server.

| Path | Responsibility |
| --- | --- |
| `src/domain/` | The authorization statechart. No socket, store or clock. |
| `src/application/` | `AuthorizationOwner`, discovery, and the ports it drives. |
| `src/infrastructure/` | HTTPS discovery and token calls, the loopback callback, the non-secret record beside sealed token files, the audit file, and the transport adapter that names an admitted token for the HTTP session. |
| `tests/` | Unit tests, included with `#[path]`, and `bounded_persistence_benchmark.rs`, a manual ignored real-file benchmark. |
