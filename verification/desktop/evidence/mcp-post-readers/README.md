# MCP POST reader verification (#622 / PR #635)

Final tested source: `ecc477df886d82e1456b80b0dba0ad8797c74cbc`, clean and frozen.

Command: `pnpm test:e2e:scripted -- --channel bundled --evidence /tmp/nessa-622-scripted-eof`, under a reaping init wrapper. Dev mode, columns layout, Chromium and WebKit. Final verdict: **pass** for all three checks and both engines. The result files record actual observations; raw service logs and screenshots are omitted because this Rust transport change does not change a desktop surface.

This evidence commit is separate from the code PR so publishing results does not alter the tested source head. See [final summary](final/pr-summary.md).

Intermittency: first run failed Chromium gateway-window on a resource abort; the subsequent three complete runs passed, including the final tested source (1 failure in 4 runs overall). The first result and failure are retained under [first-intermittent](first-intermittent/pr-summary.md). Cause is unproven. Earlier retry and allocation-only-head passes remain in the execution workspace; the PR records their disposition.
