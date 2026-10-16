# Thread-tool delivery authorization

Scope: `composition/conversation_tools.rs` and its subscription seam tests,
based on `fd55f18dd609f730f4a7d7f52e3a60e3df5f36b2`.

The structural defect was separate Page, Idle, and cached-deadline exits after
access was admitted only before source I/O. One final delivery boundary now asks
the existing target access owner before returning any successful observation.
The admitted source caller remains captured; relay cancellation/token revocation
uses its existing watch channel. Source delegation and source audit are not
repeated. The final target check has a fresh bounded attempt using the existing
ten-second setup budget; timeout is `temporarily_unavailable`, without cursor.
Its policy decision is the delivery admission point, not an atomic socket-write
or revocation guarantee. Design orderings were updated before implementation.

## Regression scope

- `delivery_reauthorizes_page_idle_and_cached_deadline_after_target_deletion`:
  twelve deterministic outcomes through the actual follow/service/policy/storage
  port path. Allowed/deleted targets are covered for Page, Idle, and deadlines
  while read, reauthorization, or idle sleep is pending. An independent Capacity
  read failure stays `subscription_capacity` even when the target was deleted.
- `blocked_delivery_authorization_cancels_or_times_out_without_cursor`: source
  cancellation and ten-second deadline while final metadata admission is blocked.
- `already_observed_cancellation_refuses_delivery_before_polling_authorization`:
  current true watch value is refused even after its version was observed;
  the authorization future is not polled.

The focused tools suite passed 10 tests before mutation. Two isolated revert
probes were verified in source before compilation:

1. Removed only the central final authorization match. The deletion regression
   failed because it received an `Ok` event batch containing `turn.ended`,
   execution `finished`, and cursor, instead of the typed deletion refusal.
2. Restored that match and removed only the current-stop precheck in
   `step_until`. The observed-cancellation regression failed its expected
   `SubscriptionEnd::Cancelled` assertion.

Both probes returned exit 101 with one failed targeted test. Green source was
restored using a fresh modification time before the final verification run.
Local raw logs are `/tmp/953-late-authorization-tests.log`,
`/tmp/953-late-authorization-mutation-central.log`,
`/tmp/953-late-authorization-mutation-stop.log`, and corresponding `.diff` files.
The coordinator owns assembled strict Clippy and subsequent independent review.
The adjacent socket event delivery path was inspected and handed to its API owner
for its separate post-read authorization change.

## Restored verification

On the restored tree:

- `cargo test -p nessa-server composition::conversation_tools:: --lib`: 10 passed.
- `cargo test -p nessa-server agent_tool_tests:: --lib`: all five source
  admission/audit/actual Relay integration tests passed.
- `cargo test -p nessa-server agent_tools --lib`: two passed (the actual Relay
  test above plus the Cedar source authority truth-table test).
- Direct rustfmt checks and `git diff --check` passed.

Restored run logs: `/tmp/953-late-authorization-restored.log`,
`/tmp/953-late-authorization-source.log`, and
`/tmp/953-late-authorization-relay.log`. Cargo used `/workspace/target` in the
Linux root checkout, serially. The existing lease-protocol constant-evaluation
warning appeared during compilation; assembled strict Clippy remains with the
coordinator after integration.
