# Issue 630 final acceptance evidence

Base: cffdabba64f821c012b2df04c2f02e5754511df3
Head: 937610ee85494b53e4591a3b3b2146118f864971
Checkout: /workspace/nessa-agent-container-reaping

Scope: opt-in pinned Docker/Python Linux PID-namespace acceptance harness and developer orchestration tests. Documentation covers supported reaping environments; existing scripts:test discovers pure tests. No production Rust, global subreaper, ESRCH shortcut, CI job, browser or provider installation change.

Checks: 27 bare-Node tests, 74 architecture tests, architecture checker, SDK documentation checker, Prettier, Python AST/tabnanny, and git diff --check passed. Fresh no-run Cargo JSON selected exactly one SDK library executable from this checkout under tini -s. No cleanup tests ran directly on the host.

Mutation evidence: /tmp/630-mutations.log contains 32 independent guard mutations that failed tests; source edits were asserted and restored with fresh mtimes. /tmp/630-supervisor-reaper-mutation.log proves a real-container broad-reaper mutation caused the negative to fail for missing adopted zombie; supervisor restored before acceptance.

Image: sha256:c30a858151fd1f110f5f1639372775425d3ed5602728f97bc288d5d1df273e4a
Image reference: nessa-process-cleanup:630
Pinned base: debian:trixie-slim@sha256:a29215f6a35e51e22adffa17f89e9d2ef06214e64a2bad10d765c46aea49f11f
Binary: /workspace/nessa-agent/target/debug/deps/nessa_sdk-3a9a37445fc631f7
Binary SHA256: 932566e1324fb80f9e30d0512e823d04d6c97b375af3a565306e39b88dd6c28d
Compiled manifest directory: /workspace/nessa-agent-container-reaping/crates/nessa-sdk

Runtime scenarios:
- infrastructure::process::tests::private_directory_is_removed_only_after_process_cleanup_is_confirmed; Docker init=False; accepted=True; exit=101; new_orphans=[{"pid": 10, "state": "Z", "ppid": 1, "pgid": 9}]; retained_dirs=["/tmp/nessa-agent-7f02ZS"]; packages={"ca-certificates": "20250419", "libgcc-s1": "14.2.0-19", "python3-minimal": "3.13.5-1"}
- infrastructure::process::tests::private_directory_is_removed_only_after_process_cleanup_is_confirmed; Docker init=True; accepted=True; exit=0; new_orphans=[]; retained_dirs=[]; packages={"ca-certificates": "20250419", "libgcc-s1": "14.2.0-19", "python3-minimal": "3.13.5-1"}
- infrastructure::acp::tests::contracts::shutdown::force_closes_a_term_resistant_parent_and_reaps_its_child; Docker init=False; accepted=True; exit=101; new_orphans=[{"pid": 10, "state": "Z", "ppid": 1, "pgid": 9}]; retained_dirs=[]; packages={"ca-certificates": "20250419", "libgcc-s1": "14.2.0-19", "python3-minimal": "3.13.5-1"}
- infrastructure::acp::tests::contracts::shutdown::force_closes_a_term_resistant_parent_and_reaps_its_child; Docker init=True; accepted=True; exit=0; new_orphans=[]; retained_dirs=[]; packages={"ca-certificates": "20250419", "libgcc-s1": "14.2.0-19", "python3-minimal": "3.13.5-1"}

All four disposable containers were removed. Read-only binary/probe/fixture mounts, container-local writable /tmp and no runtime network.

Selfreview inspected all changed files, exact test/fixture behavior, process-group proof versus directory retention, metadata/image/binary identity, timeout/interrupt/failure-to-removal ordering, preservation of rejected diagnostics, and module maps/links. No production scheduler, audit, persisted state or product UI behavior changed.

Limits: opt-in Docker/Linux acceptance; this does not prove recursive child-Agent closure, exercise every SDK test or replace platform CI. Explicit binary and compiled-manifest inputs must agree. Abrupt SIGKILL of the outer harness cannot execute JavaScript finally. Removal failures are explicit failures and retain diagnostics.

Publish separately: acceptance.json; selected-artifact.json; summary.md; mutations.log; supervisor-reaper-mutation.log; pure-tests.log; architecture-tests.log. Generated evidence is outside the source tree.
