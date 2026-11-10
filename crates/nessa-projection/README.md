# Conversation projection

The one bounded fold of a conversation's committed SDK records into the view
the gateway answers `conversation.read` with and a device shows offline.

```text
nessa-server ──▶ nessa-projection ◀── nessa-client-core
                   │          │
                   ▼          ▼
         nessa-protocol    nessa-sdk
```

Arrows are compile-time dependencies.

The view's shapes are `nessa-protocol`'s (`conversation::view`), so a client
can read a view without the SDK. Reading SDK records into them is here, so the
protocol does not depend on the SDK.

| Path | Responsibility |
| --- | --- |
| `src/projection.rs` | `Projection`, the live fold of execution updates, and `bound_view`, the one size bound every read goes through. |
| `src/records.rs` | How one SDK record reads in a view: `ViewOf` conversions and the few that need more than one record. |
| `src/tool_uis.rs` | The `McpToolUis` port: a tool's declared UI, asked by conversation; which SDK session that is stays the gateway's rule. |
| `tests/` | Unit tests, included with `#[path]`. |
