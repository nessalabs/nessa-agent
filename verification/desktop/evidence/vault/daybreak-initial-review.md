# Fresh Daybreak security review — PR #1158

Review date: 2026-10-27  
Reviewer: independent defensive-security subagent  
Disposition: **not ready to merge; one P1 and two P2 findings require closure or an explicit standards-compliant disposition**

## Frozen scope

- Feature checkout: `/Users/nessa/.codex/worktrees/github-webhooks-1107/nessa-agent`
- Frozen feature HEAD: `079798f614e59f28848a27af8121befef5ee0349`
- Current main parent: `cfd025c121e1b32c42af7b4b062d64915a481c18`
- Synthetic merge inspected without checkout/reset: `cf9b388247f22867c7ff48034e257d239c6dedef`
- Synthetic merge parents were verified as exactly main then feature.
- `git status --short` was empty before and after the review.

This was a fresh source review. Prior review notes were treated as scoped evidence, not authority. I read the repository instructions and applicable coding/review/architecture/DI rules, then reconstructed the receiver, cloud read authorization, native catch-up/follower/store, vault, credential migration, Boat workspace, SSH/MCP relay, and synthetic-main integration paths.

## Findings

### P1 — The Boat environment is neither completely admitted nor revalidated at allocation

**Owners and exact locations**

- `crates/nessa-server/src/workspaces/infrastructure/boat.rs:41-80`
- `crates/nessa-server/src/workspaces/infrastructure/boat.rs:84-105`
- `crates/nessa-server/src/workspaces/infrastructure/boat.rs:123-144`
- `crates/nessa-server/src/workspaces/application/mod.rs:111-114`
- `crates/nessa-server/src/workspaces/application/mod.rs:145-168`

**Observed behavior**

For a newly created environment, Nessa asks Boat for this posture:

- `safe_for_third_parties = false`
- `pass_agents_credentials = true`
- `pass_github = false`
- `pass_secrets = false`
- `pass_sandbox_credentials = false`

The fetched environment is admitted by checking only the first two fields (`boat.rs:78-79`). An already-existing `nessa` environment with any of the other three credential pass-through flags enabled is accepted. The provider model also exposes `env_contents`, `secret_files`, `selected_repositories` (including setup behavior), `latest_version_id`, and flattened provider fields, but Nessa has no explicit admission policy for those fields.

The lifecycle widens the problem. `Workspaces::save` is the only call to `Provider::validate`. Later allocation reads the saved connection and calls `create` directly; reconnect calls `connect` directly and can resume an archived VM. Create selects the environment by mutable name only. The vendored `CreateSandboxRequest` has no environment-version selector, although fetched environments expose `latest_version_id`, and a concrete sandbox receipt exposes its environment name/version. Thus even a complete save-time check would retain a check/use gap after an environment changes.

**Concrete hostile sequence / reproduction**

1. In the same Boat account, an existing or later-mutated environment named `nessa` has `safe_for_third_parties=false` and `pass_agents_credentials=true`, but also `pass_github=true`, `pass_secrets=true`, or `pass_sandbox_credentials=true`. Adjacent variants add environment contents, secret files, selected repositories/setup behavior, or provider credential-profile fields.
2. Save the Boat token. The current check accepts the existing environment because it tests only the first two booleans. Alternatively, save a currently clean environment and mutate it afterward.
3. Open a new Boat-backed conversation. `create()` sends only the mutable environment name, without revalidation or version pinning.
4. Boat can inject credential classes or environment/setup behavior outside Nessa's intended hosted-agent posture. Because `pass_agents_credentials=true` is intentionally enabled, hostile environment behavior executes in a credential-bearing VM and can also reach the per-open MCP relay offered to the launched agent.

Precondition: an existing/stale environment or an actor able to change the user's Boat environment. This does not require changing Nessa's local files or defeating its local environment-clearing code.

**Authoritative invariant**

The hard-coded Nessa Boat environment must have one explicit, complete, currently verified posture at the allocation boundary. Only the credential class intentionally required for hosted agents may be passed. A mutable provider environment cannot silently add GitHub, secret, sandbox, setup, repository, or unknown credential behavior between settings validation and VM creation/resume.

**Minimal owning fix**

Create one authoritative full-posture predicate, including all five booleans and an explicit decision for environment contents, secret files, selected repositories/setup behavior, credential-profile/proxy fields, and unknown fields. Refuse or reconcile and reread the environment. Invoke that admission at the last provider-supported point before every create; define and enforce the corresponding rule for resume/reconnect. Prefer an immutable environment-version identifier in the create request if the provider adds one. With the current API, compare the concrete sandbox receipt's environment/version to the admitted version before issuing any Nessa SDK, harness, SSH relay, or MCP-grant action. On mismatch, retain the actual VM target in recovery evidence and clean it up without silently creating a replacement. This post-create check prevents Nessa from extending authority to a mismatched VM; it cannot claim to prevent a privileged Boat actor/provider from injecting data during the preceding name-based create. Add provider-boundary tests for each hostile field, existing-environment admission, mutation between save and create/resume, receipt mismatch, and cleanup/recovery preservation.

No current test mentions the five environment posture fields outside production `boat.rs`; the workspace tests exercise allocation receipts and cleanup, not this provider admission.

### P2 — Relay refusal bodies and watch frames are not bounded to the protocol

**Owner and exact locations**

- `crates/nessa-server/src/github_relay/infrastructure/transport.rs:116-129`
- `crates/nessa-server/src/github_relay/infrastructure/transport.rs:137-152`
- `crates/nessa-server/src/github_relay/infrastructure/transport.rs:179-193`
- Contrast the correctly bounded success decoder at `transport.rs:205-213`.

**Observed behavior**

HTTP 409 and 410 responses are decoded with `response.json()` and no byte ceiling. A 200 page is streamed with an explicit approximately 2 MiB bound, but the negative paths do not share that boundary.

The watch path calls `tokio_tungstenite::connect_async(request)` with default `WebSocketConfig`. In the pinned tungstenite 0.29 source, defaults permit a 64 MiB message and 16 MiB frame. Nessa's protocol accepts exactly the tiny text `{"type":"changed"}` (or ping), so those defaults are far larger than the protocol requires.

**Concrete hostile sequence / reproduction**

- A compromised or malfunctioning build-baked relay returns HTTP 410 (or 409) with a very large body. Reqwest collects it for `.json()` before the gateway can reject the schema.
- Or it completes the WSS handshake and sends a large text/binary frame. Tungstenite admits/assembles up to its library default before `changed()` rejects the content.

The build-baked HTTPS origin prevents a normal arbitrary host override; the relevant precondition is compromised/misbehaving relay service, origin infrastructure, or a valid TLS endpoint serving an erroneous response. The impact is local gateway memory pressure or termination across a process boundary whose ordinary success path is otherwise explicitly bounded.

**Authoritative invariant**

Every response and message read from outside the process has a protocol-derived byte bound on success and failure paths. A refusal must not consume more memory than a valid bounded response.

**Minimal owning fix**

Use one bounded response-body decoder for 200, 409, and 410 (and any future body-bearing status). Connect with an explicit WebSocket configuration whose frame/message bounds fit `changed` and ping/pong. Add hostile tests for oversized 409, oversized 410, oversized text/binary watch frames, fragmentation, and a normal changed/ping flow.

### P2 (defense in depth) — Relay followers downgrade a zeroizing GitHub token into long-lived ordinary strings

**Owners and exact locations**

- `crates/nessa-server/src/github_relay/application/ports.rs:3-5`
- `crates/nessa-server/src/github_relay/infrastructure/credentials.rs:82-87`
- `crates/nessa-server/src/github_relay/application/follow.rs:98-110`

**Observed behavior**

The sign-in domain deliberately stores `AccessToken` as `Zeroizing<String>`, but the relay credential port returns `Result<String, Failure>`. `ConnectedGithubCredentials::token` performs `t.expose().to_owned()`. Each active follower retains that ordinary allocation across catch-up, subscription, notifications, and the 12-minute renewal interval. With the 16-binding limit, up to 16 non-erasing token copies can remain allocated concurrently.

**Concrete exposure**

After enabling live events, inspect the call path or a same-process memory/crash snapshot during the renewal interval: each follower owns a normal `String`. Dropping or renewing it does not zero the allocation. This is not a direct remote token-read primitive; the threat is residual disclosure to crash dumps, memory inspection, or later allocator reuse after a separate same-process compromise.

**Authoritative invariant**

Long-lived credential copies preserve the secret owner's redaction and zeroization semantics. Exposure as `&str` occurs only at the immediate HTTP request boundary.

**Minimal owning fix**

Change the port to return a relay-port-owned secret/zeroizing credential type (or another dependency-safe wrapper that preserves `AccessToken`'s semantics), keep that type through the follower lifecycle, and expose a borrowed string only while building each request. Avoid an ordinary `String` clone in the infrastructure adapter. Add a compile-time/type-level regression test or boundary test demonstrating the port cannot return an ordinary string.

## Checks run on the frozen feature HEAD

- `pnpm --filter @nessa/github-webhooks test` — **44 passed**. This included typecheck/dry-run Worker build, signed ingress/HMAC, no-content-length oversize body, replay/conflict, D1 capacity, current GitHub identity/private repository installation authorization, stage isolation, gaps, wake tickets, quota and indexed-expiry coverage.
- `cargo test -p nessa-server github_relay --lib` — **23 passed**, 3,986 filtered. Covered native cursor/generation races, disconnect fencing, SQLCipher rollback/reopen/replay, gap acknowledgement, real HTTP gap receipt cases, follower ordering, stage table, and vault key lifecycle.
- `cargo test -p nessa-vault` — **16 integration tests passed**. Covered authenticated file format/corruption/capacity, concurrent writes, connection-close and explicit-lock races, stale generations, cooldown, audit failure, and secret redaction.
- `cargo test -p nessa-server workspaces --lib` — **18 passed**, 3,991 filtered. Covered allocation serialization, durable intent/receipt, rollback/recovery evidence, private assignment storage, encrypted Boat settings and stale vault generations. It did not exercise Boat environment admission.
- `git diff --check cfd025c121e1b32c42af7b4b062d64915a481c18..cf9b388247f22867c7ff48034e257d239c6dedef` — clean.
- Static hostile-path probes verified that `Provider::validate` has only one caller, create/resume bypass it, the Boat request selects environment by name, refusal decoding uses the two unbounded `.json()` calls, and watch uses default `connect_async`.

All executed tests used repository fixtures/local Miniflare/temp storage. I did not read real credentials, contact GitHub/Boat/Cloudflare, run a live pentest, mutate cloud state, or provision resources.

## Reviewed boundaries and non-findings

I inspected, with adjacent tests/contracts where present:

- Worker webhook streaming body bound, fatal UTF-8, HMAC-SHA256 verification before JSON parse, UUID/event validation, immutable replay/conflict handling, atomic D1 capacity/accounting, and payload-free logging.
- Cloud current-user and repository verification, OAuth scope/current App installation and permission checks for private repositories, Cedar default-deny policy, expected-user equality, per-user budgets, stage-bound expiring wake tickets, and stage-null release configuration.
- Native binding validation, cursor/page/gap validation, second catch-up after watch, generation-fenced commit/unbind, transactionally coupled event/cursor/audit writes, disconnect cleanup, and current-account filtering.
- SQLCipher key creation/recovery refusal, plaintext upgrade preservation, passphrase vault KDF/AEAD/atomic persistence, generation and close/EOF races, encrypted Boat/GitHub settings, and one-time legacy OS-store migration.
- Boat allocation receipts, cleanup uncertainty, private SSH identity/host-key pinning, local environment/open-credential clearing, and per-open MCP stand-in mapping/revocation.
- Synthetic merge integration of current-main owner-phone review permissions and five-minute wait changes. No combined conflict resolution was present; the later main changes did not add direct GitHub/vault storage bypasses.

I found no event-to-conversation/agent automatic dispatch path. GitHub payloads/comments remain untrusted stored/read data; future PR-to-chat agent dispatch is not implemented.

## Explicit exclusions and residual limits

- The source-observed native audit row-cap overshoot on direct gap/CLI acceptance is the known P3 tracked in #1185. I verified the adjacent wrong-table trim remains in this snapshot; I did not duplicate it as a new finding or claim an unconditional cap.
- The pane-root hidden-overflow/conversation-stress defect is #1184 and its separate draft #1174; no merge authority for that UI fix was assumed.
- Broad UI families and the local Mac fixture remain outside this security review except where UI/native vault lifecycle called into reviewed boundaries.
- I did not independently attest the published Boat image/archive/lease hashes, provider secure erasure, Cloudflare managed encryption implementation, real OS keychain behavior, live GitHub membership, live rate-limit distribution, or cloud deployment state. Provider deletion acknowledgement is not secure-erasure attestation.
- The development relay's permanent repository-gap metadata admission cap and shared Cloudflare Free budgets remain documented availability limits. A distributed flood or enough distinct installations can consume shared capacity; the current design does not claim a global denial-of-service guarantee.
- Incoming webhook plaintext is visible to the relay by design. This review makes no E2EE claim.

This report is evidence for the frozen revisions above, not a blanket security guarantee. Any fix changes the reviewed snapshot and requires a focused closing review of the owning invariants and adjacent paths.
