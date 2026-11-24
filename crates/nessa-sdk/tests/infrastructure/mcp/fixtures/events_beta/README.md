# MCP Events beta loopback fixture

This independent Python standard-library peer models a synthetic CI feed.
It requires no account, environment secrets or external network access.
It binds only 127.0.0.1; port zero is the default and the first stdout line
is its assigned endpoint. Stop with Ctrl-C if launching manually.

```sh
python3 -B crates/nessa-sdk/tests/infrastructure/mcp/fixtures/events_beta/server.py --profile replay --format json
python3 -B crates/nessa-sdk/tests/infrastructure/mcp/fixtures/events_beta/server.py --profile replay --format sse
python3 -B crates/nessa-sdk/tests/infrastructure/mcp/fixtures/events_beta/server.py --profile push-only
cargo test --offline --locked -p nessa-conversation --lib events_beta_tests
```

The Rust tests launch and reap their own child processes. Replay fixture:
discover, paginated list, select `ci.build_fínished` with
`{"project":"paper-kite"}`, bootstrap with null cursor, get the first build
result, observe a deterministic retention gap, then resume at the replacement
checkpoint and get the second result. The alternate fixture advertises only
push for the selected type, so there must be no poll. It does not implement
push itself. One peer implementation serves both profiles. Normal Transport
routing, including the Unicode Mcp-Name sentinel, is checked independently.
No upstream subscriptions, workflow hooks, or OAuth. The tests use the gateway's existing Reqwest HTTP exchange without
modifying product composition.

Normative input: MCP Events Beta spec and MCP 2026-07-28 Streamable HTTP:
- https://github.com/modelcontextprotocol/ext-events/blob/main/specification/beta/events/events.mdx
- https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/streamable-http
