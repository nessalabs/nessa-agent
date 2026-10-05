# 483. The gateway's contract is its own crate; the device client depends on it, never on the gateway

## Purpose

A phone app has to link the device-side sync client without linking the
gateway. This record fixes where the code both ends of a gateway connection
share lives, and which way the crates depend on each other, so the device
client links independently of `nessa-server` (issue
[#483](https://github.com/nessalabs/nessa-agent/issues/483)).

- **Date:** 2026-10-04
- **Status:** accepted

## Context

The device client (`crates/nessa-server/src/read_only_sync`) started as test
scaffolding beside the gateway. It reached into four other parts of the
gateway crate, and two of them the gateway needs as well: the product wire
(frames, generated DTOs, the read codecs, the handshake rules, native pairing
framing and its deadline socket) and the conversation read model
(`ConversationView`, the projection that folds committed records into it,
catalogue metadata and its payload codec, the read-scope checks). Neither can
move into a device crate, because the gateway must not depend on the phone's
client; and neither can be copied, because two copies of one rule drift
([gate 13](../../../CODING_STANDARDS.md#gates)). The issue's text reads as
"the gateway depends on the device crate", which would put the SQLite cache,
the pairing client and a tokio runtime upstream of the gateway's build.

## Decision

Two crates. `nessa-protocol` holds what both ends of a gateway connection
agree on and nothing only one end does: wire frames and the payloads
generated from the protocol schemas, the product contract's outcome values,
the product DTOs, the read codecs and handshake rules, native pairing framing,
envelope codec, enrollment channel and deadline-and-wake socket, the monotonic
`Clock` port that socket reads, and the conversation read model.
`nessa-client-core` holds the device client. The dependency direction is
`nessa-server → nessa-protocol ← nessa-client-core`, and `nessa-protocol`
depends on neither. The gateway reaches `nessa-client-core` only as a
dev-dependency, for the tests that enrol a real device and run the client
against a composed gateway; nothing the gateway ships links it. The generators
write into `nessa-protocol`. Its admission rule is no process state and no
runtime of its own; `pairing::socket` is the one deliberate exception
(blocking std socket mechanics both ends need), and `tokio` is taken with
`rt` only, for the one shared worker-fault mapping. The rule is enforced by a
package denylist in `scripts/architecture/rust-dependency-graphs.mjs`.
Gateway-only rules stay in the gateway, including the SDK session for a
conversation and the mapping from access errors to passive-read refusals.

Implementation has two slices: the first extracted `nessa-protocol`; the
second moved enrollment, retained sync, matching tests and example composition
into `nessa-client-core`. Gateway online process tests consume the public client
entry point.

## Alternatives considered

- **One device crate the gateway depends on.** One crate fewer, but the
  gateway would build and link the phone's client, the device crate would sit
  upstream of every gateway change, and a future binding crate would have
  nowhere to take the wire alone.
- **A port in the device crate for transcript rendering, implemented by the
  gateway.** Keeps the projection in the gateway, but makes every device
  client depend on the gateway to draw a transcript, which is what the issue
  is removing.
- **Copies of the shared pieces in each crate.** Rejected by gate 13: the
  selector checks, the payload codec and the projection would each have two
  owners.

## Consequences

The gateway's product and conversation code imports from `nessa-protocol`,
so a change to the wire or the read model is a change to that crate, and its
public surface is wider than the old `pub(crate)` items were (the read codecs,
the projection's bounds, `DeadlineStream`). A crate named for a contract
attracts anything shared; the admission rule above is what keeps it to the
contract, and the sign it has stopped being right is a type in it that only
one end uses. `scripts/architecture/rust-dependency-graphs.mjs` keeps both
crates free of the desktop framework, rejects every renamed/transitive client
path to `nessa-server`, and refuses every gateway-to-client path made only of
normal/build dependency edges. Dev-only edges, including downstream dev edges, are excluded from that
production path check.

## Policy and test ownership

The two synchronous deadline adapters own different policies. The protocol's
pairing socket owns TLS/enrollment phase budgets, wake-tick read retries,
terminal send failure, and transfer to `BufferedIo`. The client's protected
socket owns operation budgets, cancellation, post-I/O deadline checks, first
sanitized failure evidence, and shutdown on drop. Similar timeout arithmetic
is not a second implementation of either policy. The native socket deadline
suite and the client's gateway deadline suite independently enforce them.

The deterministic saved-output race belongs with the private cache owner and
production output mapper in the client crate, using the existing private test
scheduling hook. The gateway keeps real authenticated process and saved-output
checks against the public client entry point. No production test hook crosses
the crate boundary.
The public client surface follows current callers: gateway enrollment tests use
`pairing::{NativeEnrollmentClient, NativeClientError, NativeRetryOutcome}`;
gateway online tests and child fixtures use `composition::execute`; the example
uses `composition::run_read_only_example`. Root `CommandError` is an immutable
diagnostic facade retaining the original six-variant cause privately;
Debug/Display stay unchanged and JSON owns machine
outcomes. This keeps SDK diagnostic strings private rather than widening their
mutable error representation. `NativeRetryOutcome` keeps receipt fields private
and exposes borrowed accessors. Retained-sync implementation, SQLite mutation and
reset APIs remain internal; a future binding is not a current caller requiring
them to be public.
