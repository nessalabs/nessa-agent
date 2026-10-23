# Named persistence owners across the scoped PR

Source `b3dc0d4688e473fa40bbbeddf00bb9f510130bf1` completes the organization
correction before fresh round two. The [first review](../review-1/report.md)
focused on the new reset module. Self-review of the whole PR against its
foundation base also found parent outbox behavior in `mod.rs`, added earlier
in the same PR. This is related feature organization, not an unrelated repair.

Both `mod.rs` files now contain declarations and re-exports. The private database
lock, bounded operations and their tests live in `outbox/store.rs`; canonical SQL
lives in `outbox/schema.rs`; primitive/current encoding checks remain with `codec`.
Reset fact/capture persistence lives in `source_reset/store.rs`. The existing
registration adapter retains access within the same outbox consistency boundary.
No permission, schema content, lifecycle, timeout or test assertion changes.
The test module name gains `store`; the test files and behavior remain the same.
Module map and state source list follow these owners.

## Exact-source checks

Linux cloud workspace, Rust 1.99.1. Required seven-package all-target Clippy
`-D warnings` passed in 68 seconds ([log](clippy.txt)); the existing lease-protocol
const-evaluation notice remains. Canonical webhook tests passed **97 tests**,
zero failures, 3749 filtered, 49.60 seconds ([log](canonical.txt)). The isolated
suite passed **67 tests**, zero failures, 55.39 seconds ([log](isolated.txt)).
All 17 focused reset tests passed. Architecture **108 tests**, checker and
formatting passed ([tests](architecture-tests.txt), [checker](architecture.txt)).

All **27 fail/restore probes** failed their intended regression and passed after
byte-identical source restoration ([run](mutation-run.txt), [rules](mutations.json),
[script](mutations.py)). The [source manifest](source-manifest.json) ties every
copied source file to this committed checkpoint. No assertion was weakened.

NessaDocs passed **122 pages, zero issues** ([catalog](docs-validation.json)).
Both charts rendered in Chromium 151 with zero page errors at the new source
checkpoint ([render](render.json)). The reset chart was visually inspected; the
operation chart PNG is byte-identical to the previously inspected render.
This is documentation evidence, not canonical gateway browser qualification.

The [intermediate focused checks](../organization/README.md) qualify only source
`6e624b49`. Fresh independent round two, canonical gateway Chromium/WebKit,
stress/chaos, exact-head CI and full runtime composition remain pending.
Committed-range diff checks against both the correction and intended PR base
will be recorded after the evidence commit. Working-tree checks alone do not
establish those ranges.
