# Local review record

Base: `839b1d50352c833c888254130456ac1355cccfa6`.

## R1: two findings, both corrected

Fresh independent FULL review inspected all changed source, tests and docs,
H1–H13, ownership through failures/deferred work/quit, journal publication,
identity selection, fork/exec and retirement/audit agreement. It independently
matched all eight source hashes and passed focused16/full156 headless tests.

- P2: the first acquisition `clock.now()` was before the local panic boundary.
  The outer attempt supervisor could settle the operation while a joined journal
  waiter stayed Pending. Added H14 and a gated actual application regression.
  `clock-panic-before.log` shows the original typed waiter timeout; deadline
  creation is now inside supervision. `initial-clock-supervision.log` restores
  that omission on corrected source and fails the regression; exact restoration
  passes in the full157 selection.
- Minor: lifetime and transaction locks independently constructed the same
  service-label directory. Both now call one `service_lock_directory` helper.
  The Mac-only adapter test covers same label/different data roots and distinct
  stage/instance claims; actual hosted Mac execution remains required.

## R2: source review clean; evidence whitespace corrected

Fresh independent FULL review at `6dae6f6be1579ead639fcd47ec8d1da0b3f43a87`
found no blocker, major or lifecycle source defect. It independently passed
157 tests and matched all eight source hashes, inspecting H1–H14, pre-journal
and settings boundaries, failed/cancelled callers, deferred staging, stop-worker
retention, lock identity, process release and existing retirement/audit agreement.

Its sole minor finding was two mutation-description files ending with blank
lines. The initial working-tree-only whitespace command had not inspected new
untracked evidence. Those two descriptions are trimmed and the complete
`git diff --check <base>` now passes. This follow-up changes evidence only;
production/test source and its qualification hashes are unchanged.

R1 is not relabeled clean. Full native/platform compilation, Mac-only adapter
test, browser E2E and installed-Mac acceptance remain separate gates.

## Fresh FULL R3 and current-main integration

A fresh independent FULL review at `5ac5fe6bfae58b26ab8ff98b74eb579473ba2fbd`
found no new actionable code defects. It independently passed all157 tests and
strict headless Clippy, matched all eight source hashes and checked the complete
production/test diff, H1–H14, ownership across failures/drop/deferred work/quit,
journal/settings ordering, lock identity, process release, platform gating and
retirement/audit agreement. No source changed in response.

It explicitly found the legacy transition unresolved: holding the legacy
transaction lock or observing a process snapshot cannot fence all legacy effects
or later launches. The design now documents that original-incident gap, rejected
mechanisms and the minimum coordinated-upgrade release decision. Native two-host,
updater, installed launch, timing and WebKit remain separate unrun/blocked gates.

The branch rebased cleanly onto main `cf551b6a922b9b95b7f5f98b8d9720d7f3138c80`
after #957. #957 changes SDK/server permission behavior and browser verification;
only the codebase map overlaps and merged cleanly. Desktop production/test sources
and all eight qualification hashes remain unchanged. Exact-head platform and
assembled browser checks still need their current results recorded on the PR.

## Fresh full review of the umask fixture correction

Fresh independent FULL review against base `cf551b6a`, at diagnostic head
`2dff21e4` plus the disclosed fixture/docs/scaffold-removal changes, found no
actionable defect. It inspected the combined production lease/application/native
diff, H1–H14, journal/settings/quit retention, service-label identity, fork/exec,
platform gating and legacy limits. It verified seven unchanged qualification
hashes, the intended fixture-only change, exact original workflow restoration,
scaffold deletion, and all six13/158-test umask runs. Qualified full-test Clippy
passes; the broader partial-harness baseline-lint failure is retained explicitly.
Final native CI and actual installed-Mac acceptance remain separate gates.
