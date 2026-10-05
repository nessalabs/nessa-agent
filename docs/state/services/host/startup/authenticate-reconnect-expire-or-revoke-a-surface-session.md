---
id: "host-startup-authenticate-reconnect-expire-or-revoke-a-surface-session"
title: "authenticate, reconnect, expire, or revoke a surface session"
kind: "statechart"
status: "implemented"
summary: "The native panel can obtain its gateway credential only from an allowed bundled window, for the expected stage and local destination."
parent: "host-startup"
sources:
  - "src/session/adapters/client/credential-source.ts"
  - "src/session/adapters/client/credential-source.test.ts"
  - "src-tauri/src/surface_credential.rs"
  - "src/session/adapters/client/dev-session.ts"
  - "src/session/adapters/client/dev-session.test.ts"
  - "src/session/adapters/lifecycle/supervisor.ts"
  - "src/session/adapters/lifecycle/supervisor.test.ts"
  - "src/session/adapters/lifecycle/session-lifecycle.tsx"
  - "src/session/adapters/client/authentication-failure.ts"
  - "crates/nessa-server/src/product/socket.rs"
  - "crates/nessa-auth/src/application/session.rs"
diagramLinks: {}
---

# authenticate, reconnect, expire, or revoke a surface session

The native panel can obtain its gateway credential only from an allowed bundled window, for the expected stage and local destination. Unsafe, missing or invalid credential storage is refused before its secret is used.

The host also allows the desktop window to read the panel credential, with the same stage and destination checks. Unlike panel and setup loads, the desktop read requires an already-ready startup snapshot and cannot trigger reconciliation. [GatewayReader](../../../../../src-tauri/src/gateway/infrastructure/commands.rs) owns that distinction.

Connection setup retries temporary failures with a limited backoff. Terminal failures need explicit Retry. Old connection attempts cannot replace the current one. Reconnect does not replay conversation commands. Revocation blocks later admitted commands, but work already admitted can finish.

```mermaid
stateDiagram-v2
    [*] --> Connecting
    Connecting --> Discovering: Native credential source
    Discovering --> Authenticating: Verified endpoint and assigned token
    Authenticating --> CheckingHealth: ProductSessionReady
    CheckingHealth --> Ready: Authorized server.health succeeds
    Discovering --> Backoff: Retryable transport failure
    Authenticating --> Backoff: Retryable connection failure
    CheckingHealth --> Backoff: Retryable health cause / close client
    Discovering --> Terminal: Nonretryable refusal
    Authenticating --> Terminal: Authentication lost
    CheckingHealth --> Terminal: Nonretryable health cause
    Ready --> Reconnecting: SDK transport reconnect / clear handle
    Reconnecting --> Ready: Connected / reset attempts
    Reconnecting --> Backoff: Retry window exhausted
    Ready --> Terminal: Revoked or expired or authorization lost
    Backoff --> Connecting: Delay elapsed / fresh generation
    Terminal --> Connecting: Explicit retry / new lifetime
    Connecting --> Disposed: Dispose
    Backoff --> Disposed: Dispose / cancel timer
    Ready --> Disposed: Dispose / close client
    Reconnecting --> Disposed: Dispose / close client
    Terminal --> Disposed: Dispose
    note right of Backoff
        500 ms exponential delay, capped at 5 s.
        Honor larger retryAfterMs.
        Connection setup only; commands not replayed.
        Stale generation results discarded and closed.
    end note
```

## Further reading

[Source](../../../../../src/session/adapters/client/credential-source.ts) · [Related source](../../../../../src-tauri/src/surface_credential.rs) · [Related tests](../../../../../src/session/adapters/client/credential-source.test.ts)
