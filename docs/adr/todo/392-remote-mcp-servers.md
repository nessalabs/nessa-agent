# 392. Remote MCP connections and authorization have explicit lifecycle owners

## Purpose and status

The gateway reaches remote MCP servers through an HTTP transport seam and owns
their authorization. A harness continues to use `nessa mcp-relay`, so its tools
and the desktop's apps share one upstream session for that harness opening.
Connection lifetime, token availability, consent and evidence settlement are
separate stateful concerns with explicit synchronization.

- **Date:** 2026-10-03
- **Statechart revision:** 2026-10-07
- **Status:** accepted. The gateway owns remote Streamable HTTP (and the
  scoped HTTP+SSE fallback), OAuth 2.1 (discovery, PKCE S256, dynamic
  registration or `registration_unsupported`, loopback callback, the
  private token file, refresh, revoke, and the fence before a URL change is live),
  honest app-call outcomes, and the desktop authorize/revoke/consent
  states. An owner-run check against a consenting remote server is still
  outside cloud evidence.
- **Tracking:** #392. Existing configuration work #391 is merged. The local
  `392-remote-mcp` branch retains three transport/test-server/design commits based
  on `bc2448f3`; this planning change does not merge its SDK/gateway code.

Use the [statechart authoring guide](../../state/authoring.md) and the canonical
[gates](../../../CODING_STANDARDS.md#gates). Existing stdio session and grant
behavior remains described in [MCP connections](../../design/mcp-connections.md).
The proposed remote state tables live here, rather than being independently
maintained in that implemented guide and issue comments.

## Decisions and scope

1. SDK connection logic consumes a transport port: send, receive and close.
   Stdio keeps its bounded framing. HTTP protocol logic consumes an SDK-owned
   `HttpExchange` port; gateway composition supplies the async HTTP adapter.
2. Remote configuration has stable server identity, display name and validated
   URL beside stdio definitions. Extend the existing stored-server owner and
   `mcpServers.list/save/remove/inspect`; retain enabled state, optimistic
   revision, audit and one live-set publication contract. Do not restore the
   older branch's parallel configuration reader over the merged #391 owner.
3. Each harness opening has its own logical remote MCP session. No two
   conversations share the gateway's upstream session/streams. If a server
   returns an id already held by another opening at that MCP endpoint, refuse
   the collision before readiness and release only this opening's local work;
   DELETE of the other opening's id is not cleanup. A server can omit that
   header; retain the local connection identity regardless. The gateway cannot
   promise that a remote server internally isolates its own application state.
4. The relay grant remains the ownership boundary. Revocation or conversation
   closure fences openings and closes owned local sessions, including those
   initializing. Remote transport does not enter the provider restoration
   fingerprint; consume the existing restoration-identity publication.
5. Support Streamable HTTP and the explicitly scoped 2024-11-05 HTTP+SSE
   fallback requested by #392. Fallback follows only initial POST statuses
   400/404/405. Authentication, malformed responses and unrelated failures do
   not trigger a different transport.
6. OAuth is gateway-owned: protected-resource/authorization-server discovery,
   PKCE S256, resource binding and client registration when supported. Tokens
   stay in the injected private credential store, never in settings, prompts,
   diagnostics, tool arguments or browser URLs.
7. The native host opens the returned consent URL. The gateway receives the
   loopback callback, bound to a single-use state and pending attempt deadline.
   An unavailable private token writer returns an honest unsupported outcome.
8. Authorization/refresh/revoke require correlated durable audit. Failed audit
   prevents audited success and cannot prevent necessary local cleanup or
   fencing token use.
9. Client registration without dynamic registration is outside the first OAuth
   slice. Expose `registration_unsupported` rather than claiming a server can
   be authorized. Resource/issuer/endpoint changes require new explicit consent.
10. Apps continue through the existing resource/tool/review gateway owners.
    Proposed server-level CSP consent is bound to the declared domain-set digest
    and asks again when that set changes. It is separate from OAuth consent.

A successful local close does not prove a remote tool stopped. DELETE may be
refused or unconfirmed, and a dropped stream is not a cancellation receipt.
Retain those meanings in typed outcomes and evidence.

## Current ownership and proposed changes

| Owner | Current responsibility | Proposed extension |
| --- | --- | --- |
| SDK `infrastructure::mcp::Connection` | Correlation, bounded frames, replies and connection end | Transport port; HTTP JSON/SSE decoding with the same correlation and bounds |
| SDK `McpServers`/`McpOwner` | Live configured set and sessions owned by SDK session/relay grant | Remote descriptors; owned initialization and local close |
| Gateway `mcp_servers` | Relay admission, live set, stored edits, inspection and app access | Async HTTP adapter and current stored-server remote variant |
| Gateway new `mcp_authorization` context | Not implemented | Per-server consent/token-generation/refresh/revoke domain; ports and orchestration; private-store and HTTP adapters |
| Existing credentials composition | Reads agent secrets; desktop writes agent credentials | Gateway-owned token-record writer through an explicit port; OS adapter capability reported honestly |
| Product protocol/client | Existing `mcpServers.*` and app methods | Remote shape and typed authorization/revoke status, generated from one schema |
| Desktop settings/host | Stored-server management and native link opening | Remote form, authorization/revoke actions and factual consent/connection status |

The SDK sees Nessa-owned HTTP request/response/stream values, not reqwest types.
Domain code owns token-generation and transition validity; effects stay behind
application ports. Each module gets its feature/layer map and corresponding tests.
Composition owns actual endpoint/network/storage adapters and their startup/drain.

### Published contracts to extend

The current #391 implementation is **stdio and name-addressed**. Its stored
shape is `{name, command, args?, enabled?, env?}`; remote identities are proposed
here, not already present. Extend
[`configured_server.rs`](../../../crates/nessa-server/src/mcp_servers/domain/configured_server.rs),
[`stored_servers.rs`](../../../crates/nessa-server/src/mcp_servers/infrastructure/stored_servers.rs)
and the existing `LiveServerSet` port together. Preserve stdio's absent-field
defaults, duplicate-env detection, kept-value semantics, base/per-server env
composition, managed `nessa` reservation and name-based save/remove/inspect.

The new stored variant is `{kind: "remote", id, name, url, enabled}`. The settings
owner mints `id` on creation and preserves it when an existing entry is saved or
renamed. Public remote save input is `{kind: "remote", name, url, enabled}` under
the current revision/`previousName` envelope; it cannot choose a new UUID for an
existing entry. Remote list entries publish `id`, definition revision, URL and
redacted authorization facts. Remote entries accept no executable, args, env, bearer or arbitrary header
fields. OAuth records are not part of `agents.mcpServers`. The stored remote
variant and public schema change together, without a second reader or version bump.

The SDK descriptor owns remote URL validation: HTTPS, or HTTP for an explicitly
configured loopback MCP resource; no userinfo or fragment. Gateway settings asks
that owner through the complete live-set validation, and frontend early refusal
consumes the resulting publication. Redirect handling belongs to the HTTP adapter
and consumes this URL policy; resource credentials do not follow an unvalidated
redirect. A model tool argument cannot select a URL, executable or credential item.
Stored UUID syntax and uniqueness join complete-set validation; a hand-edited
URL/id cannot bypass restored token resource/issuer binding or select a private
item outside the trusted namespace.

Extend [`protocol/product/v1.json`](../../../protocol/product/v1.json) and its
generator for remote variants, authorization commands and typed outcomes. Keep
`x-mcpServerRules`, `x-mcpServerInspect`, config/frame limits and
`McpServerLaunch::problem_in` as their current owners; introduce remote-only bounds
there or at their new HTTP/OAuth owner and publish them, rather than retyping
validators in settings, clients or UI. Reuse `INITIALIZE_TIMEOUT`,
`REQUEST_TIMEOUT`, `MAX_IN_FLIGHT` and `MAX_FRAME_BYTES` from SDK MCP; forwarded
requests still have no SDK-owned request timeout. HTTP envelope/SSE/metadata,
effect and callback limits are new constants settled by their implementing
slices, with tests at and beyond each bound. The callback window here is ten
minutes. None requires live-server credentials to choose or test.

Config publication retains current `applied`, durability and `liveSet`
replaced/withdrawn/kept meanings, including caller loss, shutdown and removal from
a hand-edited invalid list. Remote URL edits must serialize with authorization
fencing before the replacement live definition admits an opening. The settings
and authorization coordinators acknowledge that handoff; a failed handoff leaves
the new definition unavailable with a typed outcome, rather than a usable new URL
under the old token. Rename alone preserves the authorization binding.
Remote removal also fences the removed UUID and joins its authorization cleanup;
config `applied/live` and pending/incomplete secret cleanup remain distinct facts.
Disable keeps the saved UUID/consent and removes future openings through current
live-set publication; already-admitted sessions remain owned until their normal
close or explicit revoke. Cleanup must not read a removed definition by name:
its accepted operation retains the old identity/binding it needs.

## Identity and event contract

A configured server UUID is durable; rename does not rotate it. A definition
revision binds its transport/resource configuration, excluding display name
and enabled state; the whole stored-list revision still changes on those edits.
Discovered issuer/client/scopes bind the authorization record separately. Each authorization
attempt, refresh and revoke has its own identity. Token generation is monotonically
advanced under the per-server owner, and each remote session retains its own
local session/relay grant identity and negotiated upstream session/version.

Events name the server, definition revision, local session and owning attempt or
token generation as appropriate. A late callback or HTTP completion cannot
change a replacement definition, revoked generation or another conversation's
session. Presentation names and URLs are attributes, not operation identities.

Local transition selection is serialized by the owning session or per-server
coordinator. No global lock spans all servers. Effects carry their selected
identity/generation through the injected port and return as correlated events.
Slow HTTP, private-store and audit work runs outside the local decision. Their
supervisors retain accepted work and resource accounting after a caller leaves.

Closing/revoking seals future admission before effects. Results already observed
keep their original meaning; fencing cannot rewrite a successful request as never
sent. A stale generation cannot publish new tokens or admit another retry.
Duplicate callback state is rejected, duplicate close joins its owner, and
conflicting current-generation outcomes remain typed failures.

The owning implementation selects one local decision at a time, then supervises
its effects. An event accepted before close may establish its observed result;
close admitted first seals readiness/token publication. There is no implicit
priority based on Mermaid nesting or network arrival. Correlation checks precede
state changes; compatible region changes form one local decision and the enclosing
settle guard is rechecked after entry and each outcome.

| Event | Selection and observable outcome |
| --- | --- |
| Open/authorize | Validate current grant/manage authority, definition and capacity; refuse typed closed/stale/busy/store-unavailable before dispatch |
| HTTP/store/audit completion | Validate operation and epoch/generation; retain matching observation, then select publication/cleanup; stale completion cannot publish but its owned resources/candidate still join cleanup |
| Duplicate close/revoke | Join retained operation and first cause; do not re-enter state or create another unbounded effect series |
| Callback | Consume only matching pending state once; wrong/replayed/expired state is a typed refusal with no replacement of another attempt |
| Deadline/caller loss | Deadline fences the affected operation and retains uncertainty/cleanup obligations; caller loss detaches its waiter while the supervisor finishes accepted work |
| Conflicting same-operation evidence | Typed conflict blocks publication and retains both observations for reconciliation; it does not replace an acknowledged outcome |

Private-store results distinguish acknowledged, refused-before-write and
publication-unknown. Durable records retain resource/issuer/client binding,
generation, dispatch fence and unresolved operation facts. Deletion removes
secret material; non-secret fence/settlement evidence remains durable, so a
restart cannot infer authorization from an orphaned item. Credential item targets
are derived from the trusted runtime namespace and server UUID; adapter results
must match that target before publication. Store/audit ports acknowledge these
facts independently. Before exchanging a consumed code or sending a refresh,
persist its operation/generation intent; a restart with an unresolved dispatched
refresh does not repeat a potentially rotating exchange. Audit intent acknowledgement
precedes new consent/token effects, while local fencing/drain remains available
when audit fails. The OAuth slice tests interrupted writes and restart at each
effect/acknowledgement boundary, rather than assuming a network/store transaction.

Requests waiting for a valid token are bounded by existing request/capacity and
phase deadlines. The first OAuth slice refuses consent-required requests rather
than storing an unbounded offline queue. A refresh waiter waits on one current
refresh owner and may retry its original request once only when its recorded
outcome establishes authorization rejection; interrupted side-effecting requests
are not replayed from transport ambiguity.

## Connection statechart

This chart follows one local upstream MCP session. Token readiness belongs to
the separate authorization owner. `Negotiating` contains modern and legacy
transport selection; one outer close handles either branch.

```mermaid
stateDiagram-v2
    [*] --> Negotiating
    state Negotiating {
        [*] --> PostInitialize
        PostInitialize --> ModernReady: valid initialize response and negotiated version
        PostInitialize --> LegacyEndpoint: initial status 400, 404 or 405
        LegacyEndpoint --> LegacyReady: validated endpoint event and initialize response
    }
    Negotiating --> Open: protocol ready and owner still admits
    Negotiating --> Failed: refusal, malformed response or opening deadline
    Negotiating --> Closing: owner closes during initialization
    Open --> Closing: owner closes, session expires or connection becomes unusable
    state Closing {
        state LocalRelease {
            [*] --> LocalPending
            LocalPending --> LocalReleased: streams and accepted request owners drained
        }
        --
        state RemoteTermination {
            [*] --> TerminationPending
            TerminationPending --> TerminationObserved: DELETE outcome or no termination method
        }
    }
    Closing --> Closed: local drain settled and termination observation retained
    Failed --> [*]
    Closed --> [*]
```

The Failed outcome is delivered only after startup's local resources are released
or their unresolved cleanup is handed to the same close owner. TerminationObserved
is an observation, including refused/unconfirmed DELETE; it does not assert remote
process termination. Repeated close does not re-enter Closing or send an unbounded
series of DELETEs. No-session-id and legacy cases record not-applicable rather
than fabricate a successful DELETE.

Follow the [Streamable HTTP specification](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)
for request/response negotiation and headers. The implementation must publish the
versions it actually negotiates; reading this current specification does not
silently upgrade the local draft's protocol support.

Source review is pinned to MCP repository
[`8e12bf3c8f01f374146acfa19b8ece0018bfc86c`](https://github.com/modelcontextprotocol/modelcontextprotocol/tree/8e12bf3c8f01f374146acfa19b8ece0018bfc86c/docs/specification):
the 2025-11-25 transport/authorization pages and 2024-11-05 transport page.
The current SDK `wire.rs` publishes 2025-06-18, 2025-03-26 and 2024-11-05;
the transport slice consumes that publication and tests each advertised version.
Adding 2025-11-25 support is a deliberate owning change, not implied by these
references. Resumption/polling remains an explicit limitation of the first slice.

- Each JSON-RPC message is its own POST with required Accept/content headers.
  A notification/response acceptance is 202 with no payload; request replies
  accept JSON or SSE. Validate headers and correlation through typed parsing.
- Keep bounded event framing across chunks, UTF-8 and delimiter boundaries.
  Count before buffering past a bound. Skip valid SSE keepalive/priming events
  without interpreting them as JSON-RPC responses.
- Optional GET stream is separately owned. A 405 there means GET is unsupported,
  not that POST session initialization failed or needs legacy fallback.
- A stream can carry notices/requests before its matching response. Deliver
  each to the existing connection owner; do not reorder or misroute responses
  between a POST stream and an unrelated GET stream.
- An upstream session-bound 404 expires the old local session epoch and starts
  bounded fresh initialization without the expired id while its relay-grant owner
  still admits recovery, as the transport specification requires. Preserve each
  old request's observed result or uncertainty; initialization is not permission
  to replay a failed tool call. Retire old streams/handles and invalidate cached
  app/session resources before publishing the replacement session identity. The
  replacement is a new instance of this connection chart, coordinated by the
  same server-opening owner, rather than an Open transition on an expired id.
  If close/revoke/definition change wins, it fences startup; a late initialized
  replacement joins cleanup and cannot publish readiness. One recovery attempt
  uses the remaining operation/opening budget and retains failure if it cannot
  establish the replacement. A harness need not initiate another relay to trigger
  this recovery. Subsequent admitted calls resolve the current replacement;
  already-admitted calls against the retired epoch retain their own outcome.
- A disconnected stream does not itself cancel a remote request. Optional stream
  resumability is distinct from session reinitialization and command retry. The
  first slice reports an unconfirmed pending request and ends its local session;
  supporting server-directed polling/resumption requires correlated event ids,
  bounded retry intervals and further rows before claiming that capability.

DELETE/local drain share the owning close deadline, including fallback paths.
Expired deadlines retain the observation and release confirmed local resources;
remote uncertainty remains visible rather than reserving fictional remote-process
ownership. Mandatory audit has its own documented bounded delivery attempts.

### Owned POST event streams (#622)

`HttpSession` owns POST body/startup readers separately from pending JSON-RPC
calls. Its collection retains at most 256 handles (`MAX_POST_STREAMS`), including
aborted readers until reaped after completion. The semaphore is the sole capacity
authority: each retained reader holds its permit beside its task handle through
reaping or close joining. Initialization identity and GET/POST reader admission share one reader-registry
lock with close;
readers own the response and inbound sender, with no strong reference back to the session.
Before a request POST (including initialize), a capacity permit is reserved, or the call
is refused with `Busy` before exchange. Buffered JSON/202/failure releases that permit;
a streamed JSON or SSE response holds it through reader teardown. Notification and ping-reply
POSTs bypass request reservation so they can unblock active streams. If those
POSTs unexpectedly return a stream, they acquire available capacity or end with
`Unconfirmed` after dropping the excess body. This does not claim cancellation.

```mermaid
stateDiagram-v2
    [*] --> Reserved: pre-POST capacity permit
    Reserved --> Reading: response registered with JoinHandle
    Reserved --> Released: buffered JSON / 202 / exchange failure
    Reading --> Finished: matching terminal / EOF / body failure
    Reading --> Stopping: caller cancellation / session close (abort)
    Stopping --> Finished: task destruction completes
    Finished --> Released: is_finished reaping / close await joins
    Released --> [*]: retained permit dropped
```

The permit and registered task handle enforce these resource states; the ordering
table below names their regression tests. An unexpected notification/reply stream
acquires its permit when its body is admitted, joining Reading directly.

The client ends a request's POST stream after forwarding its matching result or
error. This is a client resource-lifetime policy: the specification recommends
server closure after the response, but trailing notices are not invalid protocol.
Notices and server requests before the matching response retain their order and
can elicit another POST. Events after the matching response are not consumed.
Caller cancellation stops an admitted reader directly, independently of delivery
of the best-effort upstream cancellation; a delivered queued cancellation also catches
reader admission after caller loss. When caller loss precedes response headers
and the outgoing queue cannot accept cancellation, a late reader remains bounded
and close-owned; this slice does not add a cancellation ledger. Completed handles
are reaped at admission.

Reader termination has one outcome policy: a matching response completes normally;
request EOF before that response publishes `Unconfirmed`; body/parser faults publish
their typed error; cancellation/close keep their existing cause; uncorrelated EOF
releases normally. Resource release alone does not settle a pending request. The
consuming loop drops its body before the outer reader publishes a failure, including
when the inbound channel is backpressured.

| Row | State/event/order | Result and ownership | Enforcer |
| --- | --- | --- | --- |
| P1 | Active reader receives notices/ping, then matching result/error; peer holds body | Forward preceding events and matching response, release body; repeated successful calls retain bounded readers | `post_result_releases_held_body`, `post_error_releases_held_body`, `post_ping_before_result_remains_live` |
| P2 | Active reader receives another id's response, same-id server request or nonterminal envelope | Forward it without ending this reader; retain until its own response/EOF/cancel/close | `post_other_response_does_not_retire_reader` |
| P3 | Request-correlated JSON/SSE body completes without its matching terminal (streamed or buffered), or body failure; uncorrelated body ends normally | Forward other valid evidence, then publish typed `Unconfirmed` for the unanswered call; release body and settle deadline-less calls. Uncorrelated EOF is normal. Private recovery refuses invalid initialize as `SessionExpired` (P14) | `post_eof_and_failure_settle_deadline_less_call`, `post_matched_response_before_eof_settles_normally`, `post_uncorrelated_eof_releases_normally`, `j2_complete_body_without_own_terminal_settles_deadline_less_call`, `j2_matching_json_result_and_error_remain_accepted`, `j2_neighbor_reply_is_preserved_before_unconfirmed_completion` |
| P4 | Caller disappears during active read | Abort local reader; best-effort remote notification remains separate | `post_caller_cancellation_releases_body`, `post_cancellation_with_full_outgoing_queue_releases_active_body`, `post_cancellation_during_headers_releases_late_body` |
| P5 | Close/drop while reader waits (including startup tools/list) or blocked inbound send; drop from an ordinary thread | Fence admission, abort and join owned readers before finished; repeated close shares completion | `post_close_joins_held_and_blocked_readers`, `post_finished_waits_for_reader_destruction`, `post_drop_releases_body`, `post_drop_outside_runtime_context_joins_body` |
| P6 | POST exchange returns streamed body after close drained readers | Reject late admission with `Closed`, drop body | `post_late_body_after_close_is_dropped` |
| P7 | Reader handles fill their independent bound; pending calls need not remain | Refuse next request before exchange; completed/aborted readers regain capacity | `post_reader_bound_refuses_before_exchange_and_recovers` |
| P8 | Notification/ping response unexpectedly returns SSE with full reader capacity | Drop excess body and end with `Unconfirmed`; bypass reservation for ordinary 202 replies | `post_notification_stream_at_capacity_is_unconfirmed` |
| P9 | Matching result/error is followed in the same HTTP chunk by invalid UTF-8 or an oversize event | Shared parser yields one event at a time; forward terminal and release body before interpreting trailing bytes | `post_terminal_precedes_bad_trailing_event` |
| P10 | Initialize result arrives after close; close races identity publication and GET registration | Close's registry fence refuses identity/GET admission with `Closed`; no leaked session-id claim or late GET exchange | `late_initialize_after_close_does_not_claim_or_start_get`, `late_legacy_get_after_close_releases_body` |
| P11 | One 404 recovery retires an active GET, then close begins before retirement completes | Aborted GET handle remains registry-owned until finished/reaped or joined by close; initial GET and the one recovery GET bound retained ownership on the public lifecycle | `retired_get_reader_remains_close_owned` |
| P12 | One initialize body repeats its matching result | First matching result owns phase identity/version and one GET; repeats leave that phase untouched | `repeated_initialize_result_owns_one_get` |
| P13 | Unrelated response precedes matching initialize result | Shared request-id/terminal matcher prevents unrelated data from owning negotiated identity/version; matching result establishes phase | `unrelated_response_does_not_own_initialize_phase` |
| P14 | Recovery JSON body contains only an unrelated response id | Refuse with `SessionExpired` before initialized notification, GET or new session-id claim; recovery requires admitted initialization evidence | `wrong_id_json_recovery_does_not_publish_readiness` |

```mermaid
sequenceDiagram
  participant Caller
  participant HttpSession
  participant Reader as Owned POST reader
  participant Peer
  Caller->>HttpSession: tools/call
  HttpSession->>Peer: POST (reader capacity available)
  Peer-->>HttpSession: SSE response
  HttpSession->>Reader: register handle under admission/close lock
  Peer-->>Reader: ping or notice
  Reader-->>Caller: inbound message
  Caller->>HttpSession: ping reply POST
  Peer-->>Reader: matching result, keep body open
  Reader-->>Caller: matching result
  Reader->>Reader: drop response body
  Caller->>HttpSession: close
  HttpSession->>Reader: abort remaining readers, join
  HttpSession->>Peer: best-effort DELETE if applicable
  HttpSession-->>Caller: finished after local joins and DELETE observation
```

Opening captures the owning Tokio runtime handle. Drop and shutdown use that
handle to run the local joins from an ordinary thread too; the owning runtime must
remain running through completion. Runtime shutdown is not fabricated as a
finished HTTP close observation.

The #622 slice established owned non-initialize POST SSE readers. The #623/#626/#634
extension below also owns streamed JSON and initialization/recovery startup;
body release is not confirmation of a remote tool stopping.

### Owned JSON and initialization progress (#623, #626, #634, #636)

The same bounded POST registry owns streamed JSON, streamed initialization and
replacement initialization. JSON consumption does not run on the frame writer.
Initialization accepts only its matching result, validated by `wire::initialized`,
and commits identity/version under the registry fence before GET or readiness.
An SSE reader retires at that terminal event rather than waiting for EOF.
Reader tasks keep weak session references only for synchronous commits. Two
review rounds found new cases in duplicated JSON/SSE message policy. The structural
repair is one per-message owner: typed body purpose, one correlation decision,
matching initialization publication, forwarding and terminal retirement. JSON/SSE
codecs only frame messages. Private recovery suppresses only its matching terminal;
valid neighboring replies remain visible before invalid recovery settles.

The private `Recovery` state is Available, Initializing, ReadyForWriter, Completed
or Failed with its first typed cause. One handoff check before dispatch and before
admission distinguishes actual close, retained failure, expired budget and unknown
owner loss. A lost completion channel cannot replace the writer's observed failure.
Identity publication does not change call admission. Recovery is
registered before its initialize exchange. While it is initializing,
ordinary calls are refused with `Busy`; cancellation and server replies remain
available. The registered startup owner performs the fresh initialize exchange;
ordinary frames and the initialized notification use the serialized writer.
A private `RecoveryReady` event uses that existing bounded writer queue to send
initialized; it creates no additional general frame sender. Recovery is bounded
by the injected clock's initialization budget, and never replays the failed call.
Close alone owns DELETE, retaining the ID claim through the attempt's completion.

```mermaid
stateDiagram-v2
    [*] --> Available
    Available --> Initializing: session-bound 404 / register startup before header I/O
    Initializing --> Initializing: early peer request / provisional reply binding only
    Initializing --> ReadyForWriter: matching validated initialize / publish identity
    Initializing --> Failed: startup failure or budget / retain first cause
    ReadyForWriter --> Completed: initialized accepted + valid predeadline delivery / atomic registry commit
    ReadyForWriter --> Failed: rejection, expiry or failed delivery / retain first cause
    Completed --> Completed: late startup timeout or failure / no-op
    Failed --> Failed: later failure / retain first cause
    note right of Completed
        Terminal recovery success; ordinary calls admitted.
        Waiter scheduling cannot reverse the commit.
    end note
    note right of Failed
        Terminal recovery failure; ordinary calls refused.
        The first typed cause remains authoritative.
    end note
```

Close is a separate registry fence, not another Recovery state. It prevents
publication and completion admission, joins registered owners, and owns DELETE;
a prior terminal recovery cause remains authoritative during teardown.

| Row | Trigger/order | Required outcome and ownership | Enforcer |
| --- | --- | --- | --- |
| J1 | JSON headers arrive, body stalls; call times out/drops | Writer can send cancellation and another call; direct cancellation aborts registered body; no replay | `j1_stalled_json_timeout_allows_cancel_and_next_call` |
| J2 | JSON/SSE completion without own terminal, valid terminal completion, body failure, oversize, or close | Forward neighboring evidence before typed Unconfirmed for an unanswered correlated call (P3); accept matching result/error. Bounded body and retained permit release only after destruction/reaping or joins | `j2_complete_streamed_json_settles_and_releases_body`, `j2_complete_body_without_own_terminal_settles_deadline_less_call`, `j2_matching_json_result_and_error_remain_accepted`, `j2_json_read_failure_and_bound_are_typed`, `j2_json_capacity_is_retained_through_physical_destruction` |
| J3 | Matching initialize SSE result; peer holds body | Commit validated identity/version before GET/readiness, retire body, initialized/list/ping answer dispatch before EOF | `j3_public_open_held_initialize_dispatches_initialized_list_and_ping` |
| J4 | Initialize has wrong id, request envelope, invalid version, repeated result or bad trailing bytes | Only first matching valid result establishes validated phase; invalid evidence does not start GET; terminal retirement ignores trailing bytes | `j4_invalid_initialize_version_does_not_claim_or_start_get`, `j4_initialize_terminal_precedes_bad_trailing_bytes`, P12/P13/P14 |
| J5 | Close wins before initialization publication | Refuse validated publication/admission, no new claim or GET after close; owned reader/startup is joined | `j5_close_held_initialize_body_has_no_late_publication`, `j5_drop_initial_reader_does_not_retain_session`, P10 |
| J6 | Initialization publication wins then close/caller loss | Close owns one DELETE and its completion; no independent cleanup attempt | `j6_recovery_publication_then_close_owns_one_retained_delete`, `j6_j7_close_owns_delete_and_retains_claim_through_completion` |
| J7 | DELETE held; duplicate close or a new opening repeats its ID | Finished waits; retained claim refuses collision and prevents DELETE targeting a new owner | `j7_public_open_collision_while_delete_is_held_is_not_deleted`, `j6_j7_close_owns_delete_and_retains_claim_through_completion` |
| J8 | Session 404; recovery header/body held | Failed call is SessionExpired without replay; recovery is owned before header I/O; ordinary requests Busy while controls remain available | `j8_recovery_owned_before_headers_and_controls_remain_available`, `j8_stalled_recovery_json_is_owned_and_bounded`, `j8_recovery_server_ping_reply_dispatches_before_initialize_result` |
| J9 | Recovery succeeds, fails, times out, or races close before initialized dispatch | Identity publication is separate from call admission. Gate queued calls before RecoveryReady and while initialized headers are held. One remaining initialization budget covers headers/body, queue handoff and initialized dispatch; writer revalidates its correlated handoff before dispatch and admission. Timeout/close/stale handoff/rejected initialized cannot release admission. Close joins startup and owns any claimed ID | `j9_recovery_gate_covers_queued_calls_and_initialized_headers`, `j9_expired_queued_handoff_cannot_dispatch_initialized`, `j9_saturated_queue_budget_expiry_retains_close_ownership`, `j9_timeout_while_initialized_headers_held_cannot_reopen`, `j9_rejected_initialized_does_not_release_admission`, J6 recovery-close test |
| J10 | JSON/SSE private recovery carries another pending call's reply, then ends without its own terminal | Forward the observed neighboring result before SessionExpired; no validated identity, GET or initialized; absent an early peer request, no replacement claim | `j10_recovery_neighbor_reply_precedes_invalid_initialize` |
| J11 | Budget expires after initialized acceptance but before admission; stale queued handoff or completion channel loss follows a failure | Retain typed Timeout or first non-timeout failure; actual close is Closed, unknown loss alone is Unconfirmed; no admission from stale handoff | `j11_accepted_initialized_at_expiry_reports_timeout`, `j11_completion_loss_preserves_writer_failure`, `j9_expired_queued_handoff_cannot_dispatch_initialized`, J6 actual-close test |

The completion channel and recovery phase share one terminal owner. The writer
hands its owned completion sender to `finish_recovery`; under the registry fence,
validated initialized acceptance, successful synchronous completion delivery and
Completed publication are one transition. Failed delivery fences calls before the
writer handles another frame. Completed is terminal: a startup waiter selecting
its budget after that commit cannot replace success or publish a late timeout.
Typed HTTP response failures have one classifier shared with ordinary POSTs.

| Row | Trigger/order | Required outcome and ownership | Enforcer |
| --- | --- | --- | --- |
| J12 | Completion receiver disappears after initialized acceptance, before completion commit | Failed delivery becomes terminal before any queued ordinary frame; no temporary Completed/admission gap | `j12_lost_completion_cannot_admit_queued_call` |
| J13 | Successful predeadline completion delivery wins, then startup observes its ready timer; or timeout wins before delivery | Timer selection after predeadline delivery/commit leaves Completed successful with no late Timeout; timer-first Failed Timeout blocks delivery/admission | `j13_completed_commit_defeats_late_timeout`, J9 expired handoff |
| J14 | Recovery initialized POST receives 5xx, refreshed retry receives 401, or claimed replacement returns 404 | Preserve Unconfirmed, Unauthorized or SessionExpired through writer, startup and public pending calls; claimed 404 terminates without a second recovery or replay (stateless 404 is Malformed) | `j14_initialized_http_failures_keep_typed_causes` |

Response classification owns immutable evidence of the actual POST attempt. The
same branch that emits `Mcp-Session-Id` records SessionBound; omission records
Stateless, while initialize retains its method purpose and captured initial-only
legacy eligibility. Each refreshed retry captures its own emitted headers. The
response travels with this context; neither consumer consults a later phase to
infer which request produced it.

| Row | Trigger/order | Required outcome and ownership | Enforcer |
| --- | --- | --- | --- |
| J15 | Stateless control waits on headers; private initialize publishes replacement; control returns 404 | Classify actual stateless attempt as Malformed FailCall(None), preserve queued Ready and successful subsequent replacement call; no second initialize/replay | `j15_control_response_uses_its_actual_request_context` |
| J16 | Stateless control returns 401 after replacement publication; refreshed retry emits replacement ID and returns 404 | Classify final bound attempt as SessionExpired, end without second recovery/replay; capture context separately for each real attempt | `j16_refreshed_control_retry_uses_its_own_bound_context` |

Early peer replies carry immutable response binding through Connection's framing
queue. A provisional claim permits that answer only; validation remains the owner
of version, GET and ordinary admission. A captured binding is checked on each real
POST attempt, including authorization retry, and cannot adopt a replacement.
Private initialize and ordinary POST share authorization/status policy; private
initialize has no legacy eligibility. A bound peer-answer 404 ends SessionExpired; a stateless 404 is Malformed.
Neither starts recovery or replay; ordinary-call 404 retains its existing single recovery.

```mermaid
stateDiagram-v2
    [*] --> Absent
    Absent --> Provisional: early peer request / bounded claim
    Absent --> Validated: matching valid initialize / bounded claim
    Provisional --> Validated: matching valid initialize / promote same binding
    Absent --> Closed: close fence
    Provisional --> Closed: close fence / retain claim through DELETE
    Validated --> Closed: close fence / retain claim through DELETE
```

| Row | Trigger/order | Required outcome and ownership | Enforcer |
| --- | --- | --- | --- |
| J17 | Initial/replacement SSE request precedes matching result; peer waits for answer | Answer carries originating ID, without negotiated version. Provisional claim starts no GET/initialized/admission; valid result promotes same binding. No-ID response remains stateless | `j17_initial_peer_reply_keeps_response_binding_before_validation`, `j17_public_open_waits_for_bound_peer_answer_before_validation`, `j8_recovery_server_ping_reply_dispatches_before_initialize_result`, `j17_ordinary_body_reply_keeps_request_binding_and_ignores_returned_identity`, `j17_ordinary_terminal_ignores_non_authoritative_oversized_header` |
| J18 | Provisional ID collides, exceeds 1024 bytes, or close wins | Typed refusal before answer effect/publication. Collision loser DELETE=0; owning close retains provisional claim through sole DELETE | `j18_early_peer_claim_refuses_collision_and_oversize_before_answer`, `j18_provisional_claim_is_retained_through_close_delete`, `j18_public_open_error_closes_provisional_binding` |
| J19 | Reply queues before replacement, timeout, failure or close; peer answer returns 404 | Stale answer cannot acquire current identity. Refuse before dispatch after close/failure; a stale preterminal request ending the connection fences private startup against late replacement publication/admission. Every overlapping shutdown caller raises the HTTP admission fence before returning, even if another caller already claimed cleanup. An ordinary neighboring terminal preserves healthy recovery; bound peer-answer404 ends SessionExpired (stateless404 is Malformed), without recovery/replay. Ordinary-call404 positive control recovers | `j19_queued_reply_keeps_unnegotiated_version_and_is_refused_after_close`, `j19_old_reply_cannot_adopt_reused_or_stateless_replacement_binding`, `j19_peer_answer_404_ends_without_recovery_or_replay`, `j19_close_during_reply_authorization_refuses_post_effect`, `j19_reply_retry_cannot_adopt_identity_published_during_authorization`, `j19_preterminal_stale_request_cannot_revive_ended_connection`, `j19_preterminal_ordinary_terminal_preserves_recovery_control`, `j19_reader_failure_fences_http_owner_before_ended`, `j19_duplicate_shutdown_fences_effects_after_cleanup_claim`, `repeated_shutdown_keeps_one_delete_and_releases_its_claim`; ordinary recovery control in J20 |
| J20 | Private initialize receives 401/retry, 403, 5xx, 400/404/405, 202 or exchange failure | One rejected retry maximum; observe exact scope challenge; Unauthorized/InsufficientScope/Unreachable/Malformed remain typed. 202/missing terminal is SessionExpired. No legacy GET, replacement GET/initialized or replay on failure; healthy and retry-success controls remain accepted | `j20_private_initialize_preserves_shared_authorization_and_status_policy`, `j20_private_initialize_healthy_and_retry_success_controls`, `j20_private_initialize_exchange_failure_is_unreachable`, J10 missing-terminal controls |

### Bounded peer-reply admission (#665)

The existing FIFO writer reserves one physical slot against ordinary frame
pressure. Its queue owner limits ordinary frames (including best-effort remote
cancellation notifications) to 64 and derives HTTP capacity as 64 + 1. Stdio
retains 64 slots without a reserve. A queued ordinary frame owns a permit until
it is dequeued; dropping a waiting send or the receiver releases its ownership.
`PeerReply` and `RecoveryReady` consume no ordinary permit. Controls may occupy
otherwise free ordinary slots, but the physical queue remains bounded at 65.
This provides admission against ordinary saturation, without control priority or
unlimited progress. Root accepted this single-FIFO design with Astra advisory;
a separate priority lane would add scheduling and ordering decisions.

Peer replies never await queue capacity in the reader. A full HTTP control
admission ends with typed `TooLarge("queued MCP control frames")`; a closed
writer ends with its retained terminal cause or `ServerGone`. The existing
reader shutdown fence and connection end owner retain the first cause and one
cleanup. A panicked HTTP writer does not wait for that later read: its watch
asks the same owner, after the same fence (J27). `RecoveryReady` still awaits
FIFO admission and completion under the remaining initialization budget.
Direct local cancellation remains independent of its best-effort remote
notification. The HTTP owner remains the authority
for captured binding, recovery admission, deadlines and close.

| Row | Trigger/order | Required outcome and ownership | Enforcer |
| --- | --- | --- | --- |
| J21 | 64 ordinary frames wait behind a held writer; early recovery ping or unsupported request arrives before matching initialize | Reserve admits the answer without blocking the reader. Drain pressure; ordinary calls remain Busy and answer uses originating SID and unnegotiated version. Matching initialization supplied after actual answer advances | `j21_saturated_ordinary_queue_preserves_early_recovery_answers` |
| J22 | Matching initialize arrives while saturated ordinary frames and early answer remain queued | Captured answer retains unnegotiated version; FIFO answer precedes RecoveryReady/initialized. Ordinary admission remains fenced until valid initialized completion | `j22_matching_initialize_preserves_queued_reply_order` |
| J23 | Physical queue fills with ordinary frames and one answer or entirely with controls; a live call/notification waits for ordinary or physical admission, then another peer request overflows, or writer is closed | Explicit typed overflow/closed termination through existing reader fence; first cause survives later close/failure, no queued answer effect after fence, one retained DELETE and joined startup. Connection admission observes existing Shared end publication so blocked calls/notifications resolve retained cause before the held writer drains; admission subscribes before its retained-State snapshot, and a recorded end returns that cause even while watch publication is paused. Watch publishes after releasing State and before pending response wakes. HTTP shutdown remains the downstream effect fence | `j23_control_overflow_retains_first_cause_and_one_cleanup`, `j23_terminal_admission_wakes_physical_waiters_and_refuses_ended_notifications`, `connection::end_tests::recorded_end_refuses_admission_while_watch_publication_is_held`, `j23_closed_control_queue_is_explicit` |
| J24 | Ordinary sender waits for capacity and is dropped; dequeue retains returned frame; receiver is dropped | No permit leak; dequeue releases ordinary capacity before dispatch completion; later frames progress; closed receiver rejects remaining sends. Canceling terminal Connection admission drops pending send permits/envelopes without inventing a second terminal owner | `j24_ordinary_capacity_releases_on_dequeue_and_cancellation`, `j23_terminal_admission_wakes_physical_waiters_and_refuses_ended_notifications` |
| J25 | Recovery budget expires while early answer or handoff waits behind writer pressure | Timeout remains authoritative; queued reply/initialized cannot dispatch or reopen admission after expiry; provisional claim remains close-owned | `j25_deadline_refuses_queued_saturated_reply`, existing J9/J11 handoff expiry tests |
| J26 | Explicit close or control POST failure wins before queued saturated answer dispatch | No downstream answer/initialized effect; explicit HTTP close raises its existing shutdown fence before committing the first Shared end cause, then publishes watch outside State before waking responses. Later competing causes cannot replace the first commit. Admission checked open before that commit may race queue insertion, but HTTP effects remain fenced | `j26_close_or_writer_failure_refuses_queued_saturated_reply`, `j26_close_fences_http_and_publishes_before_response_wakes`, existing J19 close-fence tests |
| J27 | A custom HTTP exchange panics the writer, or that task is aborted, while a call and recovery are in flight. No later inbound message is required. An explicit close may commit before the panic or after it | The watch is not an end owner. On panic it raises the existing HTTP shutdown fence, which refuses new posts, joins body readers, and DELETEs a claimed id once, then asks `Shared.end` for `ServerGone`. Pending calls wake with that cause. A later close cannot replace it. A close that already committed stays `Closed`. Joining a cancellation does not fence the session or record a cause. Recovery that has released its previous id and not claimed the next does not DELETE | `j27_writer_panic_ends_without_later_input`, `j27_cancelled_writer_does_not_record_an_end` |

J4/J5/J10's no-claim requirements describe bodies without an early claim for a peer answer.
An already answered request may own provisional cleanup but cannot validate
initialization. J8 previously checked dispatch only, and J14 covers the later
initialized notification; neither established J17/J20's header/status guarantees.

```mermaid
sequenceDiagram
    participant Writer
    participant Registry
    participant Body as Owned body/startup task
    participant Peer
    Writer->>Peer: POST
    Peer-->>Writer: headers and body
    Writer->>Registry: register body with capacity permit
    Registry->>Body: consume JSON/SSE off writer
    Body->>Registry: commit matching validated initialize under close fence
    Body-->>Writer: RecoveryReady (replacement only, existing queue)
    Writer->>Peer: notifications/initialized
    Note over Registry: close fences admission, joins tasks, owns DELETE
    Registry->>Peer: DELETE claimed ID once
    Note over Registry: release claim after DELETE observation and finish after joins
```

## Audit verification by recorded meaning (#631)

Gateway inspection verification selects requested/outcome records by action and
phase, then checks their shared operation identity. Millisecond wall-clock
observations do not order files. This is a test correction in
`crates/nessa-server/tests/composition/mcp_servers.rs`; it changes neither audit
publication nor transport behavior.

| Row | Ordering/input | Required verification | Enforcer |
| --- | --- | --- | --- |
| V1 | Files shuffled; observations equal or moving backward; save and inspection records interleaved | Select the inspection's requested/outcome by recorded semantics and confirm one shared operation ID; no positional or timestamp ordering assumption | `audit_inspection_pair_does_not_depend_on_file_or_timestamp_order` |
| V2 | Shutdown cuts an inspection blocked mid-read | All four audit records exist before stop returns; the inspection pair retains caller-requested intent, gateway-stopping outcome and the same operation ID | `shutdown_cuts_an_inspection_blocked_mid_read_and_records_it_before_the_stop` |
| V3 | Desired action/phase is missing, or two matching outcomes name different operation IDs | Reject missing or ambiguous evidence rather than return an unrelated transition | `audit_selection_rejects_a_missing_phase`, `audit_selection_rejects_ambiguous_operation_ids` |

## Authorization statechart

One configured server owns consent and a reusable token record across its sessions.
Its Ready state has separately evolving token availability and refresh activity.
Refresh is not another independent authorization grant.

```mermaid
stateDiagram-v2
    [*] --> ConsentNeeded
    ConsentNeeded --> Discovering: authorize [current manage authority and store available]
    Discovering --> PendingConsent: metadata and registration accepted / return consent URL
    Discovering --> ConsentNeeded: unsupported registration or discovery failure
    PendingConsent --> Exchanging: matching single-use callback before deadline
    PendingConsent --> ConsentNeeded: abandoned, expired, denied or replaced attempt
    Exchanging --> Ready: token record stored and required evidence acknowledged
    Exchanging --> ConsentNeeded: definite exchange refusal with no candidate
    Exchanging --> AuthorizationIncomplete: uncertain exchange, storage or evidence publication
    state Ready {
        state TokenAvailability {
            [*] --> TokenUsable
            TokenUsable --> TokenUnavailable: expiry or authorization rejection
            TokenUnavailable --> TokenUsable: current-generation replacement published
        }
        --
        state RefreshActivity {
            [*] --> RefreshIdle
            RefreshIdle --> Refreshing: current generation needs refresh
            Refreshing --> RefreshIdle: correlated refresh outcome retained
        }
    }
    Ready --> Revoking: invalid grant or changed resource or issuer / fence old binding
    Ready --> AuthorizationIncomplete: refresh dispatch or candidate publication unresolved
    Ready --> Revoking: revoke or remove / fence generations and new session admission
    Discovering --> Revoking: revoke, change or remove / abandon current attempt
    PendingConsent --> Revoking: revoke, change or remove / invalidate callback state
    Exchanging --> Revoking: revoke, change or remove / deny late token publication
    AuthorizationIncomplete --> Revoking: authorized reconciliation / join retained obligations
    Revoking --> ConsentNeeded: settle [local drain and deletion settled and remote observation retained and required evidence acknowledged]
    Revoking --> RevocationIncomplete: failed or deadline [required local or evidence obligation unresolved]
    RevocationIncomplete --> Revoking: authorized retry / join retained cleanup obligations
```

The per-server domain synchronizes the Ready regions. TokenUnavailable blocks new
token-backed requests while one refresh owner works. A failure known not to have
dispatched a refresh can leave the old token usable only before its known expiry
and without known rejection. A lost refresh reply may have rotated its secret;
it enters AuthorizationIncomplete instead of repeating that exchange. A successful
response alone does not enter TokenUsable: validate it,
write the record and acknowledge required evidence before publishing the new
generation. If publication fails, retain the candidate/recovery obligation and
block use instead of forgetting a refresh token that may have rotated.

AuthorizationIncomplete fences dispatch and owns the candidate or unknown remote
grant plus pending store/evidence obligations. Explicit reconciliation enters
Revoking; no fresh consent is admitted until those obligations settle. This is
distinct from a definite refusal with no candidate and from incomplete revoke.
The accepted operation also authorizes necessary cleanup after caller loss or
restart; reconciliation does not wait for a window to return. Definition change
or removal fences active consent as well as Ready, invalidates callback
state and joins cleanup under this same owner. Fresh attempts use the replacement
definition only after the handoff acknowledges its fence.

Gateway shutdown separately seals process/session admission, abandons pending
browser attempts and drains accepted HTTP/store/audit effects. It preserves
settled reusable authorization rather than treating every restart as revoke.
An outcome acknowledged during shutdown may settle its durable record but cannot
publish a live session. Unresolved dispatched exchange/refresh and cleanup facts
remain fenced on restart; a durable Ready record is revalidated before use.

A server needing no authorization can connect without entering this chart.
Availability/status reads report current facts; they do not trigger browser
consent. The [MCP authorization specification](https://modelcontextprotocol.io/specification/2025-11-25/basic/authorization)
sets discovery, resource, PKCE and token-handling requirements. The first slice
supports dynamic registration where offered and honestly refuses other client
registration mechanisms; it does not claim universal server compatibility.

### Consent and callback

The discovery and callback boundary corrections in #624 and #629 follow these
orderings. The domain remains the owner of callback acceptance; the listener
delivers bounded candidates and does not keep a copy of the expected state.

| Row | Input or ordering | Required result |
| --- | --- | --- |
| A2a | Root, nested, trailing-slash, encoded-path or IPv6 issuer | Insert the OAuth well-known prefix before the issuer path; append the OIDC suffix after it; remove terminating slashes only for candidate construction |
| A2b | Issuer query, fragment, userinfo, malformed URL or non-HTTPS scheme | Refuse before authorization-server requests; endpoint queries remain permitted |
| A2c | OAuth metadata unavailable versus successful malformed or insecure metadata | Non-200 or definite non-dispatch permits OIDC fallback; lost response or invalid successful metadata stops discovery |
| A2d | Metadata issuer differs from the original issuer, including trailing slash | Reject exact binding before registration or token exchange |
| A3a | Idle first socket, valid second socket; fragmented request line/header | Read complete CRLF-framed headers concurrently within four connection slots, 4096 bytes each, and a two-second absolute connection budget; no partial candidate |
| A3b | EOF, malformed framing/query, unrelated route, oversize or duplicate recognized query field | Release that connection without consuming consent; a later valid callback remains eligible |
| A3c | Wrong-state candidate while the returned domain decision remains PendingConsent | Continue the bounded candidate stream; preserve the attempt and perform no exchange |
| A3d | Accepted, denied, expired or stale candidate in a terminal domain phase | Drop the receiver at the admission decision, before persistence/audit/exchange effects; accepted exchange work remains owned by the application |
| A3e | Concurrent valid candidates, queued candidate during revoke/resource change | Serial domain admission consumes at most one current state; terminal decisions discard queued candidates |
| A3f | Full candidate channel, connection saturation, slow trickle, receiver drop or whole deadline | Bound active readers and queued candidates; connection deadlines do not reset; receiver closure or the original whole-attempt deadline closes the listener and its scoped readers |
| A3g | Fixed response write fails or stalls after valid framing | Bound the write, retain the valid candidate, and keep codes/state/request targets out of the response |
| A3h | Real wrong-state then valid TCP callback while token exchange remains gated | Observe successful completion of the listener's owning task before releasing exchange; task completion drops the listener and scoped connections, without relying on platform-specific refused-connect timing |

The candidate channel has capacity one. Its four scoped connection futures
include candidates waiting for channel capacity, after their sockets close.
Framing allocates at most four 4096-byte buffers; decoded candidate values are
bounded by their originating headers, including spare allocation capacity.
Receiver closure cancels the scoped futures rather than detaching socket tasks.
There is no separate empty-stream revoke signal in this correction: a worker
waiting without a candidate remains bounded by the original whole-attempt
deadline. A subsequent candidate observes the authoritative domain phase.

Record authorization intent before discovery/registration effects. Bind state,
PKCE verifier, resource, issuer, client registration, definition revision and a
10-minute injected deadline to one attempt. State is unpredictable and consumed
once under that attempt owner, before exchanging a code. Wrong/unknown/replayed
state is rejected without replacing another pending attempt. Callback pages do
not echo codes, credentials or account details.

Discovery parses Bearer challenges, follows `resource_metadata` when present,
otherwise uses the path-specific/root protected-resource metadata order. It tries
the specified OAuth metadata/OIDC endpoints for each selected issuer and verifies
exact issuer/resource binding and advertised PKCE S256 before registration. All
authorization-server endpoints use HTTPS; the loopback callback uses its exact
registered URI. A cloud authorization-server fake therefore uses a test TLS
listener/trust adapter, rather than making production OAuth accept HTTP.

Bind requested/granted scopes to the attempt and token record. The current
challenge's scope set is authoritative; it need not be a subset of
`scopes_supported`. Without a challenge scope, use protected-resource
`scopes_supported`, or omit scope when absent. A 403 `insufficient_scope` returns
typed scope-required facts for explicit renewed consent. It does not refresh/replay
the tool automatically or open a browser from status/list. This is the first
slice's deliberate handling of step-up authorization. Token audience validation
is the resource server's duty; the gateway binds its opaque token to discovered
resource/issuer and does not claim to inspect an opaque token's audience.

A replaced attempt invalidates its old callback and records abandonment. Changed
redirect port/URI requires supported re-registration during explicit authorize,
not silent registration on refresh. Gateway restart abandons unsaved browser
attempts; a callback cannot recreate authority from its query string.

Validate resource and issuer bindings and the allowed URL scheme/origin policy at
one owning boundary. Resource bearer tokens are not sent to discovery/registration
endpoints or forwarded across arbitrary redirects. The legacy endpoint event must
resolve to the configured MCP origin for this first slice. OAuth authorization
servers may differ from the resource origin, but their metadata/endpoint binding
is established through discovery, not by accepting arbitrary model URLs.

### Refresh and revoke ordering

Concurrent callers at generation n join one refresh. Capture generation before
dispatch; if another caller published n+1 meanwhile, reuse that known replacement
instead of starting a second refresh. Correlated 401s can retry once under n+1;
a lost POST answer cannot establish that retry is safe. invalid_grant retires use
and closes affected sessions through their common owner.

```mermaid
sequenceDiagram
    participant C as Conversations A and B
    participant O as Server authorization owner
    participant S as Private token store and audit
    participant R as Authorization server
    C->>O: authorization rejection under generation n
    O->>S: record refresh intent
    O->>R: one refresh for n
    R-->>O: candidate replacement or typed failure
    alt same generation and definition still admit publication
        O->>S: store replacement and record required outcome
        O-->>C: publish n+1, each eligible rejected call may retry once
    else revoke or definition change won
        O->>O: retain rejected late outcome, do not publish or retry
    end
```

Revoke first seals token/session admission and invalidates browser attempts. Local
sessions drain regardless of remote revocation availability. Attempt advertised
RFC 7009 revocation with a bounded result, delete private credential material
while retaining the durable fence, and record actual outcomes. A 2xx response
is an observed remote acknowledgement; absent
endpoint/network failure is unconfirmed. Both leave local token use fenced.

A refresh completion racing revoke must not rewrite a deleted credential item.
Per-server effect publication and store deletion serialize their ownership:
retain the generation compare through candidate publication, without holding an
admission mutex across network I/O. A pending write that cannot be cancelled
remains owned and must be joined/reconciled before deleting its record. Failure
of deletion is RevocationIncomplete and does not re-enable the retained token.
Late exchanged/refreshed candidates join this cleanup, including an advertised
revocation attempt where available; discarded local bytes do not prove their
remote grant ended. An unknown exchange/refresh response retains remote
uncertainty rather than inventing a token to revoke.
Stored generation/fence evidence governs restart; token presence alone is not
proof of usable authorization. While effects/observations remain pending, stay
Revoking. Enter ConsentNeeded only after confirmed local drain, settled private
deletion, retained remote revocation observation and required audit acknowledgement.
The remote observation can be acknowledged, unsupported or unconfirmed; keep that
qualified outcome visible. If bounded settlement fails with a local/evidence
obligation unresolved, enter RevocationIncomplete. Successful deletion with failed
outcome audit therefore has only the incomplete transition enabled.

## Configuration and app integration

Extend the current #391 stored-server owner to mint/preserve remote
UUIDs. It already serializes config edits at the current revision and validates
the complete live list through the SDK publication; the extension exposes
redacted remote status. Env
values and tokens are not returned in a list response. Changing a resource URL
under an existing id fences its old token/attempt generation and requires new
consent; renaming alone preserves authorization.

Existing conversations keep their per-open stand-ins. New openings use the
current live set, consistent with the existing restoration-identity rule. A stale
stand-in digest is refused by the relay. Extend the existing digest owner with
remote identity/definition data rather than introducing another check in settings.

The app bridge remains scoped to that conversation's own upstream session.
App-origin visibility, destructive-tool review, tickets and release use current
owners. OAuth readiness does not grant an app permission to invoke hidden tools.
Server-level CSP consent binds to the inspected declared domain-set digest;
changed declarations require renewed consent before activation. Desktop reads
inspect/list outcomes and shows unauthorized, consent needed, token expired,
refresh failing, unreachable and incomplete revoke as distinguishable facts.

The generated product API adds `mcpServers.authorize` and `mcpServers.revoke`,
addressed by remote UUID and current definition revision under existing server
management authority. Authorization returns an operation/attempt identity and
pending-consent URL/deadline, already-ready facts, or typed refusal. Revoke returns
its operation and separate local drain, secret deletion, remote observation and
evidence facts; pending/incomplete does not become success because a caller left.
`list` reports redacted authorization facts independently from connection facts;
`inspect` uses an already-ready token or returns consent/scope/store-required facts,
without initiating consent. SDK HTTP outcomes distinguish authorization rejection,
scope requirement, session expiry, definite dispatch failure and unknown request
result; adapters never derive these by parsing error strings. Product/client
mapping preserves those distinctions, and generates its types from the schema.

## Regression tables and fixture contract

Rows are proposed runtime acceptance cases. Checked-in protocol/server fixtures
exercise boundaries today; they are not proof that the proposed gateway adapter,
OAuth storage or desktop controls already satisfy these rows.

### Connection

| Row | Ordering or input | Required result |
| --- | --- | --- |
| C1 | Initialize JSON or SSE, with/without session id | Own one local session; retain negotiated id/version and apply required subsequent headers |
| C2 | Initial POST 400/404/405 versus 401/5xx/malformed | Only the first group enters legacy endpoint discovery; typed refusal otherwise |
| C3 | Optional modern GET returns 405 | Continue supported POST operations without legacy fallback |
| C4 | Notices/requests precede response, split chunks or keepalive events | Preserve bounded framing, order and request correlation |
| C5 | Session 404 with calls in flight; recovery races owner close/revoke | Fence expired epoch, preserve old results/uncertainty and start bounded fresh initialize without old id; late recovery joins close; no tool replay |
| C6 | Stream disconnect before or after reply; optional polling advertised | Do not infer cancellation; retain an observed reply. Request POST EOF before its matching reply publishes `Unconfirmed` immediately; uncorrelated EOF releases normally. Report unsupported resumption accurately |
| C7 | Two conversations use same server; one closes; second initialize repeats an owned session id | Separate local ids/grants and cleanup; survivor remains usable; reject upstream id collision without DELETE of the survivor |
| C8 | Close during initialize, caller loss or late HTTP result | Fence dispatch, drain owned startup and accepted requests; ignore late readiness for current admission |
| C9 | DELETE acknowledged, refused 405, fails or times out | Local drain still runs; remote observation remains distinct from physical confirmation |
| C10 | Oversize/malformed UTF-8/event/header/frame | Reject before buffering past bounds; release local stream/request ownership |
| C11 | Same close twice, server definition edited or grant revoked | Join close, preserve first cause and refuse stale openings |

### Authorization

| Row | Ordering or input | Required result |
| --- | --- | --- |
| A1 | No auth required or no token writer | Unauthenticated server works; needed OAuth returns explicit store-unavailable refusal |
| A2 | Valid challenge/well-known/OIDC discovery, S256 and scopes versus unsupported/changed binding or 403 insufficient_scope | Bind metadata/resource/issuer/scopes or refuse; explicit scope-required result, no silent credential forwarding or automatic step-up replay |
| A3 | Correct, wrong, duplicate, late or replaced callback | One current state consumed; retain denial/abandonment and reject stale callback |
| A4 | Exchange reply lost or succeeds, token-store or outcome-audit fails; restart at effect/ack boundary | AuthorizationIncomplete fences use, retains candidate/remote uncertainty and publication obligations; definite refusal with no candidate can return ConsentNeeded |
| A5 | Concurrent 401s at n, refresh already published n+1 | One refresh owner; eligible rejected requests retry once with n+1 |
| A6 | Refresh not dispatched before/after expiry, response lost after rotation, or invalid_grant | Retain old generation only while known usable on definite non-dispatch; lost reply fences as AuthorizationIncomplete without exchange replay; invalid grant requires consent after session closure |
| A7 | Refresh/revoke/definition change/callback simultaneously ready | Generation fence prevents late publication, store resurrection and new requests |
| A8 | Remote revoke unsupported/fails; private deletion fails | Local token use remains fenced; actual remote/local outcomes are retained |
| A9 | Audit unavailable, UI gone or response lost; deletion succeeds but outcome audit fails | Required cleanup continues; failed bounded settlement remains RevocationIncomplete; no success without required evidence; caller loss does not abandon owner |
| A10 | Restart with usable, closing or incomplete token record | Revalidate binding; resume cleanup/fenced state; presence of a token does not grant dispatch |

Issue [#687](https://github.com/nessalabs/nessa-agent/issues/687) specifies the
successful-refresh token retention cases within A5–A7. The application owner
publishes the replacement credential; storage does not merge generations. Carry
the token used by that refresh into publication, rather than reloading a possibly
replaced secret after the HTTP response. RFC 6749 section 6 permits omission of a
replacement refresh token.

| Refresh reply / ordering | Required result | Regression in `mcp_authorization::tests` |
| --- | --- | --- |
| Expiry refresh succeeds without `refresh_token` | Retain the dispatched token through another expiry and owner restoration | `a_refresh_without_a_new_refresh_token_preserves_the_old_one` |
| Expiry refresh supplies a new `refresh_token` | Persist the replacement and use it at the next expiry after restoration | `a_refresh_with_a_new_refresh_token_uses_the_rotated_one` |
| Rejected bearer refresh succeeds without `refresh_token` | Retain the dispatched token for the next expiry | `a_rejected_bearer_refresh_without_a_new_refresh_token_preserves_the_old_one` |
| Refresh publication is overtaken by revoke | Preserve A7's fence and delete the late candidate | `a_token_published_after_revoke_is_deleted` |

### Apps and current configuration

| Row | Ordering or input | Required result |
| --- | --- | --- |
| U1 | Remote save/edit/disable/remove with current/stale revision; removed id has pending OAuth work | Config owner preserves current publication/refusal/env meanings; disable preserves consent; removal fences old UUID and joins retained cleanup without confusing config publication with deletion success |
| U2 | Rename versus resource URL change | Preserve durable id; authorization stays for rename and fences/reconsents for resource change |
| U3 | Two conversations mount same server app | Each uses its own upstream session and existing visibility/review/resource policy |
| U4 | CSP declaration changes, denied consent or stale inspection | Activation uses acknowledged current domain-set digest; honest consent/refusal state |
| U5 | Authorize/revoke/mount close under narrow layouts | Reachable controls and factual pending/failure outcomes in Chromium and WebKit |

Use a dependency-free local HTTP MCP fixture for cloud transport replay.
Sanitized captured ACP frames remain separate provider-boundary evidence, not
remote MCP transport proof. Record versions, scenario/prompt, capture method, normalized
identities and exact source provenance. Keep synthetic error cases explicitly
synthetic. Never commit complete recordings, account status, real tokens or local
credential paths. Fixture tests must assert actual protocol/correlation/cleanup
behavior and valid neighboring cases; a JSON file that merely repeats this table
is not regression evidence.

The planning PR's current cloud entry points are documented in
[`scripts/mcp-test-server/README.md`](../../../scripts/mcp-test-server/README.md):

```sh
node --test scripts/mcp-test-server/http-server.test.mjs
node --test scripts/subagent-contracts/*.test.mjs
```

The developer HTTP fixture imports the current stdio server's tool/resource
answers, while its transport tests use real loopback sockets. It does not install
remote transport into the product. Sanitized ACP evidence is owned by
[`scripts/subagent-contracts/`](../../../scripts/subagent-contracts/README.md),
with capture provenance and a separate credential-free verifier. This distinguishes
live provider boundary evidence from local deterministic server scenarios.

OAuth needs a separate deterministic authorization-server/private-store fixture
in its implementing slice: rejected consent, replayed state, short expiry,
rotating refresh tokens, invalid_grant, slow/failed storage/audit and unavailable
registration/revocation. Manual live verification then exercises one consenting
real remote server with UI; cloud correctness checks use local substitutes and
sanitized recordings without the owner's credentials.

| Existing fixture evidence | Useful rows and required product regressions |
| --- | --- |
| `http-frames.json`, JSON/legacy replay | C1/C2 payload/correlation seed; run the SDK HTTP adapter against the listener, adding negotiated-version/header checks and modern POST SSE replies |
| Actual two-session DELETE/disconnect tests | C7/C9 seed; prove gateway grant/conversation isolation, duplicate close, late startup and owner drain in C7–C9/C11 |
| Injected absolute TTL and 404 | C5 seed; expire a product session with concurrent calls, reinitialize once under the same grant, invalidate old app epoch and race close/revoke |
| GET 405, 202 notification, bearer 401 | C2/C3 and authorization-rejection seed; add every fallback status plus auth/malformed/non-fallback neighbors; 401 fixture is not OAuth discovery or refresh |
| Invalid JSON and fixture body bound | C10 seed; add product-owned body/header/frame limits, invalid UTF-8, split SSE delimiters/data, priming events and notices-before-reply in C4/C10 |
| No existing resumability/slow effect fixture | C6/C8/C9 need gated disconnect, late response, refused/timed-out DELETE and no-replay observations under injected scheduling |
| No existing OAuth/private-store fixture | A1–A10 need deterministic challenge/discovery/TLS/registration/code/refresh/revoke server plus acknowledged/refused/unknown store/audit effects and save/restart interleavings |
| Separate `subagent-contracts` recordings | Keep provider ACP/legacy evidence checks green; they establish no C/A/U runtime row |

The implementing slice records a row-to-test map beside its owning tests. The
OAuth fake must expose dispatch/response/store/audit barriers so tests choose
ordering without sleeps, including a refresh that rotates then loses its reply,
deletion that succeeds before audit fails, and a late write after revoke. Test
both orders at each barrier and accepted neighboring states. Fixtures use only
fixed synthetic credentials; source hashes identify the local fixture/probe
that produced a corpus. Product tests must call real public owners/adapters,
not replay JSON that states the desired result.

## Implementation sequence

The planning PR carries the local developer HTTP fixture and this proposed
contract. The three old `392-remote-mcp` commits remain reference material;
cloud coding starts from merged #391 plus this plan, without transplanting the
old parallel reader or advertising a gateway transport from fixture tests.

| Slice, dependencies | Source owners and deliverable | Acceptance rows |
| --- | --- | --- |
| T0 transport seam, after plan | SDK `infrastructure/mcp/{connection,process,framing,servers,wire}.rs` and `tests/infrastructure/mcp/`: transport-owned tasks/end events and Nessa-owned `HttpExchange` values. Keep stdio framing/correlation behavior; no reqwest/OS store in SDK | Existing stdio suite, capacity/framing and grant-close neighbors for C7/C8/C10/C11 |
| T1 modern HTTP + remote definition, after T0 | SDK MCP HTTP codec/descriptor; gateway `mcp_servers/domain/{configured_server,stand_in}.rs`, `application/{settings,ports}.rs`, `infrastructure/{stored_servers,live_set,inspector,relay,grants}.rs`, `composition/mcp_servers.rs`, product schema/generator/client mapping. Add async reqwest adapter with owned stream/drain, remote UUID/config publication, no-auth connections and factual 401/store-required outcomes | C1/C3–C11, U1/U2 no-token cases and A1; actual SDK/gateway calls against loopback fixture, old stdio/env/revision/live/restoration regressions |
| T2 scoped legacy HTTP+SSE, after T1 | Same SDK HTTP owner and gateway adapter; bounded endpoint/message decoding, same-origin endpoint policy and only the agreed initial POST fallback statuses | C2/C4/C6–C11 in legacy mode; non-fallback auth/5xx/malformed neighbors |
| A0 authorization domain + substitutes, after T1 identities | New gateway `mcp_authorization/{domain,application,contracts}` and matching tests; pure generation/attempt/fence decisions, private-record/audit/HTTP/clock/entropy ports and deterministic TLS server/store fixtures. Domain emits effects rather than accessing SDK internals or keychain | A1–A10 domain/ordering cases, including scopes and uncertain publication; no claim of completed network or product rows |
| A1 consent + private store integration, after A0 | `mcp_authorization/{application,infrastructure,entrypoint}`, gateway composition and generated product/client `mcpServers.authorize/revoke` mappings. Add protected-resource/OIDC discovery, S256/dynamic registration, loopback callback, durable fence/secret records and the private token file; wire manage authority and ready-token port into MCP. Include basic URL-change/removal fencing and retained session/credential cleanup through settings publication, plus revoke/cleanup for pending consent and Ready, before exposing ready-token dispatch | A1–A4/A8–A10 and token-backed U1/U2 through public product routes and real deterministic TLS listener without concurrent refresh; ready-token URL edit/removal, unavailable store, callback restart, basic revoke, discovery/scope and audit neighbors |
| A2 refresh/revoke + definition handoff, after A1 | Same per-server owner, token request seam and current settings/live-set/relay-grant close owners; extend the existing revoke owner to single-flight refresh and concurrent generation races, retained candidates, refresh-specific definition-handoff races, secret deletion, remote observation and evidence settlement | A5–A10, C5/C8/C11 and U2 with tokens; rotating/lost replies, every effect/ack restart point, late write/revoke races, failed drain/deletion/audit |
| U desktop/apps, after T2+A2 | `src/desktop/settings/{model,adapters,ui}`, desktop dependency factory, existing `src/host`/`src-tauri/src/links.rs` native URL seam; gateway/SDK MCP app resource/CSP and current review/ticket owners; generated client API consumption | U1–U5, browser gateway scripts in Chromium/WebKit, narrow layout/focus/pending/failure evidence and one owner-run real remote app check |

Each slice updates the relevant chart/table when tests reveal another ordering,
and publishes limits, module maps and public SDK docs with its owning code.
T1's async reqwest feature changes must respect the gateway's existing TLS
provider composition and package minimum Rust version; check the combined CI
crate selection, rather than relying on an isolated adapter build.

Bare-Node checks run from a temporary tree containing the checked-in
`scripts/mcp-test-server` and `scripts/subagent-contracts` fixtures plus their
checked-in provenance manifests (including the referenced harness lockfile), without
`node_modules`, credentials or harness installations. Run the commands above
and capture/replay the deterministic HTTP corpus. Runtime slices then run the
SDK/server focused suites, SDK/public doc checks, protocol regeneration checks,
architecture checks, formatting and `-D warnings`, followed by the effective
package selection in `.github/workflows/local-auth.yml`. Shared portable
contracts compile on macOS, Linux, and Windows; relay-backed gateway remote
behavior uses the existing Unix capability. Non-Unix returns current
not-configured facts instead of silently bypassing the relay. The sealed
private-file store is the OAuth writer on every OS. A write is unavailable
only when that directory cannot be created. Token bytes stay in the sealed
file.

U uses the repository desktop verification skill/checklist and extends
`verification/desktop/scripts/mcp-servers-gateway.mjs` plus app scripts against
the local gateway fixtures. Cloud checks do not launch a consenting real browser
account or read owner credentials. The final real-server interoperability check
is run by an owner with consent after deterministic checks pass; its exclusion
is visible until that evidence exists, and does not block cloud coding.

Each implementation PR names #392 and the rows it completes. SDK/public API,
module maps, generated contracts and guides change with the owning implementation.
Review requirements remain in the canonical coding standards; implementation
reviews apply them to the actual diff and its declared environment.

## Alternatives and remaining decisions

Direct harness HTTP connections would split tool/app sessions and multiply token
owners. Shared upstream sessions would couple unrelated conversations. OAuth in
the desktop would make refresh depend on a window. These alternatives remain
rejected in favor of gateway-owned sessions and authorization.

Transparent stream resumption, pre-registered/client-metadata registration,
and unrestricted legacy endpoint origins are separate capabilities. MCP
tokens stay in the private file store. Publish supported protocol versions
and finite HTTP/SSE, request, metadata and operation limits in their owning
implementation slices.
No additional compatibility readers or schema version bump is implied by this
record. The two protocol eras are the explicit transport scope of #392.
