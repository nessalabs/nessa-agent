# OpenCode hosted configuration — scoped closing review

**Result: no actionable findings at any priority in this integration repair.** This is a source review of the repaired OpenCode host projection and adjacent launch/cleanup contracts, not a whole-PR, provider-runtime or shipping approval.

## Exact snapshot

Checkout `/Users/nessa/.codex/worktrees/github-webhooks-1107/nessa-agent`.
The initial resolved merge was HEAD `31237ca42b287b3d87d6fc4516531d73377b2526`, MERGE_HEAD `41339b0ba52dbddb2441389c9ebce1e4f7426c9e`, with 188 staged main files and the unstaged OpenCode repair plus ADR1152 invariant rows. During this read-only review the coordinator committed the unchanged source as **`a79f324f39c9cd58c2082206b531b438d794d438`**. The final working tree is clean. No source, cache, build, credential or cloud resource was changed by this reviewer.

Reviewed ten-file path/NUL/content/NUL SHA-256: `bb5ef7edc46af8292877712e22ade28625a8a6a215097f90fb10626147fc69a2`:

- SDK OpenCode `sessions/binding.rs`; shared ACP `sessions/config.rs`.
- SDK provider `harness_host.rs`; `harness_process.rs`; `process.rs`.
- Server `env_serve/infrastructure/launcher.rs`; `workspaces/infrastructure/ssh.rs`.
- SDK ACP contract `harness_host.rs`; `stand_ins.rs`.
- ADR `docs/adr/todo/1152-local-vault.md`.

Also inspected Claude/Codex host call sites, SSH LeaseHost/HostLink relay access, ACP session/grant ownership, and OpenCode fixture isolation expectations. Applied AGENTS, canonical review/brief/agreement/evidence rules and coding/architecture/PR skill instructions.

## Relationships checked

1. **Selected host -> workspace -> scoped relay.** OpenCode now asks the same `AcpConfig::on_host(host.workspace(), host.mcp_relay())` owner as Claude/Codex. The projection uses the selected host workspace and unresolved path resolver, so a path existing on this machine is not interpreted as the remote host's readable path. Retained `host` selects `ProcessScope::remote(host.start(...))`; there is no local-spawn fallback after remote refusal.
2. **Local credentials versus explicit per-open MCP grants.** The shared projection clears environment, runtime environment, credential environment and open credential source. A later OpenCode `with_open_credentials` therefore cannot reread the original local credential source. Absent relay yields no MCP commands or stand-ins. Present relay reconstructs only mapped entries and preserves scoped grant ownership. SSH mapping requires the configured executable, relay subcommand, socket and exact argument shape; returned command uses the remote relay executable/socket. This grants transport to the existing stand-in authority, not blanket permission for an MCP tool.
3. **Session grant lifetime versus remote process credentials.** `StandInSessions::opened` mints a grant for the actual session and keeps it held for that provider opening. MCP entries receive that scoped environment; OpenCode process launch itself is `HarnessLaunch::default()`. Its empty `LAUNCH_VARIABLES` allowlist rejects gateway-supplied launch variables at the host. Host executable, account credentials and optional database remain the host's `LaunchSpec` authority, separate from gateway MCP grant origin.
4. **Remote launch -> private home -> confirmed cleanup.** Host OpenCode starts with `start_on_host`, reuses the fixed private-home/project-config/plugin policy and preserves only the host-selected OPENCODE_DB across isolation. `SupervisedHarness::start_private` retains directory ownership through `ProcessScope`, including failed private starts. The repaired gateway call adds no competing cleanup owner and does not change uncertainty into confirmed deletion. The existing fixture checks host launch/model/echo/cleanup, empty launch variables and refusal of external home/data/db values.
5. **Source protocol versus published artifact.** Independently computed the owner macro's 43 ordered normalized inputs using its exact length-prefix/FNV algorithm: **`b6e4f48aa694ce08`**, agreeing with the supplied final manifest. OpenCode binding is a fingerprint input, so the repair changes the prior pre-fix value. The old `066217f7701b98b9` release mapping remains incompatible; its rejection must remain until the coordinator publishes/verifies and pins the matching worker.

## Evidence and limits

Inspected actual compiler RED `/tmp/nessa-1158-phone-main-clippy.log`: E0061 at the former one-argument OpenCode `on_host` call. Inspected GREEN `/tmp/nessa-1158-phone-main-clippy-fixed.log`: eight-package strict all-target Clippy completed in 2m26s, no compile errors. Its existing long-running const-evaluation diagnostic is recorded, not hidden. Independently ran `git diff --check` (passed), read source/contract assertions and recomputed the protocol fingerprint. No mutation/revert probe was performed because the coordinator owns frozen-source native verification; the actual broken compile is the supplied RED.

Native hosted binding, stand-in, input preparation, GitHub and vault tests were still compiling/running under the coordinator when this report was written. I did not run Cargo, start another build, use the coordinator's session handles or claim those tests passed. The existing tests separately cover hosted OpenCode without a relay and shared explicit-relay projection; this review does not claim a dedicated live hosted-OpenCode MCP end-to-end run. Matching worker construction/clone/runtime verification, provider subscription inference, browser checks and fresh hosted CI remain coordinator-owned.

P3 audit retention-cap issue #1185 and baseline UI issues #1162/#1173/#1178/#1179 remain separate dispositions. This review neither fixes nor suppresses them. No blanket security, external pentest, physical erasure, deployment, Cloudflare plan or full-PR approval is asserted.
