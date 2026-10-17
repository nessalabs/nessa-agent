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

R2 will review the complete corrected source; R1 is not relabeled clean. Native
compilation/package CI, browser E2E and installed-Mac acceptance remain separate.
