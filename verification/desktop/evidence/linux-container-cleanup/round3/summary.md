# Issue 630 structural correction evidence

Base: cffdabba64f821c012b2df04c2f02e5754511df3
Head: d9ac477436605a5ac4b360d198aa7fedd830b1e0
Checkout: /workspace/nessa-agent-container-reaping

R2 dispositions:
- Publication cancellation: publishAcceptance is the single owner of accepted status; it checks abort before and after the awaited record write. Cancellation during successful independent removal or final write rejects the run, retains rejected diagnostics and leaves the fourth entry unaccepted. Record-write failure also retains unaccepted diagnostic evidence.
- Negative cause proof: selected exact test panic header followed immediately by Result::unwrap Err CleanupUncertain is required. Incidental tokens, another test panic and a different actual cause reject. Numeric thread IDs are optional for supported Rust libtest formats; CRLF is supported.
- Canonical ordering table was updated before code; its Mermaid sequence diagram illustrates actual runScenario/removal/publishAcceptance functions.

Checks at correction:37 restored bare-Node tests; architecture checker, Prettier, Python tabnanny, git diff --check passed. No broad historical gate repetition or Cargo rebuild. Prior74 architecture tests/SDKdocs retain earlier provenance.

New probes:7 targeted mutations each caused test failure; edits asserted and source restored with fresh mtime. Source content matches the committed check-container.mjs at d9ac4774; only README outcome/provenance updates followed probes. Historical32 and R1 22 probes remain separately labeled937610ee and93647941.

Runtime: current correction harness exited0 and all4 entries accepted; all containers removed. Explicit preserved binary /tmp/nessa-630-main-base-library-test is the main-base Cargo artifact copy; SHA256=932566e1324fb80f9e30d0512e823d04d6c97b375af3a565306e39b88dd6c28d
Image=sha256:c30a858151fd1f110f5f1639372775425d3ed5602728f97bc288d5d1df273e4a
Compiled manifest=/workspace/nessa-agent-container-reaping/crates/nessa-sdk

Scenarios:
- infrastructure::process::tests::private_directory_is_removed_only_after_process_cleanup_is_confirmed; init=False; accepted=True; exit=101; orphans=[{"pid": 10, "state": "Z", "ppid": 1, "pgid": 9}]; retained=["/tmp/nessa-agent-00dvoJ"]; packages={"ca-certificates": "20250419", "libgcc-s1": "14.2.0-19", "python3-minimal": "3.13.5-1"}
- infrastructure::process::tests::private_directory_is_removed_only_after_process_cleanup_is_confirmed; init=True; accepted=True; exit=0; orphans=[]; retained=[]; packages={"ca-certificates": "20250419", "libgcc-s1": "14.2.0-19", "python3-minimal": "3.13.5-1"}
- infrastructure::acp::tests::contracts::shutdown::force_closes_a_term_resistant_parent_and_reaps_its_child; init=False; accepted=True; exit=101; orphans=[{"pid": 10, "state": "Z", "ppid": 1, "pgid": 9}]; retained=[]; packages={"ca-certificates": "20250419", "libgcc-s1": "14.2.0-19", "python3-minimal": "3.13.5-1"}
- infrastructure::acp::tests::contracts::shutdown::force_closes_a_term_resistant_parent_and_reaps_its_child; init=True; accepted=True; exit=0; orphans=[]; retained=[]; packages={"ca-certificates": "20250419", "libgcc-s1": "14.2.0-19", "python3-minimal": "3.13.5-1"}

Selfreview traced captured proof and typed schema, selected panic identity/cause, independent rm through abort, accepted/rejected durable-record mapping, and CLI rejection/success. Overall acceptance requires successful harness exit plus four confirmed entries; accepted-looking partial/cancelled output alone is insufficient.

No production Rust/subreaper/ESRCH shortcut/CI job/provider installation change. Source clean and frozen. Coder/build slot released.

Publish separately: /tmp/630-r2-summary.md; /tmp/nessa-630-r2-evidence/acceptance.json; /tmp/630-r2-mutations.log; /tmp/630-r2-pure-tests.log. Original selected Cargo artifact remains /tmp/630-final-selected-artifact.json; generated evidence stays outside source.
