# Review brief: #262 weak-link scheduling

Branch `cursor/weak-link-scheduling-da70`. Base `origin/main`
(`a8dfeceb067ae70c2769bbcf5b16453d935330aa`).

## Design pass, before code

Intended behavior: one schedule for weak and metered links on the subscription
path. One active quantum. Priority is open transcript, catalogue, older
history, then one artifact range. Metered work requires a known quote no
larger than the remaining allowance before any byte moves. Sleep, unreachable,
reconnect backoff and a closed pace copy nothing. Measurements are a real SDK
source and private cache, paced and unpaced, plus a fixed size table.
Phone radio, battery, TLS billing and a production percentile are out of scope.

That pass's findings were folded into `docs/design/weak-link-scheduling.md`
before the Rust was written: the decision lives in `LinkSchedule`, quotes stay
out of `nessa-sync`, `follow` keeps its ending rows, and fragment reassembly
stays open.

## Implementation pass

Owned scope: the design note, `read_only_sync` domain/application/infrastructure
link and measure modules, their tests, the `weak_link_measure` example, and
`docs/design/weak-link-measurements.json`.

A reviewer read the design, `domain/link.rs`, `application/link.rs`,
`measure.rs`, the three test files, and a spot-check of the recorded JSON.
They did not re-run `cargo test`.

One minor finding: the order test did not stack sleep on a closed pace, or a
closed pace on an unknown quote, so swapping those checks would stay green.
`deferral_order_is_unreachable_then_reconnect_then_sleep_then_pace` now admits
a quote that fits, finishes one byte so the pace closes, and checks both
stacks. The design names that test for the order.

Revert probes, each restored after the named test failed: admitting a metered
unknown quote; dropping the pace wait's 1000 ms factor; ranking an open
transcript while the subscription is down.

`node scripts/check-architecture.mjs` passed. `cargo clippy -p nessa-client-core --all-targets -- -D warnings` passed.

Not claimed: the device session does not call `LinkSchedule`. Presence,
allowance and pace are phone facts this process does not have. That call is
[#1062](https://github.com/nessalabs/nessa-agent/issues/1062), not a second scheduler.
