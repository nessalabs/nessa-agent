# Issue 630 review correction evidence

Base: cffdabba64f821c012b2df04c2f02e5754511df3
Head: 9364794177684a846b9071f268b5b870b198036a
Checkout: /workspace/nessa-agent-container-reaping

Review dispositions:
- Major missing-proof false positives: required booleans, positive process identities, typed process/directory arrays and concrete package/executable metadata validated before acceptance; initial, supervisor, test and adopted identities correlated. Missing/nonnumeric PID, absent flags and identityless zombie/directory examples reject.
- Minor image-inspection interruption: the existing abort signal is passed to image inspection; mocked inspection-stage interrupt rejects before any container is created and restores handlers.

Current checks:32 bare-Node orchestration tests,74 architecture tests, architecture checker, Prettier, Python tabnanny and git diff --check passed. SDK documentation checker passed at pre-review head; no SDK Rust/public documentation change in this correction. No Cargo rebuild was needed: binary source and compiled-manifest inputs are unchanged.

New correction probes:22 independent schema/cancellation mutations failed tests; edits were asserted and restored with fresh mtimes. Outcomes /tmp/630-r1-mutations.log; individual failures /tmp/630-r1-mutation-*.log. Historical32 probes and real broad-reaper mutation are from937610ee, described separately in /tmp/630-final-summary.md.

Image:sha256:c30a858151fd1f110f5f1639372775425d3ed5602728f97bc288d5d1df273e4a
Binary SHA256:932566e1324fb80f9e30d0512e823d04d6c97b375af3a565306e39b88dd6c28d
Compiled manifest:/workspace/nessa-agent-container-reaping/crates/nessa-sdk

Fresh current-correction runtime evidence:
- infrastructure::process::tests::private_directory_is_removed_only_after_process_cleanup_is_confirmed; init=False; accepted=True; exit=101; new_orphans=[{"pid": 10, "state": "Z", "ppid": 1, "pgid": 9}]; retained_dirs=["/tmp/nessa-agent-EbolMH"]; packages={"ca-certificates": "20250419", "libgcc-s1": "14.2.0-19", "python3-minimal": "3.13.5-1"}
- infrastructure::process::tests::private_directory_is_removed_only_after_process_cleanup_is_confirmed; init=True; accepted=True; exit=0; new_orphans=[]; retained_dirs=[]; packages={"ca-certificates": "20250419", "libgcc-s1": "14.2.0-19", "python3-minimal": "3.13.5-1"}
- infrastructure::acp::tests::contracts::shutdown::force_closes_a_term_resistant_parent_and_reaps_its_child; init=False; accepted=True; exit=101; new_orphans=[{"pid": 10, "state": "Z", "ppid": 1, "pgid": 9}]; retained_dirs=[]; packages={"ca-certificates": "20250419", "libgcc-s1": "14.2.0-19", "python3-minimal": "3.13.5-1"}
- infrastructure::acp::tests::contracts::shutdown::force_closes_a_term_resistant_parent_and_reaps_its_child; init=True; accepted=True; exit=0; new_orphans=[]; retained_dirs=[]; packages={"ca-certificates": "20250419", "libgcc-s1": "14.2.0-19", "python3-minimal": "3.13.5-1"}

All four cases accepted and all containers removed. Source checkout clean; coder/build slot released for readonly review. No production Rust, global subreaper, ESRCH shortcut, CI job or provider installation change.

Publish separately: /tmp/nessa-630-r1-evidence/acceptance.json; /tmp/630-r1-summary.md; /tmp/630-r1-mutations.log; /tmp/630-r1-pure-tests.log; /tmp/630-r1-arch-tests.log. Selected Cargo artifact remains /tmp/630-final-selected-artifact.json from the main-base build. Evidence stays outside source.
