# Pending browser acceptance

Use only a separately approved executor with the repository's pinned Playwright
browsers and runtime dependencies already available. This is a command brief,
not a task created on a person's local machine. Do not retry the denied browser
download from this cloud executor or replace canonical engines with a fallback.

Source code commit: `4a3120e2d3eed25de87c14a2c2ab49b64ad4cb56`.
Integrated main: `6a278df920562700a25df832d0361ed7c036b20f`.
Use the final published descendant containing this evidence when available;
record its full SHA, Git tree and clean status before execution. The missing
`768f7da7` commit and old PR head `46762c7d` are not this source.

Prepare Node 24, pnpm 11.9.0, stable Rust, the pinned UI checkout and locked
package dependencies through the repository's existing setup. Verify
Playwright 1.63.0 and the package-selected Chromium/WebKit revisions; inspect
its browser manifest for the target platform's WebKit override. Keep the
configured browser installation path. Use a fresh evidence directory per run.

Run the canonical signed-out suite, including its dev-only MCP check:

```sh
pnpm test:e2e:scripted -- --channel bundled --evidence /tmp/965-canonical-1
```

The runner selects both engines. Repeat with distinct `-2` and `-3` evidence
directories to quantify the previously intermittent MCP context failure. Preserve
all failures, console/request lines, verdicts and `pr-summary.md`; passes alone
do not establish the original cause. Production mode skips the MCP check and
cannot replace this run.

Run default and raised conversation stress in both engines:

```sh
pnpm test:e2e:stress -- --engine chromium,webkit --channel bundled --evidence /tmp/965-stress-default
pnpm test:e2e:stress -- --engine chromium,webkit --channel bundled --seed 7 --rounds 40 --parallel 4 --turns 5 --asides 4 --switches 5 --stops 3 --fails 2 --reviews 24 --restarts 2 --pairedCatalogues 4 --evidence /tmp/965-stress-raised
```

The new native step compiles before the window opens, then runs its exact test
with one/four cycles alongside background work. Keep its captured output,
nonempty test count and exit status. Retain the existing final at-rest checks,
follower replay comparisons, reader-abort/EOF/hash checks, deadlines and failure
annotations. A missing browser or failed row remains a failed/unavailable gate.

The existing manual `canonical-browser-verification.yml` workflow can run the
canonical signed-out suite using `source=<full published SHA>` and empty `base`
when that source already contains the integrated main. Its provenance artifact
must match the requested source/tree. That workflow does not run the default
and raised stress commands above. Dispatch access is presently unavailable.

#746 additionally requires real macOS qualification and diagnosis. Its exact
existing convergence test is:

```sh
cargo test --locked -p nessa-server --lib composition::read_only_online_tests::online_two_devices_follow_live_views_and_converge -- --exact --nocapture
```

A passing Linux run is not macOS acceptance; historical ten-run macOS success did
not reproduce preparation. Keep the product's four-attempt bound and existing
assertions unchanged while investigating the original failure.
