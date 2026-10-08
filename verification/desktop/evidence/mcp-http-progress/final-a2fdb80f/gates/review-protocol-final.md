# PR637 fresh final independent review — protocol and ownership

Reviewed exact base `aa774e05a0c9d127ba9cab78a7068cc7b56e8388` to head `a2fdb80f14d186614ec63d245936cf4ad4c5df94` in `/workspace/nessa-agent-http-progress`. HEAD and clean working tree were checked directly. This is round 6 under the explicit user exception; prior approvals were not used as proof. **One major finding remains; this is not a clean approval.** No other confirmed finding at any priority.

## Major: duplicate shutdown can return before the HTTP closing fence

Locations: `crates/nessa-sdk/src/infrastructure/mcp/connection.rs:210` and `:517` (new terminal shutdown callers), relying on `crates/nessa-sdk/src/infrastructure/mcp/http.rs:268–273`.

The new writer/reader terminal paths call `session.shutdown()` before `Shared.end`, intending to prevent recovery publication and HTTP effects after the end becomes observable. However, `shutdown` first swaps `delete_started` to true, and only later takes `readers` and stores `closing=true`. A second caller seeing `delete_started=true` returns immediately, without acquiring or observing the closing fence.

Concrete reachable interleaving on the supported multithreaded runtime:

1. A recovery startup body is registered and awaiting its matching initialize result. An old ordinary body or another independent body is also active.
2. The writer receives a terminal outcome (for example a stale queued peer reply), enters `shutdown`, swaps `delete_started=true`, and is preempted before acquiring `readers`.
3. The connection reader independently receives a terminal body error. Its `shutdown` sees true and returns; `Shared.end` publishes the error and resolves `ended()`/pending callers.
4. Before the first caller resumes, the recovery task receives its valid matching initialize result, acquires `readers`, sees `closing=false`, and executes `publish_initialize`: claims replacement identity, sets open/version, and spawns replacement GET. That GET may execute before the first shutdown caller raises closing and aborts readers.

Expected: once an automatic terminal end becomes observable, HTTP publication/admission has already been fenced. Actual: the DELETE ownership election is observable before that fence and allows a concurrent terminal publisher to bypass it. This contradicts ADR392 J19 and the newly added terminal-before-ended guarantee, and violates the admission/concurrency and agreement gates. It does not create two DELETEs; the defect is the ordering of cleanup ownership versus admission.

Make every `shutdown` caller synchronously establish/observe the registry closing fence before taking the duplicate-cleanup return. Preserve the single cleanup/DELETE election. Add a controlled overlap regression for writer and reader shutdown contenders plus a late recovery result; the existing `j19_reader_failure_fences_http_owner_before_ended` and preterminal stale-request tests use one effective shutdown caller and cannot distinguish this interleaving. This finding is source-proven; no runtime reproduction or mutation was executed under the read-only brief.

## Whole-diff scope and dimensions

All nine changed files were inspected: complete production `http.rs` and `connection.rs`, owning `mcp/mod.rs`, complete new `http_progress.rs`, `post_body.rs`, changes to `post_streams.rs` and the test module map, ADR392 changes, and `docs/codebase-structure.md`. Neighboring `HttpExchange`/response-body definitions, framing bounds, connection shared state/admission/drop, SDK metadata and test platform gates were inspected. `AGENTS.md`, `CODING_STANDARDS.md` (required local gate, all nine dimensions, agreement, evidence and gates15–17), architecture and dependency-injection context were read.

| Dimension | Assessment and concrete enforcers |
| --- | --- |
| Authority and identity | `Phase.binding` owns identity; captured `ReplyContext` shares immutable Arc evidence. `check_reply_binding` requires pointer identity and refuses absent/recycled/replacement ownership, including refresh retries. J17–J19 cover bound/stateless and reused-ID neighbors. No new second mutable identity authority found. |
| Domain invariants | `is_response_to` requires own numeric ID, no method, and exclusive result/error; only matching initialize result reaches `wire::initialized` and `publish_initialize`. `Recovery` separates publication, serialized initialized, delivered completion and admission. J2/J4/J9–J14 cover malformed/unmatched and valid neighbors. |
| Admission and concurrency | Reader registry serializes close, identity publication, startup registration and recovery terminal commit. Existing bounded writer carries frames/replies/RecoveryReady. Recovery blocks ordinary calls while controls remain available. **Major duplicate-shutdown ordering gap above** prevents full terminal-fence approval. |
| Failure and cleanup | Actual emitted POST context owns status classification; one401 retry and403 scope handling are shared for public/private initialize. Private startup cannot legacy fallback. Close retains claimed ID until its sole actual DELETE attempt ends; joins body owners before finished. Bound peer404 ends without replay/recovery; ordinary404 retains single recovery. J6–J8/J14–J20 exercise these distinct meanings. |
| Observations and restoration | Shared per-message policy covers JSON/SSE, buffered/streamed, matching retirement and preserved neighboring results. Ordinary response headers cannot replace originating reply identity. `Shared.end` retains first connection cause; later HTTP Closed need not equal that cause. No snapshot/persistence model is changed in this slice. |
| Audit | This diff adds no durable audit API or transition store. Existing outer MCP ownership remains outside the changed adapter; typed primary terminal causes remain available to consumers. No claim of durable audit delivery or physical remote deletion is made. |
| Representation boundaries | Session-ID limit is checked before authoritative copy/claim, UTF8 byte length <=1024; own initialize result is validated before GET. Framing/body limits and SSE errors remain typed. Headers for actual retries are rebuilt under closing/identity checks. Existing port header first-value behavior is unchanged; no new policy decoder was introduced. |
| Resource bounds | POST/startup permit count256 survives abort until physical destruction/join/reap; pending RPC256 and inbound/outgoing64 remain distinct. Weak body/startup callbacks avoid retaining session across waits. Arc bindings bound identity copies; body frame/event limit16MiB remains. Physical destruction regression uses controlled DropGate. |
| Public surface and organization | New evidence/event types remain private to owning HTTP/Connection boundary; no dependencies/exports/process fixtures added. `HttpSession::open` becomes crate-private to avoid exporting its internal evidence. Module maps and ADR track shared fixture and ordering ownership; SDK missing_docs enforcement remains. |

## Agreement and gates15–17

Related facts traced together: actual request headers → ResponseContext classification → captured binding/version → matching initialize validation → identity claim/GET → RecoveryReady → initialized acceptance → synchronous completion delivery/Completed → ordinary admission; terminal shared cause → HTTP closing fence → owned reader destruction → sole DELETE → retained claim release/finished. Authorities are `Phase`/`Recovery`, `Shared.state.ended`, request-building context, reader registry/closing, and shutdown owner. Checks reject stale captured ownership without allowing replacement, while genuine neighboring ordinary results and ordinary404 recovery remain valid. The shutdown finding is precisely an otherwise-valid cleanup election combined with an invalid observable end/publication relationship.

Gate15: ADR P/J ordering tables and named tests were inspected; the overlap of two shutdown contenders above is missing. Gate16: existing binding/writer/shutdown owner is the appropriate owner; no additional controller is needed to repair the ordering. Gate17: supplied actual final browser evidence was read: umbrella exit0, Chromium and WebKit each pass, Apps16/16, gateway13/13, scenarios8/8. This verifies the scripted signed-out app scope, not the wire-level race identified here.

## Checks and limits

Performed only source/diff/document/evidence reads and Git identity/cleanliness checks; no Cargo, source edits, mutation probes or browser executions. Supplied manifest records format, clippy, MCP/http-progress, architecture, SDK docs, and six-package suite passing; initial six-package run failure is preserved and corrected subreaper run passes. Rust checks are attributed to source-equivalent3add, final ADR-only head a2f; final browser rerun owns final source. Prior runtime probe reports and author review were treated as supporting evidence, not independent proof of concurrency completeness. External CodeRabbit remains pending and Greptile unavailable, neither a pass.

Exclusions: no new audit/persistence/restoration implementation; OAuth-provider internals, signed-in live remote providers, physical WKWebView, remote cleanup acknowledgement, Windows runtime execution and performance are not verified. Unix-gated fixtures do not execute on Windows; pure source helpers were reviewed for portable compilation but not compiled here. Indefinitely blocked custom authorization/DELETE intentionally retains exclusive cleanup ownership; that documented limitation is not this finding.
