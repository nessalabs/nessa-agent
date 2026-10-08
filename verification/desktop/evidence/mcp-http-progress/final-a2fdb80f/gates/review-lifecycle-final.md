# PR637 fresh independent combined review — round 6

Reviewed base `aa774e05a0c9d127ba9cab78a7068cc7b56e8388` through exact head `a2fdb80f14d186614ec63d245936cf4ad4c5df94` in `/workspace/nessa-agent-http-progress`. This is the explicitly authorized sixth round; the five preceding rounds remain counted. The checkout initially matched that head. Subsequent reads used immutable `git show a2fdb80` after the coordinator authorized a sole author to correct the finding below. This report approves no later tree.

Verdict: **one major/P1 finding; not clean at the reviewed head**. No additional actionable findings at other priorities.

## P1 — concurrent terminal shutdown can publish ended before the HTTP fence

Location: `crates/nessa-sdk/src/infrastructure/mcp/connection.rs:210` and `:517`, using `HttpSession::shutdown` in `http.rs:268–273`.

The newly added writer and reader terminal paths rely on `shutdown()` returning only after HTTP admission/publication is fenced. Its once-only guard does not establish that: it sets `delete_started` before acquiring `readers` and setting `closing`, and every competing caller returns immediately when the guard is already set. Independent Tokio reader/writer tasks can call it concurrently.

Reachable interleaving on a multi-thread runtime, with private recovery awaiting a replacement result:

1. An ordinary response body produces a terminal reader error while a peer-answer/control POST independently produces a writer `End`. The recovery owner is still live.
2. Reader A enters shutdown, sets `delete_started=true`, and is preempted before acquiring `readers` or storing `closing=true`.
3. Writer B enters shutdown, observes the already-set guard, returns without fencing, and calls `Shared.end(error)` at line 211.
4. Replacement startup C receives its valid result, acquires `readers`, observes `closing=false`, and `publish_initialize` claims/publishes replacement binding and spawns GET. C can run before A resumes. The connection's ended observation is already public.
5. A eventually fences and aborts cleanup. That cannot undo a GET already dispatched after the ended observation.

Expected: every terminal path establishes the existing HTTP registry fence before publishing `Shared.end`, including when another shutdown owns DELETE. Actual: the losing shutdown caller can publish ended while initialization/publication is still admitted. This violates ADR392 J19 and the agreement between connection terminal state and HTTP effect authority. The `SeqCst` atomic ordering does not order a future fence performed by A after B's return.

This is a source-confirmed interleaving, not a runtime reproduction executed by this reviewer. No outer lock serializes the two new terminal callers. Existing single-owner J19 reader/writer tests and duplicate-close tests do not cover the overlap: duplicate close is exercised after the DELETE gate has already been reached, when the fence is established.

Correction should make every caller acquire the existing registry fence and establish/observe `closing` before the once-only cleanup early return. Preserve the sole abort/join/DELETE owner and retained claim through actual DELETE completion. Add a deterministic competing-caller regression that distinguishes this head from the correction; cover both reader/writer arrival orders and keep the ordinary-terminal healthy recovery control. The coordinator acknowledged this finding; no future fix or runtime result is claimed here.

## Whole combined scope and agreement check

Inspected all nine changed paths: MCP `connection.rs`, `http.rs`, and `mod.rs`; `http_progress.rs`, `post_body.rs`, `post_streams.rs`, and the test `mod.rs`; ADR392 and `docs/codebase-structure.md`. Read the local review gate, all nine dimensions, agreement requirements, and gates 15–17 in `CODING_STANDARDS.md`, plus `AGENTS.md`. Inspected neighboring `HttpExchange` body semantics and connection call/cancellation/close/delivery paths.

| Dimension | Checked owners, relationships, and evidence |
| --- | --- |
| Authority and identity | `Phase.binding` is the owning immutable `Arc<SessionBinding>`; `ReplyContext` captures origin/version and `check_reply_binding` checks Arc identity on each authorized attempt. Followed provisional, reused-string, old-stateless, retry, and replacement contexts. J17–J19 and corresponding mutation pairs enforce these boundaries. |
| Domain invariants | One matcher/`receive_message` policy for JSON/SSE, buffered/streamed; `wire::initialized` precedes validated identity/version/GET. Provisional claim is distinct from readiness. `Recovery`/`validate_handoff`/`recovery_failure` retain first failure and separate publication from completion/admission. J2/J4/J10–J14 cover invalid and positive terminal forms. |
| Admission and concurrency | Traced reader-registry-before-phase lock ordering, registered startup barrier before headers, bounded writer handoff, outstanding initialization and caller cancellation. `complete_recovery` commits synchronous delivery/admission under one fence. Constructed simultaneous terminal-owner contradiction above; that relationship is not enforced at this head. |
| Failure and cleanup | `Shared.end` owns first published connection cause; later direct dispatch may correctly report Closed. `shutdown` owns physical abort/join/DELETE; `finished` and `SessionClaims` retain cleanup ownership through actual attempt completion. Provisional/public-open failure, collision loser, no-ID/legacy no-DELETE, and recovery-close evidence inspected. Concurrent shutdown fence is the finding. |
| Observations and restoration | Exact own terminal retires a POST before bad trailing bytes; neighboring ordinary replies retain their outcomes. Private initialize suppresses only its matching terminal. Captured reply evidence cannot borrow current binding. No snapshot/persistence/restoration contract changed in this slice; extracted #631/main work is outside this diff. |
| Audit | No new durable audit port or audited domain transition introduced by this transport slice. Checked that diagnostic/HTTP status does not manufacture readiness or remote cleanup acknowledgement and that connection cause/claim ownership survive cleanup. Existing outer application audit composition remains on main. |
| Representation boundaries | Typed request purpose and actual emitted headers determine status policy; 401 retries once, 403 records challenge, private initialization cannot select legacy fallback. Own-ID result/error shape and wire version checks precede publication. Session ID bound is checked before authoritative header copy/claim; ordinary returned ID is ignored. |
| Resource bounds | 256 POST permits retain physical-body/join ownership; inbound 64 and existing outgoing 64 queues remain bounded, pending RPC 256. Reply binding shares Arc identity; IDs capped at 1024 UTF-8 bytes. GET replacement aborts retained predecessors; close joins registered owners. Checked destruction gate and queue/deadline controls. |
| Public surface and organization | Private evidence types remain at HTTP/connection owner, tests use existing MCP fixture/module map, no new dependencies/process fixtures. `HttpSession` is not reexported; changing `open` visibility does not remove an exported SDK entry point. Owning module maps and canonical ADR are updated. |

Agreement traced: originating POST headers → immutable reply context → bounded writer queue → each authorized attempt → response classification; matching initialize → provisional/validated claim → GET/readiness → initialized acceptance → delivered completion → ordinary admission; terminal reader/writer error → HTTP fence → first public cause → physical joins/DELETE → retained claim release. The final terminal-to-fence relationship fails only in the competing-shutdown ordering described above. Healthy ordinary-terminal recovery and ordinary bound404 recovery positive controls remain inspected and accepted. No evidence of additional duplicated mutable authority or alternative cleanup controller was found.

Gate 15: ADR J1–J20/P1–P14 gives ordering/enforcer mapping; the newly found overlap must be added before implementation. Gate 16: existing registry/cleanup owner is sufficient to correct it; no extra recovery state/controller needed. Gate 17: inspected actual author browser results/summary for final a2f source: Apps16/16, gateway13/13, scenarios8/8, Chromium and WebKit, signed-out dev mode; performance and live-provider behavior are not established.

## Evidence inspected and explicit limits

Read author `self-review.json`, gate manifest, documentation source-equivalence manifest, browser rerun summary, 15 original mutation-pair manifests/runtime outcomes, two final terminal-owner applied hunks and actual runtime RED/GREEN logs. Actual terminal-owner RED logs assert late dispatch was incorrectly accepted when each new shutdown invocation was removed; restored logs pass. The wrong writer-path surviving mutant and E0382 compiler failure are excluded and are not runtime RED evidence.

Inspected actual final logs: HTTP progress 49 passed; MCP 214 passed/1 ignored; full six-package grouped runner passed with verification-only Linux child subreaper (21 physical orphan reaps), including SDK1059 and server1994 passes. Initial runner failures remain preserved. Rust gate source at 3add is byte-equivalent to a2f except the ADR Mermaid syntax line, verified by `git diff --name-only`; a2f browser evidence is its own rerun, not relabeled earlier source. Earlier browser setup failures and Chromium navigation abort remain excluded from the final pass.

This reviewer ran read-only git/source/evidence inspection only, **no Cargo, runtime tests, browser, mutation probes, or source edits**. Writing this requested report is the sole artifact change. These logs establish the author's executions, not a reviewer reproduction, and they do not disprove the new overlapping-owner interleaving. Windows compile helper boundaries were inspected; Unix-gated HTTP runtime fixtures do not execute on Windows. Physical WKWebView, signed-in/live providers, OAuth implementation, provider settlement/restoration redesign, performance, and confirmation of physical remote deletion are excluded. A permanently blocked custom authorization/DELETE intentionally retains the claim; no release-on-timeout claim is made. External CodeRabbit is pending and Greptile unavailable, neither is a pass.
