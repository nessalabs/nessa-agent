# Retention-gap wire follow-up

Source: `ed64020f5162d1601511724e439fd4ec9b1bf36d`, after the initial combined
head `3494d2aa873e831a8d551b21aaa2a5def0ab2088` passed all twelve hosted checks.
A late automated review correctly found the missing camelCase mapping in the
native HTTP410 decoder. The initial green run did not exercise this wire gap.

The producer sends `expiredThrough`. The private decoder now reads that current
spelling, with no snake_case alias or protocol version change. A shared receipt
in `protocol/fixtures/github-relay-gap.json` is compared with the bundled Worker's
actual HTTP410 JSON and consumed by the native HTTP adapter regression. The
native tests cross loopback HTTP into the real encrypted SQLite store.

## Verification

- Actual before-fix RED: `Some(InvalidResponse)` instead of `Some(Gap(1))` from
  the real HTTP adapter, before database setup. Log:
  `/tmp/nessa-1158-gap-red-wire.log`. The source lacked the mapping; adding it is
  the only production behavior change between this RED and GREEN.
- Fixed transport/storage: **3/3 passed**. Reopen retains the gap; a paused retry
  fetches nothing; explicit owner acceptance permits the next valid page.
  Authoritative encrypted audit rows retain target, actor, cause and cursors.
  Cloud-gap and owner-acceptance audit failures roll back and recover on retry.
  Wrong stage/kind, missing/snake_case watermark and invalid bounds leave the
  inbox/cursor and gap audit untouched.
- Eight-package GitHub selection: **70 passed**. Eight-package strict all-target
  Clippy passed. Worker **44/44** and Worker typecheck pass; changed package files
  and shared JSON pass Prettier.
- The initial TLS-provider panic and private-root storage rejection were fixture
  setup failures, not wire RED evidence. The final fixture uses explicit
  per-client ring configuration and the existing owned-private-child-directory
  pattern. Production HTTPS origin validation and storage guards are unchanged.

[Initial scoped review](gap-contract-review.md) closed its audit-evidence finding.
A [fresh closing review](gap-closing-review.md) reports no findings in this wire
repair. The separate local audit-cap observation is tracked in #1185; it is not
silently included in that scoped clean result.

The existing relay statechart already models durable Gap and explicit acceptance;
no lifecycle/diagram change is needed. Its source review checkpoint advances only
that owning page to the exact source above. Other state-page checkpoints remain
unchanged. The module maps now link both consumers of the shared receipt.

The UI source is unchanged, so the recorded real-gateway/Settings/scripted browser
checks and [26/36 broad-suite ledger](https://github.com/nessalabs/nessa-agent/pull/1158#issuecomment-6453732152)
remain source-specific evidence with their stated baseline limits. The new HTTP
regression does not claim to drive the Settings UI; its existing selection/gap
projection and generation test remains separate. TLS itself is outside this
loopback serialization regression. Lease fingerprint inputs, the verified Boat
image, Cloudflare deployment and Workers Free plan are unchanged. Final new-head
hosted CI is recorded on the PR after the push.
