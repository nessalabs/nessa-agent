# Desktop host ownership: cloud qualification

Issue #943. Base `839b1d50352c833c888254130456ac1355cccfa6`.
[Ordering contract](../../../design/desktop-host-ownership.md).
`source-sha256.json` identifies the source, including the new Mac-only label test.
No installed service, Login Items, bundle, credential or conversation was changed.

## Executed locally

Linux x86_64, Rust 1.99.1, private workspace toolchain and isolated Cargo target.
The headless harness included the production gateway application/domain sources
and the native Unix ownership module by path. It used Tauri 2.11.5's unchanged
`async_runtime.rs` with a minimal JoinError/Result crate shell; source/archive
hashes are in `qualification.json`. This is not a build of the complete Tauri
host, native macOS adapter, or package-selection dependency graph.

- Full source selection: **157 passed, zero failures/ignored**. See
  `restored-full.log`. Focused new selection: **17 passed** (including the child
  process entry, which is inert outside its explicit subprocess environment).
- Headless test Clippy: pass with `-D warnings`; `headless-clippy.log`.
- Standard desktop script: **304 pass, three existing skips**, format/platform
  source checks pass; native host compilation/tests explicitly skipped because
  WebKitGTK is unavailable. See `desktop-check.log`.
- Repository architecture and diff whitespace checks pass.
- Nine production-rule omissions each compiled and failed their intended runtime
  assertions; `results.json`, individual logs and mutation hunks retain both
  outcomes. Exact original source was restored after each probe and the full
  positive selection passed. No other source was tested while a probe was active.

The first subprocess test run failed because its exact-test selector included
the crate prefix from `module_path!`; the Rust test runner's names omit that
prefix. `original-child-filter-failure.log` is retained. The fixture now selects
the actual module name, uses bounded readiness and cleans up its child. The
final process-death/fork/exec assertions pass; no production behavior was changed
for that fixture correction.

## Self review

The lease is acquired before journal access/recovery and durable Claude settings
publication. A refusal publishes the unopened journal's failure to joined callers.
The host owner retains its lease across failed registration; reconciliation and
stop workers retain ownership after caller/foreground loss. Stop borrows the
already held lease after its existing bounded readiness wait, so it adds no new
five-second acquisition wait to the quit deadline. Native file opening reuses
private-storage validation; the separate ownership lock file is not unlinked.
Service-label construction remains the existing native owner and ignores bundle
and data-root differences. Existing retirement identities, plans, audit ordering
and generation decisions are unchanged.

## Pending acceptance

Full `nessa-app` Clippy/tests and the repository's exact package/platform CI are
required. The Mac-only test constructs real Launchd adapter instances with fake
manager effects and private directories; it is not an installed two-bundle run.
Actual native competing-bundle registration, updater/restart, signed launch,
Login Items and startup timing remain coordinated Mac work. #562 and #109 stay
open. Older binaries that do not implement the cooperative lease can still
mutate the service; this change does not claim to exclude them retroactively.

The code changes desktop host reconciliation, not the gateway wire/ACP/MCP or
rendered UI. `pnpm test:e2e:scripted` was attempted but stopped before browser execution:
the package-manager prerequisite could not create its default pnpm data directory.
See `scripted-e2e-prerequisite-failure.log`; the browser result is **unverified**,
not a pass. It cannot replace the pending native acceptance.
