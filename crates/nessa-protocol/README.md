# Gateway protocol

What both ends of a Nessa gateway connection agree on, and nothing only one
end does. The gateway (`nessa-server`) depends on this crate, and so does a
device client; this crate depends on neither. Decided in
[ADR 483](../../docs/adr/todo/483-protocol-and-client-core-crates.md).

```text
nessa-server ──▶ nessa-protocol ◀── device client
                      │
                      ▼
      nessa-auth, nessa-sdk, nessa-sync
```

Arrows are compile-time dependencies.

**Admission rule.** No process state and no runtime of its own. A type belongs
here only when both ends use it; one used by only one end belongs to that end.
`pairing::socket` is the one deliberate exception: blocking std socket
mechanics both ends need, which a copy at each end would let drift. `tokio` is
taken with `rt` only, for the shared worker-fault mapping.
`PACKAGE_DENYLISTS` in `scripts/architecture/rust-dependency-graphs.mjs` refuses
a server framework, HTTP client, async TLS stack or WebSocket library anywhere
in this crate's graph (`node scripts/check-runtime-dependencies.mjs`).

**Generated code.** `protocol/generated_*.rs` are written by
`scripts/generate-protocol-types.mjs`; `product/generated.rs` and
`product_contract/generated.rs` by `scripts/generate-product-protocol.mjs`.
Change the generator, never the file; `pnpm protocol:check` compares bytes.

**Layer words.** `scripts/check-architecture.mjs` refuses a `use` naming
`product`, `infrastructure`, `composition`, `adapters` or `presentation` from a
`domain/` or `application/` file anywhere in the workspace, so an application
layer may import `nessa_protocol::product_contract::…` but not
`nessa_protocol::product::…`.

## Module map

| Path | Responsibility |
| --- | --- |
| `src/protocol/` | Wire frames (`frames.rs`), unique-key JSON (`json.rs`), the health response and payload bound (`encode.rs`), default shortcuts, and the generated payload types and method/event catalogue. |
| `src/product_contract/` | The product contract's generated outcome values and close policy. |
| `src/product/generated.rs` | Generated product DTOs, method and event names, bounds, and wire-shape validators. |
| `src/product/handshake.rs` | Version overlap and the refusal-to-close rule both handshake directions apply. |
| `src/product/passive_read.rs` | Passive read transport conversion and the capped response encoder. |
| `src/product/record_read.rs`, `src/product/catalogue_read.rs` | The record and catalogue page codecs, both directions in one file each. |
| `src/pairing/frames.rs` | The one length-prefixed frame reader and encoder. |
| `src/pairing/wire/` | The enrollment envelope codec; `status.rs` is the status it encodes. |
| `src/pairing/enrollment_channel.rs` | Enrollment envelopes over Auth's TLS transport. |
| `src/pairing/limits.rs` | Protected product frame bounds. |
| `src/pairing/socket/` | The deadline-and-wake socket (`deadline_stream.rs`, `wake.rs`) and the worker-fault mapping (`worker.rs`). |
| `src/clock.rs` | The monotonic clock port the socket's deadlines read. |
| `src/agents/` | `AgentId`, the agent names a conversation uses. |
| `src/conversation/domain/` | Conversation identity, model and approval choice, the summary a list shows, and catalogue identity. |
| `src/conversation/view.rs`, `src/conversation/projection.rs` | The `conversation.read` shape and the one bounded projection of committed records into it, with its `McpToolUis` port (`tool_uis.rs`), asked by conversation; which SDK session that is stays the gateway's rule. |
| `src/conversation/catalogue_metadata.rs`, `src/conversation/catalogue_payload.rs` | A catalogue entry's metadata and its one stored representation. |
| `src/conversation/read_scope.rs` | The read scope a passive read is admitted for, the checks of a source scope against it (catalogue scope identity included). Which access errors become which refusal, and which wire code a refusal answers with, are the gateway's (`access_refusal`, `refusal_code`). |
| `tests/` | Unit tests, mirroring `src/` and included with `#[path]`. |
