## Problem and behavior

During HTTP session recovery, 64 queued ordinary frames could fill the outgoing queue and silently discard an early peer `ping` reply. A peer waiting for that answer could then withhold its initialize result until recovery timed out.

This change keeps one serialized FIFO with 64 ordinary-frame permits and 65 physical slots for HTTP. Peer replies and `RecoveryReady` bypass ordinary permits, so ordinary saturation leaves room for a control message. Permits release when the writer dequeues a frame. Controls retain FIFO order and may use other free slots; the reserve does not promise priority or progress under control saturation. Stdio retains its existing 64-slot behavior.

```mermaid
sequenceDiagram
    participant Calls as Ordinary callers
    participant Queue as Connection FIFO
    participant Reader as HTTP reader
    participant Writer as Serialized writer
    participant Peer as Recovery peer
    Calls->>Queue: Queue 64 ordinary frames
    Peer->>Reader: Early ping with captured response context
    Reader->>Queue: Admit PeerReply using remaining physical slot
    Writer->>Queue: Dequeue ordinary frames and release permits
    Note over Writer: Recovery phase refuses queued ordinary calls
    Writer->>Peer: Send ping answer using captured binding and version
    Peer->>Reader: Matching initialize result
    Reader->>Queue: Enqueue RecoveryReady within startup deadline
    Writer->>Peer: Send initialized notification
    Note over Writer: Confirm readiness before ordinary dispatch
```

If controls exhaust all physical capacity, peer-answer admission returns typed `TooLarge` through the existing reader shutdown fence and first-cause owner. A closed queue uses its retained terminal cause or `ServerGone`. Recovery deadlines, cancellation, captured response binding, and once-only cleanup remain owned by the existing session.

```mermaid
sequenceDiagram
    participant Peer as Recovery peer
    participant Reader as HTTP reader
    participant Queue as Connection FIFO
    participant Session as HttpSession
    participant Shared as Connection terminal owner
    participant Calls as Pending callers
    Peer->>Reader: Another request while all 65 slots are occupied
    Reader->>Queue: Try to admit PeerReply
    Queue-->>Reader: Full
    Reader->>Session: Shutdown and fence admission
    Session->>Session: Retain once-only reader and DELETE cleanup
    Reader->>Shared: End with TooLarge
    Shared-->>Calls: Settle pending calls with TooLarge
    Note over Queue,Session: Fence prevents queued reply effects
    Note over Shared: Later close retains the first cause
```

## Verification

Initial author checks passed: formatting, six-package all-target Clippy with `-D warnings`, HTTP progress (56 passed), and full MCP (221 passed, zero failures, one existing ignored test). Six deliberate rule mutations produced failing runtime regressions, followed by a successful restored HTTP progress run. Broader suite checks, independent review, browser evidence, and hosted CI are being assembled; final exact-head results will be recorded before merge.

ADR 392 ordering rows J21–J26 cover saturated recovery answers, matching initialization before drain, control overflow/closure, permit release/cancellation, deadline expiry, and close or writer failure before dispatch. Regressions reuse the existing in-process HTTP fixtures; no subprocess fixture is added.

A custom HTTP adapter unwind can separately leave writer loss without immediate terminal publication; that confirmed adjacent gap is tracked in #670 and is outside this queue admission fix.

Closes #665.
