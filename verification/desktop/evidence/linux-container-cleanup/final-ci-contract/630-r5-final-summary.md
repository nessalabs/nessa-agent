R5 frozen candidate: 95e445ac70cc407dacdf72178ee073dfc12394a0
Base: cffdabba64f821c012b2df04c2f02e5754511df3
Assertion-only correction since e0fcd75f40d7db82b7ad4df8a418b0b4da8f9908. No production, CI job, ownership or prerequisite changes. Local commit only, not pushed. Clean worktree and released coding slot.

Whole desktop/config Node suite: 255 tests, 252 passed, 3 existing platform skips, 0 failures (/tmp/630-r5-desktop-tests.log). Cleanup suite: 42 passed (/tmp/630-r5-cleanup-tests.log). Ownership mutation removing actual package scripts:test cleanup glob: targeted existing test failed with exit1 (/tmp/630-r5-ownership-mutation.log); package.json restored byte-identically and with fresh mtime; restored target passed (/tmp/630-r5-ownership-restored.log). Prettier affected assertion PASS; architecture PASS (/tmp/630-r5-architecture.log); git diff --check PASS.

Initial desktop/architecture invocation lacked configured Cargo PATH, so metadata lookup failed ENOENT. Final runs used configured CARGO_HOME/RUSTUP_HOME/PATH and passed; no compilation or runtime test repeated. Four-container runtime proof and UTF8/audit probes remain explicitly attributed to e0fcd75f; /tmp/630-631-final-summary.md and /tmp/nessa-630-r3-evidence/acceptance.json unchanged.

Full base/head file list:
README.md
crates/nessa-sdk/README.md
crates/nessa-server/tests/composition/mcp_servers.rs
docs/adr/todo/329-subagents.md
docs/adr/todo/392-remote-mcp-servers.md
docs/codebase-structure.md
docs/state/services/sdk/runtime/stop-cancels-owned-work-and-confirms-process-cleanup.md
package.json
scripts/desktop/ci-harness.test.mjs
scripts/process-cleanup/Dockerfile
scripts/process-cleanup/README.md
scripts/process-cleanup/check-container.mjs
scripts/process-cleanup/check-container.test.mjs
scripts/process-cleanup/container-init.py
