# Typed app review verification

Source base: `5d254cd45f14603e6a381e3be446c7f4309217a6` (2026-10-13).
The screenshot is the real app-review fixture card at 340 CSS px, captured
headlessly by Playwright Chromium. Chromium and WebKit ran both columns and
sidebar layouts in development mode. The initial matrix used the read-only
pinned UI at `5963247f906d9cba4457bc98f2bb6cbf70e5e379`; subsequent scripted
verification uses a private snapshot of that same source and private dependencies.

```text
smoke: 32/32 held
app-review: 36/36 held
app-review: 12/12 held (restored target/review/message rows)
Tests 121 passed (gateway source and captured fake answers)
architecture: 102/102 tests; architecture check passed
verification library: 164/164 passed
```

Each negative probe changed only its named rule, printed the changed source,
failed, and restored the original source byte for byte:

```text
old four-field consumer: TS2493 (tuple has no index 3), TS2367 (target is not string)
old tuple runtime: the card is still drawn 5 s after Allow; app's call came back waiting
conversation comparison removed: another conversationId settled the original review
execution comparison removed: another executionId settled the original review
permission comparison removed: another permissionId settled the original review
generic denial restored: tool Deny left outcome failed, review null
callback reads last answer: expected turn/review/allow/first; received other-turn/other-review/deny/second
```

The identity checks above cover the fixture's pending request correlation.
Production authorization and SDK target issuance retain their existing owners;
these fake-gateway results are not evidence of server authorization.
