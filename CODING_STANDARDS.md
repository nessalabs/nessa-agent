# Coding standards

Nessa-specific gates for every pull request. The general working method lives in
the **`coding`** skill ([nessalabs/skills](https://github.com/nessalabs/skills));
this file is what reviewers and agents **must check before merge**.

If a change fails a gate below, fix it in the same PR — do not land “clean up
later.”

## PR gating checklist

A PR is not ready to merge until every applicable item is true.

### Errors and failures

- [ ] **Typed errors, not string matching.** Classify and branch on enums /
      variants / `ErrorKind` (and downcast cause chains when wrapping foreign
      errors). Do **not** decide control flow or log level by
      `error.to_string().contains(...)`.
- [ ] **Errors carry meaning.** Prefer domain types (`EnvironmentError`,
      `DecodeError`, `WsReadFailure`, `RunError`, …) over bare `String`,
      `anyhow::Error`, or untyped `Box<dyn Error>` at module boundaries.
- [ ] **Expected vs unexpected is explicit.** Benign teardown (client reload,
      quit, peer reset) is a typed variant logged at debug; real faults warn or
      fail. Do not warn on happy-path disconnects.
- [ ] **Survivable host/edge failures stay survivable.** Match existing policy:
      frost, tray, viewport, and similar edge failures degrade and log with the
      `[nessa]` prefix — they do not abort launch. See
      [docs/codebase-structure.md](docs/codebase-structure.md).

**Good (typed):**

```rust
match WsReadFailure::classify(&error) {
    WsReadFailure::Disconnected => tracing::debug!(%error, "websocket disconnected"),
    WsReadFailure::Unexpected => tracing::warn!(%error, "websocket read error"),
}
```

**Bad (stringly):**

```rust
if error.to_string().contains("without closing handshake") { /* ... */ }
```

In TypeScript, the same bar: discriminated unions / narrow classes for failure
modes at boundaries (`SessionHealthError`, stage config errors), not parsing
`message` strings to choose behavior.

### Shape and boundaries

- [ ] **Change sits in the right owner.** Check
      [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and
      [docs/codebase-structure.md](docs/codebase-structure.md) — no new
      cross-boundary leaks (domain importing Tauri/serde transport, shell owning
      product rules, OS `cfg` sprinkled outside `platform/`).
- [ ] **No drive-by refactors.** Diff is scoped to the stated problem; unrelated
      cleanup is a separate PR.
- [ ] **Public seams stay dual.** Host event names / payloads that cross into the
      webview stay declared on both sides (`src-tauri` host + `src/host/window.ts`)
      with the existing drift test still green.

### Tests and verification

- [ ] **Failure modes are tested.** New error variants or classifiers have unit
      tests on the typed cases (not on Display text).
- [ ] **Relevant checks pass.** At minimum what the change touches, e.g.
      `pnpm check` surface for TS, `cargo test` / `cargo clippy -D warnings` for
      Rust crates you edited.
- [ ] **Linux is not “later.”** Host/window changes must not only work behind a
      macOS `cfg` when the behaviour is product-visible on Linux.

### Product honesty

- [ ] **No fake success.** If a capability is not wired (e.g. chat send before
      turn RPCs), it stays an explicit no-op or clear status — do not invent
      turns or pretend the server answered.

## When you add a new error type

1. Put it next to the module that owns the failure (`env/error.rs`,
   `protocol/decode.rs`, `server/entrypoint/ws_error.rs`, …).
2. Implement `Display` + `std::error::Error` (Rust) or a narrow class / union
   (TypeScript).
3. Preserve `source()` / cause chain when wrapping foreign errors.
4. Match on the type at the call site; never re-parse the message.
5. Cover the variants that change logging or control flow with tests.

## Out of scope here

Formatting, import order, and generic Rust/React style live in the **`coding`**
skill and the repo’s formatters/linters (`pnpm format`, `pnpm lint`,
`cargo fmt`, `cargo clippy`). Architecture and absences live under `docs/`.
This file only gates **merge readiness** for Nessa PRs.
