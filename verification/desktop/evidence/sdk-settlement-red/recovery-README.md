# Additional test-only recovery red checkpoint

Checkout/branch and command environment are the same as README.md. Production base remains 3e65d96e44e27c502a6225ec16e554fe18c4a152. Input test head is recorded in recovery-source-head.txt; additional checkpoint in recovery-red-head.txt. Only the public test file changed. No production/docs edits, push or PR.

Using the exact Tini/environment prefix in README.md:

```sh
cargo fmt --all
cargo test -p nessa-sdk --test application never_bound_root_terminal_store_rejection_recovers_after_resume -- --nocapture
cargo test -p nessa-sdk --test application r3_resume_of_a_closing_tree_does_not_dispatch -- --nocapture
cargo test -p nessa-sdk --test application previously_bound_root_cannot_use_never_bound_settlement -- --nocapture
```

Actual red: recovery-red.log, exit 101, running 1 test, assertion at line 1433. Recovery explicit close returns Err(Incomplete), required Ok(()). No compilation, fixture or selection correction was required for this additional test. Controls each run 1 test and pass (recovery-control.log, recovery-bound-control.log). Formatting and git diff --check pass. Prior invalid zero-selection log and two red/control logs remain preserved unchanged.

The wrapper delegates to a real MemoryOwnershipStore, allowing all writes except exactly one snapshot containing the selected original root at Closed. Selection is taken under mutex before awaiting; rejection is counted and does not mutate the store. The root is opened through the real public coordinator and never bound or handed to a factory. Close audit uses accepting ScriptAudit. Initial close honestly returns Store(Rejected), rejected terminal write count=1, memory root=Closed and actual retained root=Closing all pass before the decisive red assertion. Real factory prepare/submit counts remain zero and no physical resource is fabricated. The original coordinator is dropped, and a new instance resumes the same store and explicitly closes under independent timeouts.

The desired recovery requires the implementation to persist the actual original absence fact in a Closing snapshot before the fallible terminal Closed publication. The test does not authorize inferring absence from a restored empty resource map, a restored Closing/Closed label, or an authored snapshot. It deliberately does not assert the old coarse settlement layout, so future typed proof fields can supply the durable authority. Current proofless restore remains conservatively Incomplete and is the observed defect of the missing earlier proof persistence, not an assertion that restored unknown owners may be ignored.

Session reopen/new root ID and zero physical/factory replay after recovery are follow-on assertions, unexecuted while red. This establishes the requested future #646/#649 outcome; it makes no claim of a current #628 guarantee or baseline regression. Independent review, full lint/gates, typed durable proof/restoration and production repair remain deferred until landed-base/implementation sequencing.
