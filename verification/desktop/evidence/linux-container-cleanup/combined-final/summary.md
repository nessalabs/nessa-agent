Frozen combined #630/#631 candidate

Base: cffdabba64f821c012b2df04c2f02e5754511df3
Head: e0fcd75f40d7db82b7ad4df8a418b0b4da8f9908
Working tree clean; coding/build slot released. No production Rust or transport change. #631 extracts only the reviewed MCP composition audit test correction from 00ebdc4302adf9103a5ac4f3bcee3e71fbacd713, plus missing/ambiguous evidence regressions and its scoped ADR392 authority.

Final checks: 42 process-cleanup Node tests PASS; 27 focused server MCP composition tests PASS under tini; CI's six-package Clippy --all-targets -D warnings and fmt PASS; architecture and runtime dependency default/all-feature verification PASS; Prettier for affected JS/package JSON PASS; Python tabnanny PASS; git diff --check PASS. No full SDK run needed for tooling/docs and test-only changes.

Actual runtime: /tmp/nessa-630-r3-evidence/acceptance.json, four entries accepted and CLI exit 0; disposable containers removed. Negative cases each exited 101 with the selected test's CleanupUncertain panic and new PPID1 zombie; directory negative retained its private directory. Docker-init cases each exited0 with no orphan zombies or retained directories. Binary explicitly /tmp/nessa-630-main-base-library-test, SHA256 932566e1324fb80f9e30d0512e823d04d6c97b375af3a565306e39b88dd6c28d; original Cargo JSON selection /tmp/630-final-selected-artifact.json and /tmp/630-final-sdk-artifacts.jsonl. Embedded manifest /workspace/nessa-agent-container-reaping/crates/nessa-sdk mounted read-only. Image sha256:c30a858151fd1f110f5f1639372775425d3ed5602728f97bc288d5d1df273e4a, pinned Debian base a29215f6a35e51e22adffa17f89e9d2ef06214e64a2bad10d765c46aea49f11f; python3-minimal3.13.5-1, libgcc-s1 14.2.0-19, ca-certificates20250419.

New actual failure probes: six UTF-8 retention/decoder/overflow mutations (/tmp/630-r3-mutations.log) and three audit mutations: legacy timestamp-position selector (/tmp/631-timestamp-mutation.log), omitted unique-match assertion (/tmp/631-uniqueness-mutation.log), removed action/phase predicate (/tmp/631-selector-mutation.log). All caused actual test failures and were restored with fresh modification times; resulting source content is exactly the frozen head. A discarded invalid PATH predicate attempt is excluded. Historical probes remain attributed to 937610ee (32), 93647941 (22), and d9ac4774 (7), not rerun or claimed against this head.

Restored logs: /tmp/630-r3-pure-tests.log; /tmp/631-final-composition-tests.log; /tmp/631-final-ci-clippy.log. Historical summaries: /tmp/630-final-summary.md, /tmp/630-r1-summary.md, /tmp/630-r2-summary.md. Generated evidence stays outside source; root publishes separately.

Frozen source SHA256 (probe restoration provenance):
20cfd8563dddc93658392cfabcc0639b4914f625d2f0337570912c624585782a  scripts/process-cleanup/check-container.mjs
bdb64183bcd9807b6b25a23999c41f6f9fdcba4441e97e53ae54493d5e064a84  scripts/process-cleanup/check-container.test.mjs
2ab9cd85c206d212974a069f0b74e5bb5eba07d40962b7a117cf5b9b550323cf  crates/nessa-server/tests/composition/mcp_servers.rs
