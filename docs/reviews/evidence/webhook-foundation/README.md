# Signed webhook foundation evidence

Source reviewed: `04a421a47711a3c326a7cd4e1ab23bd9cf17f1b8`.
Base: `0b8019cf45f99b285f10c41199875d474492c6d1`.
This is a first, uncomposed slice of #896. It does not satisfy the issue's
registration, durable outbox, capability filtering or revocation E2E gates.

## Local checks

- Rust toolchain: 1.99.1, installed under `/tmp`; no system toolchain changed.
- `cargo test -p nessa-server --lib webhooks --no-fail-fast`: 26 passed at
  `04a421a4`, zero failed/ignored; 3748 unrelated tests filtered out. See
  [`cargo-webhooks.txt`](cargo-webhooks.txt). The exact-source isolated module
  harness also passes 26 after the test-ownership and owner-DTO corrections.
- CI package selection Clippy passed with `--all-targets -- -D warnings`:
  local-storage, auth, server, protocol, client-core, device-client and SDK.
  Rustc emits a forced long-constant-evaluation notice for the existing
  `env/config.rs` lease-protocol digest; the command exited zero.
- Architecture and formatting passed before final docs were added. No protocol,
  UI, grant or credential implementation changed. Scripted desktop/stress was
  not run: this slice is not composed into any conversation or gateway flow.

## Independent review and dispositions

The first fresh reviewer inspected the working foundation, its W-* contract,
installed reqwest/Hyper code and isolated probes. It found two major defects:

1. Generic request errors do not prove zero response bytes. A loopback probe
   showed empty, truncated-header and malformed-status responses have the same
   error predicates. The mapper now returns IncompleteResponse/Unknown, with no
   retry. Regression: `lost_or_malformed_headers_do_not_claim_zero_response_bytes`.
2. A Busy result after saving Sending had no honest resumable state. Preparation
   now reserves transport/DNS before admission, carries the same deadline, and
   returns a consumed POST handle whose error type has no Busy variant.
   Regressions: `local_backpressure_precedes_sending_and_does_not_consume_an_attempt`,
   `retained_dns_capacity_refuses_preparation_while_delivery_stays_queued`, and
   `expired_preparation_refuses_before_polling_a_request`.

Minor layout and body/header-bound documentation findings were also corrected.
The second fresh reviewer checked committed `d3b3db4d`, found no additional
behavioral defect, and requested moving JSON/HMAC tests out of application.
Commit `04a421a4` moves tests to their owners and adds a regression using the
actual `RecordWireRecord` DTO. The framing golden remains explicitly synthetic.
The third fresh reviewer checked the combined base-to-`04a421a4` diff and the
test-ownership correction, independently recomputed the MAC and varied all three
framed fields, and reported no findings at any priority for this foundation
scope. It did not approve the unimplemented full issue. Reviewers did not run
Cargo or CI; their source and independent probes are distinct from the
implementer's executed checks. The draft PR records current-head CI separately.

## Deliberate fail and restore probes

[`mutations.json`](mutations.json) contains 14 verified one-rule changes, their
before/after text, original and modified SHA256, exact regression, and failed
test output. All 14 tests failed after the mutation. The mutation was verified
in the file before compiling; each copy was restored and checked byte-for-byte
against the checkout afterward. The final restored copy passed all 26 tests in
[`isolated-module-restored.txt`](isolated-module-restored.txt).

These probes used copied `src/webhooks` and `tests/webhooks` trees in `/tmp`,
compiled by rustc against the exact dependency rlibs already built by Cargo.
The small harness supplies the same ring-provider installation helper. They do
not mutate the production checkout, rebuild shared artifacts, test composition,
or replace canonical Cargo/CI verification. HTTP probes use loopback fixtures
and test-only keys. No proxy-policy mutation or public recipient was used.

The first scheme probe exposed a missing `http://host:443` negative input;
that input was added and the probe then failed as required. An initial capacity
probe targeted the DNS constructor rather than the HTTP constructor; its target
was corrected and the applied hunk verified. Neither initial result was counted
as a successful fail probe.

## State documentation

The [state page](../../../state/services/gateway/webhook-foundation.md) records
the exact source checkpoint and explicitly distinguishes values, adapter
effects, acknowledgements and unimplemented recovery/authority owners.

Nessa Docs renderer `c0bc0627611b100ab71a96cc6fe2fb65f1a014d8` validated 120
documents with zero issues. Both charts rendered in installed Chromium
151.0.7922.173 with zero page errors; Fit was applied after fonts loaded and the
screenshots were inspected for clipping. See [`render.json`](render.json),
[delivery value chart](diagram-1.png) and [transport chart](diagram-2.png).
The Playwright browser download was denied with `403 Domain forbidden`; no
alternate download route was tried. Rendering used the existing local browser.

## Outstanding work and access limits

The gateway still has no webhook registration or worker. No granted source is
read, no outbox/cursor or causal audit is persisted, and no grant/credential
permit is held by this slice. Domain recovery alone is not crash/restart
delivery. One-chat filtering, unshare/revoke, stale epoch before POST and
forbidden conversation.send need full integration E2E before #896 can close.
There is no exactly-once remote-effects claim.

Project-board lookup was denied: `gh api graphql -F n=896` with the repository
issue/projectItems query returned `Post "https://api.github.com/graphql": Forbidden`.
No alternate board API was tried. GitHub issue/PR operations use the connected
GitHub tools. No live registration, persistent real key, production transmission,
public listener, account/security change, merge or force-push was performed.
