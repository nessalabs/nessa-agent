# MCP HTTP progress verification (#623, #626, #634, #636, #631)

Final clean source: `441996826c1717a817f884b35ac3ab0d284e9a70`, base `cffdabba64f821c012b2df04c2f02e5754511df3`.

Command: `pnpm test:e2e:scripted -- --channel bundled --evidence /tmp/nessa-http-progress-scripted-local-deps`, under Tini subreaper with bundled Playwright engines. Dev mode, columns layout, Chromium and WebKit. [Final summary](final/pr-summary.md): all three checks passed in both engines; gateway-window 13/13 held, scripted-scenarios 8/8 held. The Rust transport change does not alter a desktop surface, so raw logs and screenshots are omitted.

The [first browser run](first-worktree-font-failure/pr-summary.md) failed console checks because Vite refused fonts served through this worktree's shared node_modules symlink (HTTP403). Installing this worktree's own dependency links with the unchanged frozen lockfile corrected the environment. One complete failure and one complete pass on the same source; two earlier pnpm invocations stopped before browser execution because a shared-directory reinstall required a terminal. This is not claimed as a product fix.

Full SDK: 1,640 passed, 19 existing ignored (including doc tests). Six CI-selected package MCP tests: 480 passed, 2 ignored. All-target Clippy with warnings denied, SDK docs, architecture checks/tests, fmt/diff passed. [Gate results](local-gates.json) retain command outcomes. [Six structural repair mutation probes](mutation-results.json) each produced actual expected regression failures and were restored before final checks; nine earlier probes are recorded on the PR.

Two fresh independent read-only reviewers inspected the full final diff and reported no findings at any priority. This separate evidence commit preserves the tested source head.
