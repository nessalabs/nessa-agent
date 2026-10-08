Both Codex findings are accepted at `441996826c1717a817f884b35ac3ab0d284e9a70` and will be fixed before merge.

The structural cause of P1 is that admission commit and completion delivery have different owners. `finish_recovery` will own the sender and validate, deliver, and commit under the existing registry fence; a failed delivery will fence admission before the writer takes another frame. The same terminal owner will make a valid pre-deadline success irreversible, so a later timeout observer cannot publish a contradictory failure. A timeout that wins first prevents admission. No additional state or request ledger is needed. P2 will share the existing HTTP response classification and retain Unauthorized/Unconfirmed/scope failure rather than classify HTTP failures as malformed protocol.

```mermaid
sequenceDiagram
  participant Startup
  participant Session as Recovery terminal owner
  participant Writer
  Writer->>Session: initialized accepted; completion sender
  Session->>Session: validate deadline/close under fence
  alt receiver accepts before valid commit
    Session-->>Startup: success
    Session->>Session: commit Completed
    Startup->>Session: late timeout observation
    Session-->>Startup: preserve committed success
  else timeout or delivery failure wins
    Session->>Session: commit Failed typed cause
    Session-->>Writer: End before next frame
  end
```

Canonical ADR orderings, deterministic regression tests, mutation probes, a fresh whole-diff review and final-head checks will precede another merge decision. Previous-head check passes are retained as historical evidence, not approval of a later source.
