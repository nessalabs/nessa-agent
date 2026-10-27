# Independent retention-gap contract review

## Result and reviewed source

**No open findings in the final scoped source.** The initial audit-evidence
finding is closed by the test changes and observed GREEN results below. This is
same-reviewer closure of that finding; the coordinator is arranging a fresh
clean closing round as required by the repository review loop.

Reviewed HEAD `3494d2aa873e831a8d551b21aaa2a5def0ab2088` with uncommitted changes
to the seven files listed below. The runtime diff is the camelCase serde mapping
on WireGap; its other source addition only registers the private test module.

Seven-file path-NUL-content SHA-256: `c161bea4890899ecc6eadcfde064fdb439c1aab98f0a86b1f0444ad02ab27ff8`.

## Closed finding and evidence

**[P2, closed] Assert durable cloud-gap audit at the HTTP-to-store boundary.**
The original new test asserted a saved gap/cursor/events but would have stayed
green if the cloud-gap audit insert disappeared. The final test reads the real
encrypted audit and asserts user11/repository42, before0/after1,
`cloud_retention_gap`, `system:cloud-relay`; a reopened paused retry does not fetch
again or duplicate evidence. Owner acceptance is separately attributed to
`local-owner:github:11`, preserving the original cursor until acceptance.

Trigger refusal of either the cloud-gap audit or acceptance audit returns typed
Storage and rolls back to the prior cursor/gap/evidence. Removing the trigger and
replaying succeeds. Malformed/stage-refused receipts leave no gap audit, cursor
advance or event. These are the requested transition failure/cleanup variants,
not a second runtime audit implementation.

I read the actual coordinator-owned logs rather than treating its statement as
proof:

- `/tmp/nessa-1158-gap-red-wire.log`: with only the camelCase mapping absent, the
  first real HTTP `.page` assertion reports `left: Some(InvalidResponse)` versus
  `right: Some(Gap(1))`; the test fails before database setup. This is the valid
  load-bearing RED reproduction.
- `/tmp/nessa-1158-gap-green-final.log`: the mapping is restored in the inspected
  final source; all **3 tests pass** (0 failed, 0.38seconds execution): wire gap
  through durable pause/acceptance/replay; invalid receipts; cloud-gap audit
  refusal/recovery.
- The initial crypto-provider panic and the later private-folder Storage setup
  failure occurred before decoding. They are explicitly **excluded** as revert
  proof. The final fixture configures an explicit per-client ring provider,
  creates its database in `private/inbox.sqlite`, and has top-level type imports.

No Cargo, Worker, browser or provider command was executed by this reviewer.
The tests were run by the coordinator; I inspected their exact outputs and source.
`git diff --check` independently passed during this final review. Final combined
GitHub tests, strict Clippy and new-head hosted CI remain coordinator-owned.

## Relationships and boundaries checked

- Worker `readRelay` uses its persisted expiry watermark and returns
  `{kind: gap, expiredThrough}`; its HTTP adapter adds current stage and status410.
  The TS regression compares the real bundled Miniflare Worker response with
  the shared fixture. A fresh database's first expired delivery has sequence1.
  Both references resolve to the same root protocol fixture.
- `HttpRelaySource::page` decodes the current camelCase response, then checks
  stage and kind before reporting the typed gap. The change adds no snake_case
  compatibility alias, protocol version, fallback or validation bypass.
- `catch_up` checks watermark ordering and the existing safe-integer ceiling
  before `RelayStore::gap`; the store commits gap and audit atomically before
  reporting it. The negative HTTP receipts cover wrong stage/kind, missing or
  snake_case watermark, stale0 and above-safe-integer values before effects.
- Saved gaps remain paused across reopen. The direct owner bind acknowledgement
  tested here atomically clears the stored gap and advances to its watermark;
  valid trailing page2 still checks user11/repository42 and ordered cursor2.
  The existing `opt_in_survives_restart_without_resetting_the_cursor_and_disable_fences_old_pages`
  regression separately covers Settings' LiveEvents status gap count and
  select-based acknowledgement with a new binding generation. This new loopback
  test does not drive Settings UI itself.
- The test uses real loopback HTTP and real SQLCipher. The client/configuration
  fixture is private to the adapter's child test module. It introduces no
  production visibility, alternate shipped constructor or runtime test switch;
  production HTTPS-only origin validation remains unchanged. Empty test TLS roots
  do not disable validation, and no global crypto-provider state is changed.
  Local HTTP intentionally substitutes for the external TLS service; TLS is not
  this regression's claim. Scoped abort-on-drop owns the test server, and requests
  have a five-second bound. No real credentials, cloud deployment or provider
  resources are involved.
- Ownership remains in transport/application/storage; the shared receipt is under
  protocol fixtures and is linked in the owning maps. The ADR table includes the
  representation gap and audit-refusal rows before implementation. No Cedar,
  selection authority, retention policy, UI or rollout stage rule is changed.
- None of these paths is an input in the explicit lease source fingerprint list
  in env/config.rs. Replacing the published worker image is not required for this
  unrelated native relay serialization correction.

## Limits

This is a focused source/boundary review, not a fresh audit of the375-file stack,
TLS validation, all pre-existing HTTP error-body resource bounds, production
Cloudflare behavior or native UI. No external penetration-test claim is made.
A later source change needs its own attribution; the earlier12-check green run
at3494 does not constitute hosted approval of this uncommitted correction.

## Reviewed files

- `crates/nessa-server/src/github_relay/infrastructure/transport.rs`
- `crates/nessa-server/tests/github_relay/transport.rs`
- `protocol/fixtures/github-relay-gap.json`
- `packages/github-webhooks/tests/relay/relay.test.ts`
- `docs/adr/todo/1152-local-vault.md`
- `packages/github-webhooks/README.md`
- `protocol/README.md`
