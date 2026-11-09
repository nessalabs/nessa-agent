# Native lifecycle clock qualification

The [original Windows assertion failure](ci-491-windows-failure.txt) is retained.
The correction changes only the lifecycle test's existing elapsed-clock input and
wraps its three refreshes with the existing independent failing watchdog. It does
not change production source, budgets, completion assertions or retention limits.

Base: `9bbe126b86f6a4f87594dc5de529e998dd418841`.
Reviewed lifecycle file SHA-256:
`c32ff81109cd3d23169fc0f3c9de541ea72f419424a3656b3134da15c3b94d57`.

- `cargo test -p nessa-server --lib composition::read_only_online_tests::native_session -- --nocapture`:
  three passed, zero failed, one opt-in Swift probe ignored, 0.75 seconds.
  Includes complete lifecycle, explicit expiry after actual Head entry and refusal
  after confirmed partial history.
- Independent reviewer reran that selection using the exact compiled binary:
  three passed, zero failed, one opt-in Swift probe ignored, 1.64 seconds.
- Unchanged `default_budget_preserves_real_server_read_timeout`: one passed,
  10.45 seconds; real server read-timeout code and minimum elapsed assertion held.
- `cargo fmt --all -- --check`: passed.
- Exact CI seven-package all-target Clippy selection with `-D warnings`: passed,
  exit zero. Existing macro-generated lease-protocol warnings were emitted and
  preserved in the local log; no new warning suppression was added.

No Windows reproduction or corrected Windows pass is claimed by these Linux
checks. New-head automatic platform CI is required. The unrelated Swift opt-in
probe is not represented as executed. Native domain transitions and production
bytes are unchanged, so no state-diagram checkpoint advances for this test fix.
