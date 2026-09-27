# Gateway alpha architecture and quality review

Date: 2026-09-16. The assembled stack was reviewed from base
`c4e1d4d0ab5659f0a62d6c891b7ea26291a75a20` through head
`a1c569b4a73bda952a9a515fd04a84aa7650b7c4`. The review-evidence commit that
contains this document changes documentation only. This is a review of the
stacked pull requests, not a merge or production release approval.

## Stack boundaries

Merge these pull requests in order. Each branch is based on the preceding branch
so reviewers can assess one vertical at a time:

1. `#35` — SDK execution lifecycle and restoration invariants.
2. `#36` — local authentication and storage boundaries.
3. `#37` — authenticated gateway conversations, protocol client, and MCP shell.
4. `#38` — managed desktop gateway and panel integration.
5. `#39` — repository organization enforcement and this review evidence.

## Decision

Keep the existing ownership chain:

```text
frontend projection -> authenticated client -> gateway conversation application
                                                -> SDK Agent / SessionManager
                                                -> provider adapter
provider tool call -> MCP shell application -> Shepherd / private audit adapters
```

Arrows denote calls, not shared mutable authority. The SDK remains the scheduler;
the gateway owns authorized conversation access and read projections; the
frontend maps those projections to AgentEvent/TranscriptBuilder. Concrete backends
are selected by composition, not by the domain or the renderer.

Browser authentication uses one private gateway credential only for sign-in. The
server exchanges it for a random 256-bit opaque cookie ID and persists only that
session ID's `CredentialId`, exact origin, and idle lifetime. Every authenticated
operation resolves current credential, membership, and policy from the auth registry;
the browser-session record never grants authority by itself.

Establish context-first directories and role modules for new code now. The new
MCP shell and desktop gateway are concrete examples. Existing SDK layouts and
all their callers are explicitly deferred to the
[SDK organization task](../todo/sdk-context-first-organization.md), per the user's
scope clarification. No compatibility aliases or data converters were added.

## Review responsibilities

- **Sol backend reviewer:** SDK queue/message evidence, gateway domain/API,
  initialization, persistence, cancellation and cleanup ownership.
- **Sol client reviewer:** DTO validation, replacement projections, agent-stream
  mapping, optimistic input and polling; T3Code comparison.
- **Sol authentication reviewers:** opaque browser-session persistence, current
  authority resolution, replacement ownership, timeout compensation, restart
  replay, and audit correlation.
- **Luna architecture reviewer:** OpenHands comparison, composition and tool
  organization; implemented the MCP layout change.
- **Coordinator:** assessed and narrowed findings, reviewed desktop/MCP effects,
  added host injection and dependency checks, corrected documentation, and
  verified the assembled tree.

Reviewers examined the assembled implementation and its intermediate stack
boundaries, not just this report. No external repository was treated as a coding
standard for Nessa.

## Findings and decisions

| Finding | Trigger / consequence | Decision and evidence |
| --- | --- | --- |
| P1: unowned initialization slot | Cancel the first read after reserving a slot; shutdown could wait forever for an initialization nobody started. | Fixed. Initialization is claimed while holding the owner-map lock and runs under a detached supervisor. `first_read_caller_loss_cannot_leave_an_unstarted_shutdown_slot` covers caller loss and shutdown. |
| P1: transient open failure cached permanently | A one-time storage/provider outage remained in the OnceCell and consumed capacity until restart. | Fixed. Only typed, resource-free transient failures retire their exact slot generation. Corruption/configuration failures stay explicit; uncertain cleanup retains its owner. Tests cover storage retry, provider retry, and unconfirmed cleanup. |
| P1: incomplete provider frame permitted dispatch | A complete frame followed by a partial frame could clear the reader's in-progress state, so an empty OS probe authorized a new prompt over incomplete provider evidence. | Fixed. Reader state now includes bytes retained by the incremental framer. A deterministic transport regression and repeated Unix dispatch-policy runs cover the boundary. |
| P1: unknown native tools could bypass review policy | The adapter admitted any bounded native tool name except a small deny set, while the provider ask policy covered a smaller explicit set. | Superseded. The explicit reviewed set was the wrong half to fix: the harness defers tool schemas, so a name the adapter had not heard of refused the whole execution. Admission and the ask policy are now aligned the other way — the ask policy is `*`, so every tool the harness offers reaches Nessa's permission owner, and admission needs no enumerated set. `DISALLOWED_TOOLS` is the deny boundary; configured MCP namespaces remain explicit. |
| P2: SDK fixture raced explicit native cleanup | A policy-test provider returned its response and exited before `ProcessScope` owned termination, so parallel macOS runs could observe uncertain process-group cleanup. | Fixed. The fixture stays alive until cleanup closes stdin, retaining the original two-second budget and production semantics. The exact regression passed 200 repetitions, all worker tests passed, and all 702 SDK tests passed. |
| P2: metadata I/O under global owner lock | A blocked create prevented unrelated conversations from obtaining the map lock. | Fixed. Reserve capacity and initialization ownership under lock, then perform repository work in that slot's supervisor. `blocked_metadata_create_does_not_hold_unrelated_live_owner_lock` verifies isolation. A permanently blocked repository write still retains its owner; the current port has no cancellation acknowledgement. |
| P2: client accepts contradictory identities | Duplicate execution/tool/permission records collapse maps and UI keys; self/forward steering can misplace input. | Fixed in `conversation-validate.ts`. Tests reject provable contradictions before returning a view and preserve valid truncated references and pending/message overlap. |
| P1: existing conversation ownership checked after reservation | A non-owner could request a known UUID after restart and consume capacity before final rejection. | Fixed. Durable metadata is loaded or created and existing ownership is verified before slot reservation or provider work. The restart regression proves zero provider opens for the non-owner and subsequent owner access. |
| P1: conversation creation lacked audit acknowledgement | Ownership creation and provider opening could report success without an application-owned audit sink. | Fixed. Creation/reopen records carry target, before/after state, cause, verified initiator, correlation and times; audit failure blocks success and provider open, while caller loss does not abandon admitted work. |
| P1: failed initial audit could lose the creation transition | Ownership was durable before its independent audit write. A retry could mislabel the original `Absent` to `Owned` transition as an idempotent reopen. | Fixed. The durable conversation retains the original creator, surface, correlation and request time. Initial audit records use a deterministic identity, retry validates an existing exact record or writes the missing one, and provider restoration remains blocked until that evidence is acknowledged. |
| P1: malformed controls restored dormant providers | Remove, answer and cancellation commands constructed caller-supplied value objects after resolving the conversation. Invalid input could therefore start a provider before rejection. | Fixed. Caller attribution and every command value object are validated before resolution. Dormant-conversation regressions prove malformed input causes zero provider opens. |
| P2: pending and message text disagreed | The server retained up to 8 KiB in a queued message but exposed only 1 KiB for the matching pending item, while the official client requires equality in a complete view. | Fixed. Both representations use the same UTF-8 byte bound, with Rust projection and TypeScript validator regressions above the old boundary. |
| P2: response validators accepted schema drift | Unknown fields and contradictory pending/message or tool tuples could cross the generated client boundary. | Fixed. Exact-key and cross-field validation now covers every conversation result and nested DTO, with explicit bounded-view truncation allowances. |
| P1: PR37 CI omitted its new contracts | The inherited workflow could pass without compiling `nessa-mcp` or exercising the official client, product protocol, architecture and formatting gates. | Fixed. A PR37-scoped Ubuntu job runs MCP tests/Clippy and an isolated no-vendor client/protocol harness in addition to workspace formatting and architecture checks. |
| P1: PR37 CI invoked the retired binary contract | The auth harness copied `nessa-server`, and the development command omitted the required `server` subcommand after the executable became `nessa`. | Fixed in PR37. CI copies `nessa`/`nessa.exe`, and both the package command and dependency-injection example invoke `nessa server` through Cargo. |
| P2: Windows tests read a journal through its live exclusive lock | Four persistence tests inspected raw journal bytes while `PersistentSessions` still owned the file, which correctly fails on Windows with sharing error 33. | Fixed. Each test releases the store before durable inspection; production exclusive locking and acknowledgement ordering are unchanged. All other raw journal inspections already followed this ownership boundary. |
| P1: non-macOS builds compiled inactive retirement services | The macOS managed-gateway retirement application and file adapter were compiled on Windows even though their signal wiring was inactive, so Clippy correctly rejected 23 dead-code warnings. | Fixed. Production retirement code is scoped to macOS at composition, application, infrastructure and retirement-only domain boundaries; portable runtime identity remains cross-platform. The local architecture gate requires active adjacent target attributes, rejects commented or broadened guards, and protects the portable identity from target gating; native platform Clippy remains authoritative. |
| P2: desktop stop delivery errors discarded | A failed launchctl invocation disappeared when the user enabled stop-on-quit. | Fixed. `GatewayHost` returns typed errors; the caller reports failed delivery. Tests prove isolated injected service identities and no effect after failed registration. Signal delivery still does not prove physical cleanup. |
| P1: desktop quit could target an unverified service | Stop-on-quit derived a service label even when reconciliation never established authority over that process. | Fixed. Shutdown uses only the exact identity retained from successful reconciliation; unreconciled and foreign services are never signalled. |
| P1: a reconciled label could later name another process | A label retained after successful reconciliation could be reused after replacement or a failed later reconciliation. Quit could then signal the new owner. | Fixed. Stop authority is a typed service label, PID, runtime fingerprint, instance and generation tuple. Reconciliation clears prior authority before each attempt; shutdown rereads launchd and managed health and sends nothing unless every field and process-identity certainty still agree. |
| P1: clean and cross-platform packaging diverged | Non-macOS preparation failed unconditionally, and clean CI did not build the linked agent-stream package. | Fixed. Managed runtime preparation is capability-scoped to macOS, other hosts retain their packaging path, and pinned vendor bootstrap installs and builds every linked package from a clean clone. |
| P2: browser session duplicated authority | Persisted principal, organization, membership, credential expiry and authorization revision required a separate integrity-key subsystem and could disagree with the current registry. | Fixed. The journal keeps only `CredentialId`, origin and idle lifetime. Current authority is resolved for checks, WebSocket admission, policy, dispatch, renewal and logout. The copied authority and integrity-key subsystem were removed. |
| P1: raw credential IDs could restore authority | A public application method accepted a `CredentialId` as sufficient evidence to construct an authenticated session. | Fixed. `ResumeSession` requires bounded opaque `SessionEvidence` validated by a `SessionVerifier`; the browser adapter binds it to current lifetime and exact origin before current registry resolution. |
| P2: auth paths followed intermediate links or failed first-run roots | Path-based registry setup could traverse an ancestor link, while strict Windows roots rejected an unprepared first-run directory. | Fixed. Registry I/O is handle-relative beneath an explicit private root; offline composition prepares that root atomically. Unix intermediate-link and Windows protected-root regressions cover both cases. |
| P2: desktop credential reads followed intermediate links | The desktop surface credential used a protected leaf open on a full path, leaving intermediate auth directories replaceable with links. | Fixed. Credential loading is handle-relative beneath the trusted root and rejects an intermediate symlink in a dedicated regression. |
| P2: abandoned replacement restoration lacked exact provenance | A fabricated same-origin prior session could be supplied to cleanup, and replay could accept it. | Fixed. The store derives replacement provenance from its own atomic journal transition, requires exact prior ID and state, rejects competing and same-ID replacements, and reconstructs the same proof on restart. Live and tampered-journal tests cover stale, fabricated and contradictory restoration. |
| P2: browser session IDs and journal resources were weaker than their contract | Concatenated UUIDv4 values supplied 244 random bits, and the append-only journal had no total byte bound. | Fixed. IDs use 32 direct CSPRNG bytes with a lossless lowercase-hex boundary. The complete journal is capped at 64 MiB before replay and append; reaching the bound preserves all accepted audit evidence and requires explicit offline archival rotation. |
| P2: optional DTOs missing public API documentation | TypeDoc omitted `ConversationPart`/`ConversationRuntime`; schema descriptions were absent. | Fixed in the schema and client exports, regenerated through the existing generator. Public API documentation check passes. |
| P3: new modules too flat / typed validation missing | New shell domain/application files and host gateway mixed distinct responsibilities, making future placement ambiguous. | Fixed with context-first role modules, application-owned host port, typed shell validation and admission failure, corresponding tests and module maps. |
| P3: stale architecture and enforcement gaps | Docs claimed send/stop were no-ops; architecture checks inspected only frontend imports and misread valid Rust strings/comments. | Fixed documentation. Rust roots are discovered from the workspace and all Tauri contexts, paths are normalized across hosts, and the scanner masks strings plus nested comments. Negative fixtures run locally and in CI. |
| P3: gateway infrastructure module root contained selection logic | `infrastructure/mod.rs` implemented the target adapter selector despite the repository rule reserving module roots for documentation, declarations and re-exports. | Fixed in PR38. Target selection now lives in `infrastructure/selection.rs`; the module root only declares native adapters and re-exports `current`. Desktop tests, Clippy and architecture checks pass. |
| P3: browser sign-in used a function-local Rust import | `SignIn::execute` imported authentication port types inside the method despite an existing grouped module import. | Fixed in PR37. `AccessError` and `CredentialEvidence` are grouped at module scope and the signature uses the short type name; formatting and server Clippy pass. |
| P1: reorder could change live order without retaining its history | `Agent::reorder_queued` applied the validated order to the live queue and only then waited for the evidence mutex inside a bounded select. An unrelated observation save holding that mutex made the wait time out, so the live queue dispatched an order replayed history did not contain and queue validation rejected the next selection. | Fixed in PR35. `SessionManager::retain_queue_reorder` suspends only while acquiring evidence ownership, before either effect; appending the mutation and applying the live order then complete synchronously, and storage moved to a separate interruptible flush. A rejected mutation no longer stays in retained history. `reorder_waiting_for_evidence_leaves_live_order_and_history_agreeing` blocks the evidence mutex with a stalled observation save while the reorder leaves its audit; it fails against the previous ordering and passes after the fix. |
| P1: failed creation audit could be bypassed through read or send | `create` persisted ownership and then requested the mandatory creation audit. On audit failure ownership remained while provider opening was correctly withheld, but `resolve` — reached directly by the authenticated read and submit routes — checked ownership and opened the provider without reconciling that failed audit. | Fixed in PR37. Every first provider opening now loads the authoritative creator record and reconciles its `CallerRequested` audit before any provider work, so the gate is the slot opening rather than one entry point. Audit failure retires the slot and a later attempt retries idempotently. `IdempotentReopen` is recorded after the creation it repeats. `read_and_send_cannot_open_a_provider_before_the_creation_audit_is_reconciled` proves zero provider opens and zero dispatches for create, read and send while the audit rejects, then exactly one reconciled record and one provider open on recovery. |
| P2: a lagged projection discarded authoritative snapshot text | `lagged()` set a persistent fence and `event()` suppressed text while it was set. Settlement rebuilt a message by replaying its committed record through that same suppressing path, so a completed response could stay blank although its text was saved. | Fixed in PR37. Live broadcast handling and authoritative snapshot reconstruction are separate: `observe(event, authoritative)` keeps the live fence and lets a committed terminal record rebuild its text once. The existing terminal guard still drops buffered live chunks arriving afterwards. The regression covers text, lag, terminal snapshot, settlement, a late buffered chunk and a subsequent invocation. |
| P2: interrupted metadata creation could poison a conversation ID | `LocalConversationRepository::create` created the final `<conversation-id>.json` and then wrote it, so a crash or failed write could publish an empty or partial record that every later read and create rejected. | Fixed in PR37. Ownership bytes are written and synced under a private sibling temporary and published with exclusive, no-replace semantics (`fs::hard_link` on Unix, `MoveFileExW` without `MOVEFILE_REPLACE_EXISTING` on Windows), then the directory is synced. Taking the directory releases temporaries an interrupted publish left behind. Genuinely corrupt completed records still fail closed and are never repaired. Regressions cover the storage primitive and repository recovery. |
| P2: a complete queue replacement retained stale confirmed rows | The gateway exposes up to 64 pending inputs but retains 24 message rows. After the queue was stopped, `applyView` kept the omitted local rows as `queued` because they carried action IDs, and `failSend` then counted them as other active work and left the panel `thinking` while offline. | Fixed in PR38. Unacknowledged local intent and local failures are still retained, but a server-confirmed `queued`/`accepted` classification is retired when a complete queue omits that identity; an incomplete queue keeps everything. Two regressions reproduce the reported failure and three controls plus an incomplete-queue case prove nothing else is discarded. |
| P1: Windows runtime link containment and identity tests | `runtime-fingerprint.mjs` detected absolute link targets with `target.startsWith("/")`, which does not recognize Windows drive-absolute or UNC targets, and one test asserted POSIX executable-bit identity on a platform whose `chmod` cannot change `stat.mode`. | Fixed in PR38. Containment uses `isAbsolute`, matching the Rust fingerprint's `target.is_absolute()`. Path and byte-boundary framing identity is asserted on every host; the executable-bit rule is asserted only where the platform reports one, and the skip is explicit rather than a broadened assertion. |
| P1: Windows desktop host compiled unused platform items | With every platform module resolved for a non-macOS, non-Linux target, the live-resize event names, the monitor lookup the pinned webview needs, and the reconciled service accessors had no consumer, so desktop Clippy correctly rejected them. | Fixed in PR38. Each declaration carries a target-conditional dead-code allowance naming the hosts that use it. The host/shell seam still declares every protocol name on every target, so its drift test remains complete, and the reconciled runtime identity is not hidden behind a target cfg. |
| P2: a reopen was attributed after the conversation became usable | Moving the mandatory creation audit into the opening gate left `create`'s idempotent-reopen record after `resolve`. A reopen whose attribution the audit sink refused therefore returned an audit failure with the provider already open and usable through every other entry point. | Fixed in PR37, found by re-review of that gate. Both create branches now acknowledge the original creation, attribute the reopen, and only then open, through one shared reconciliation. `a_failed_reopen_audit_refuses_before_the_conversation_becomes_usable` rejects only the reopen record and asserts no provider open; it fails against the previous ordering with two opens. |
| P2: publication had a platform-divergent public contract | `publish_new` was exported although its post-conditions differ by platform — the Unix link leaves the writer's own name behind, the Windows move does not — and it had no consumer outside the crate. Its `PrivateTempFile::publish` wrapper also relied on `Drop` to release that name and discarded the failure. | Fixed in PR37. `publish_new` is crate-internal, and `publish` releases its own name before reporting success, reporting a failure to do so instead of returning `Ok` for a two-link record. A failed sweep of leftover temporaries reports its cause instead of discarding it. |
| P2: the reorder held its admission lock across two bounded waits | Splitting the reorder into retention and persistence gave each its own 30-second bound, so the scheduler's admission lock could be held for their sum. Automatic teardown never signals explicit close, so it could not shorten that wait. | Fixed in PR35, found by re-review of that fix. Retention and persistence share one budget measured from the moment the lock is taken. A close before retention now reports `Closed` rather than a storage fault, matching the audit wait, and `close_interrupts_reorder_retention_and_leaves_the_order_unchanged` covers that branch. |
| P2: a rejected mutation rolled back for callers that had already acted | The rollback for a rejected queue record lived in the shared append helper, whose other callers record removals, admissions and selections the scheduler has already performed. Rolling those back would erase evidence of a completed transition. | Fixed in PR35. Only the reorder transition rolls back, and its live order change is fallible rather than asserted. `a_refused_reorder_retains_no_mutation_and_never_changes_the_live_queue` proves the refused record leaves observed evidence and the live queue untouched. |
| P2: a slow private-file bridge was reported as an unsafe one | Each Windows bridge call starts PowerShell and compiles its Win32 helper through Add-Type, so one call's floor is a C# compiler run. The 15-second budget sat below that floor on a cold or loaded machine, and exceeding it reported the credential file as unavailable or unsafe — a claim the bridge had not established. The Windows fixture asserted only that its safety checks rejected, so a timeout satisfied them without the check ever running. | Fixed in PR36, found by a Windows CI failure whose sources no commit in this stack touches. The budget is 60 seconds and a timeout is its own typed failure; every fixture refusal is asserted by that type, so a bridge that only ran out of time fails the fixture. Production credential loading still reports one unavailable result, so panel behavior is unchanged. |
| P2: a self-revoking credential had to win its own race | Revoking the credential a session is using races its own acknowledgement: the gateway answers the mutation and closes the socket it just revoked. The end-to-end check required the answer to arrive first, so a Windows runner that delivered the close first failed the whole flow with a mutation error caused by the very close the step was asserting. | Fixed in PR36, found by a Windows CI failure. The step accepts the answer when it arrives and a close carrying `credential_revoked` when that arrives instead, and rejects any other failure. The close, its reason, its non-retryable policy and the resulting connection state are still asserted, and the flow passed three consecutive local runs. |

### Rejected or narrowed recommendations

- Rejecting every missing reference is wrong for bounded snapshots. The gateway
  can truncate collections independently. Pending work intentionally overlaps
  messages, and a completed steering target is valid history. Only contradictions
  supported by the retained evidence are rejected.
- A long service file alone is not proof of a broken aggregate. Keep projection,
  repository, provider and wire translation separate; do not add another scheduler
  or split one live-session consistency boundary into independent aggregates.
- Rebuilding the bounded transcript is acceptable at current limits. Indexing
  repeated scans becomes worthwhile if those limits or consumer counts grow;
  no speculative cache was added.
- Do not adopt an event database, backend registry, generic workspace framework,
  or migration machinery solely because another product uses one.

### Findings narrowed with an explicit reason

- A locally submitted row the gateway no longer lists in a complete queue, and
  no longer carries in its bounded message view, is dropped rather than retired
  to a local terminal receipt. Inventing an outcome the gateway never reported
  would be worse, and retaining the row indefinitely would unbound the
  transcript. The same view marks the omission with `truncated`, and the
  gateway keeps the real cancellation evidence in its own audit. This is the
  documented behavior of a bounded replacement view, not silent loss, and the
  regression asserts that marking alongside the retirement.
- The rejected-mutation path inside a reorder's retention is not reachable from
  the live producers today: restoration gates on the same replay, and every
  live mutation is constructed from validated domain state. It is covered by a
  direct application-level test rather than an end-to-end one, because forcing
  it through the public API would require a production test switch.
- Windows behavior in this change set is established by the workflow's Windows
  job, not by a local Windows build. The two-link publication window and the
  private-file link-count rule are exercised on Unix; on Windows publication is
  a move, so that window does not exist there.

## Comparison with external projects

**T3Code**, pinned at `3efdcc5296f1754e0f3bf7fee5fc2ada510e0438`, separates
server-owned execution from clients and provider adapters. Its orchestration
uses persisted events, projections and command receipts committed together,
with effects performed afterward. Keep the ownership and acknowledgement
precision; Nessa's bounded replacement views remain sufficient today. A durable
cursor needs a real replay consumer and separate contract tests before adoption.
[Official architecture](https://github.com/pingdotgg/t3code/blob/3efdcc5296f1754e0f3bf7fee5fc2ada510e0438/docs/internals/overview.md).

**OpenHands software-agent-sdk**, pinned at
`4c89b6899d4a98e9ce8329490d54e1b2263cd517`, separates agent SDK, tools, and
agent-server packages. Its Conversation factory selects local/remote behavior
from workspace configuration. The useful lesson is explicit composition and
separate tool/runtime responsibilities. Nessa already has those seams; add a
second concrete workspace adapter when needed, without importing a broad
conversation object or observer framework now.
[Official Conversation source](https://github.com/OpenHands/software-agent-sdk/blob/4c89b6899d4a98e9ce8329490d54e1b2263cd517/openhands-sdk/openhands/sdk/conversation/conversation.py).

## Remaining release findings

Current release status:

- The registered launchd service now switches to a staged installed/moved
  runtime through an explicit, correlated retirement transaction. Cleanup and
  audit acknowledgement precede replacement, and readiness verifies the exact
  launchd process and runtime incarnation.
- Managed launchd service installation remains macOS-only. Other platforms keep
  their desktop build path and skip that capability without pretending it ran.
- Managed-update readiness verifies gateway identity in its health response and
  is followed by authenticated connection. Desktop stop-on-quit remains a
  separate request-delivery acknowledgement only.

Concrete follow-up scope is in
[remaining Linux desktop packaging](../todo/linux-desktop-packaging.md).
The review does not approve cross-platform packaging.

## Verification and limits

The final code is checked with frontend formatting, lint, protocol generation
consistency, client/root typechecks, all frontend tests, architecture negative
fixtures, client API docs, Rust formatting, the combined CI crate test selection
plus MCP, Clippy with warnings denied, and desktop host tests/Clippy on macOS.

Important relationships exercised: admission versus caller lifetime, slot versus
provider cleanup ownership, transient retry versus permanent evidence, repository
wait versus unrelated owner access, repeated identity versus projection mapping,
and truncated references versus complete-view constraints.

This review did not install a DMG, restart the user's gateway, run live
paid-model traffic, test Linux/Windows binaries, run LLVM domain coverage, or
perform a new browser visual/accessibility audit. Windows behavior is covered by
the workflow's Windows job, not by a local Windows build: the desktop host's
target-conditional items were reasoned about and verified through that job
rather than cross-compiled here. The import guard is a lexical
source check: macros, fully qualified expression paths, transitive exports and
semantic ownership still require review and compilation. It is not a claim that
all DDD properties are mechanically proven.

### Final checked tree

The [source manifest](gateway-alpha-scope.json) identifies 392 extant changed
non-docs files by SHA-256 and records 4 deleted paths. Manifest SHA-256:
`971ec35aaed49745cd535ca3d91c7c9ccd39c01bd4d83aa904a25828e97778f1`.
Ignored build and runtime artifacts are excluded.

Final results: 323 frontend tests, 175 server tests, 42 authentication tests,
four local-storage tests, 10 MCP tests, 705 SDK tests, and 74 desktop native tests
passed. Rustdoc, combined Rust Clippy, 19 desktop packaging checks, six architecture fixtures,
protocol generation consistency, formatting, typechecks, public API
documentation, and the authentication end-to-end flow passed. The macOS, Ubuntu
and Windows workflow jobs passed on each pull request's current head.
