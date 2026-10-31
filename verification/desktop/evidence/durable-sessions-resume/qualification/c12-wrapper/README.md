# C12 credentialed Claude wrapper qualification

Reviewed source: `6d1b413b19d2af4355fdf331778fe98fa4a683a4` (tree `edb5e9a2fba369503238859b54924bb6cdfc0437`). The isolated probe checkout was reset to that exact source after preserving its historical C11 patch in `prior-probe-work.patch`.

Only mutation: remove the nine-line `AgentProvider::restores_saved_context` override from `credentialed_claude.rs`. The verified hunk is `delegation-removal.patch`; no test or fixture edits were made.

Command, after sourcing `/workspace/.setup/activate.sh`:

```sh
CARGO_TARGET_DIR=/workspace/nessa-agent/target CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 /usr/local/bin/docker-init -s -- cargo test -p nessa-server --lib credentialed_claude::tests --offline -- --nocapture
```

Negative: compilation completed; 6 tests passed and exactly 2 failed, none ignored. `context_restoration_delegates_without_reading_credentials` returned true instead of false; `an_unused_credentialed_claude_context_reopens_without_native_history` attempted native resume and failed with `Transport("provider stdout closed")`. Full terminal results are in `negative.log`. The original process handle expired across context compaction, so its numeric exit code was not independently recovered; the complete cargo failure summary is retained.

Restoration: copied the saved pristine bytes back, producing a fresh mtime and matching SHA-256 `e103dab8d89653f8b1b583b6b189f91889ba831fd2932a9dcbd6db23135fdfb8`. Git diff and status were clean. The same command recompiled successfully, exited 0, and passed all 8 tests with none failed or ignored (`positive.log`). No root source files were changed. Shared target released to the coordinator.

Relationships checked: credential-free restoration classification delegates to validated Claude configuration; unavailable credentials are not queried by classification; admitted versus unused contexts have different restoration decisions; actual wrapper/Agent attachment survives unused native-history loss through RecordStorage reload. Source inspection confirms local composition supplies the same wrapper through InProcessEnvironment and LeaseFence to Agent::prepare, with the fence forwarding the restoration decision. C11 captures admission prefix before native opening; publication accepts the earlier conservative prefix without changing same-ID watermark preservation or clear-after-admission refusal.

Limits: the executable C12 probe uses the real wrapper, Agent, local Python ACP fixture, and physical RecordStorage, but does not instantiate the private LeaseFence constructor. No real credential/provider call was used. Full CI package selection, coverage, Clippy, browser/scripted E2E, other platforms, and final documentation commit qualification belong to the coordinator and are not reported passed here. Both test compiles emitted the existing lease-protocol long constant-evaluation warning. No new source finding in this focused follow-up; final combined PR review still awaits canonical gates.
