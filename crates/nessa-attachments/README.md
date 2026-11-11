# Attachments

The files a person attaches to a conversation: upload tickets, the private
store each file is kept in, holds that say which conversation keeps it,
images fitted to the selected model, and reading a held file back.

```text
nessa-server ──▶ nessa-attachments ──▶ nessa-sdk, nessa-protocol, nessa-auth
                                  └──▶ nessa-local-storage, nessa-images, nessa-timer
```

Arrows are compile-time dependencies. nessa-server imports it as
`nessa_attachments`, and keeps in its own `attachments` module the two pieces
that meet the rest of the gateway: the HTTP upload route (`http.rs`, which
answers with the server's CORS rules) and the adapters that meet the
conversation context through its ports (`conversation.rs`).

| Path | Responsibility |
| --- | --- |
| `src/domain/` | Attachments, holds, upload tickets and their lifetimes. No disk or network. |
| `src/application/` | The attachment service, held reads, artifacts, and the ports they drive. |
| `src/entrypoint/socket.rs` | Uploads that arrive in pieces over a product session. |
| `src/infrastructure/` | The local store, durable audit, OS ticket secrets, and image normalizing. |
| `tests/` | Unit tests, included with `#[path]`; `support.rs` is the shared doubles, also built for nessa-server's tests under the `test-support` feature. |
