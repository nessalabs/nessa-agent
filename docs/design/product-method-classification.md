# Product method classification: source catalogue and implementation plan

Owner: [#921](https://github.com/nessalabs/nessa-agent/issues/921). This is the
implementation companion to [ADR 921](../adr/todo/921-close-product-method-classification.md).
The ADR owns the architectural decision; this document owns the source inventory,
type sketch, migration and regression plan. All changes below are proposed.
Runtime classification is still string-based; no compile probe or runtime
migration is implemented by these documents.

## Current source catalogue


The manifest already owns product method names, parameter types, and the Cedar
grant asked before dispatch. Its generator publishes string constants,
`PRODUCT_READY_METHODS`, and `action_for_method`. The gateway then makes four
independent choices from the original string:

1. `product/socket.rs` routes exact names and name prefixes to handler modules;
2. `ResponseClass::for_method` selects Control, App, Record, or the Ordinary
   default before work takes a lane permit;
3. the socket separately selects server execution capacity (controls, record
   reads, deletions, upload beginnings, artifact reads, or ordinary requests);
   App work has no socket-owned server permit, and connection-owned paths branch
   before these pools; and
4. `product/read_access.rs` selects Drive and Comment paired access from exact
   names, gives every other conversation write the Write default, and separately
   lists methods whose own read/list/follow admission is authoritative. Its share
   handler also applies paired Share/Unshare/Shares checks after manifest-grant
   admission.

Handler modules make further exact-name choices. Subscriptions have
connection-owned dispatch before the ordinary handler; the handshake and
`auth.session` have lifecycle-owned dispatch. Unknown strings are refused as
`unknown_method`, but they currently inherit the Ordinary lane on their way to
that refusal, subject to existing request validation, session checks and lane
capacity. An invalid or saturated request can therefore be refused earlier.

The test named `every_manifest_method_has_one_runtime_dispatch_path` proves only
that manifest names equal the generated advertised-name list and that most names
have a generated grant. A newly generated, granted name satisfies those checks
without selecting a handler, a non-default lane or execution pool, or paired admission. No
advertised method is presently known to lack a handler, and this finding was
source inspection rather than a runtime reproduction.

The extension path therefore crosses these owners:

| Concern | Current owner | Extension step |
| --- | --- | --- |
| Wire name, params, grant | `protocol/product/manifest.json` and `v1.json` | Add the method/schema and regenerate Rust and TypeScript |
| Known-name identity | generated string constants/list | Generated indirectly from the manifest |
| Authentication lifecycle | product socket | Place handshake and session methods in their lifecycle path |
| Handler route | product socket and handler module | Add a top-level route and the handler's exact method case |
| Response capacity | product socket | Add a lane case only when the Ordinary default is wrong |
| Execution capacity | product socket | Select a server pool; otherwise the ordinary-request pool is inherited |
| Paired admission shape | `product/read_access.rs` | Update the own-check list, Drive/Comment choice, or compound paired share action |
| Policy verdict | Cedar behind the access ports | Keep deciding whether the admitted principal may act |
| Client presentation | generated client plus the consuming feature | Add the typed call and its product behavior |

This is related to #797 only at a later boundary. A future private admitted
command may bind the parsed method, caller, and immutable initiation together.
That command must still check grant freshness at effect entry; issuance cannot
turn an authorization observation into a permanent capability. This decision
does not implement or enlarge that work.

Current source owners (reviewed at `9812c3c`):
[manifest](../../protocol/product/manifest.json),
[generator](../../scripts/generate-product-protocol.mjs),
[socket](../../crates/nessa-server/src/product/socket.rs),
[paired access](../../crates/nessa-server/src/product/read_access.rs),
[subscriptions](../../crates/nessa-server/src/product/subscription/mod.rs), and
[writer tests](../../crates/nessa-server/tests/product/socket/writer.rs),
[artifact reader](../../crates/nessa-server/src/product/attachment.rs),
[record admission](../../crates/nessa-server/src/product/record_read/dispatch.rs), and
[catalogue admission](../../crates/nessa-server/src/product/catalogue_read/dispatch.rs).

## Proposed admission taxonomy

The fourth field is `AdmissionPlan`: it
selects an existing admission owner and the operation shape that owner consumes.
It is neither a policy verdict nor a universal authorization executor. Runtime
facts and policy evaluation stay inside those owners. The source catalogue below
assigns one path to each current manifest method; it inventories socket-level
admission, not downstream application policies inside a successful handler.

| Plan path | Existing owner and stages retained |
| --- | --- |
| Authentication | Socket challenge/authentication; authenticated repetition retains its refusal path |
| CurrentSession | Socket's `auth.session` path and browser presence |
| Manifest | `read_access::admit_request` asks the generated grant; no extra socket-level paired action for this method |
| Request(Drive / Comment / CatalogueWrite) | `read_access::admit_request` consumes this typed static shape; preserves owner fallback, paired facts and current pairing-grant substitution for Drive/Comment; Cedar decides the result |
| ManifestThenConversationRead | Request owner admits generated grant, then conversation handler calls `admit_conversation` |
| ManifestThenList | Request owner admits generated grant, then conversation handler calls `admit_list` |
| ManifestThenArtifactRead | Request owner admits generated grant, then attachment reader calls `admit_conversation` before artifact effects |
| PassiveRecord / PassiveCatalogue | Record/catalogue read facade constructs `AdmitPassiveRead`: generated passive grant, current identity/binding, metadata/grant and policy checks before source work |
| ManifestThenShare(Share / Unshare / Shares) | Request owner admits generated grant, then `dispatch_share` asks paired-share admission with the typed action before effects; transactional share checks remain in application/storage |
| FollowConversation / FollowList / FollowAgents | Subscription owner calls `authorize_batch` before registration and before each batch; conversation follow retains pairing-grant substitution, list follow calls `admit_list`, and agents follow asks its manifest grant |
| UnsubscribeConversation / UnsubscribeAgents | Socket calls `admit_now` inline before connection-owned stop; conversation unsubscribe retains paired-follow grant substitution and token ownership |

The other fields select a handler entry point, response lane and execution
capacity. Capacity values name the existing Controls, RecordReads, Deletions,
UploadBegins, ArtifactReads and Requests pools. AppOwned retains capacity with
the conversation service after socket departure; ConnectionOwned branches before
these pools. Authentication retains pre-authentication ownership and the Requests
pool on an authenticated repetition. No new permit owner is introduced.

Static shape selection moves to the plan; existing admission facades consume it
instead of rematching strings. Request and share shapes do not copy Cedar rules.
Handler operation dispatch can retain exact typed matches, but must not invent a
second routing owner. Required plan fields have no defaults; the synthetic
extension probe checks catch-all inheritance. No compile-time claim covers a
future policy check that an implementation fails to model: semantic completeness
still needs source review and per-method tests against this catalogue.

Unknown strings stay on the current validation, authentication and bounded
Ordinary refusal path; early parsing does not bypass those checks. Classification
contains no cached verdict, membership, grant or policy revision. The future
admitted-command proposal in #797 remains outside this migration.

## Complete current method catalogue

Snapshot: all manifest methods at `9812c3c`; path labels refer to the owners above.
The cutover's tests should encode reviewed expectations, not derive expected
values from the new classifier under test. `Authentication` is phase-sensitive
as described above; unknown names are absent and retain Requests/Ordinary refusal.

| Method | Handler owner | Response lane | Execution capacity | Admission plan |
| --- | --- | --- | --- | --- |
| `session.authenticate` | `socket` | Ordinary | Authentication | Authentication |
| `auth.session` | `socket` | Ordinary | Requests | CurrentSession |
| `server.health` | `socket` | Ordinary | Requests | Manifest |
| `credential.issue` | `socket` | Ordinary | Requests | Manifest |
| `credential.list` | `socket` | Ordinary | Requests | Manifest |
| `credential.revoke` | `socket` | Ordinary | Requests | Manifest |
| `conversation.create` | `conversation` | Ordinary | Requests | Request(CatalogueWrite) |
| `conversation.read` | `conversation` | Ordinary | Requests | ManifestThenConversationRead |
| `conversation.recordsHead` | `record_read` | Record | RecordReads | PassiveRecord |
| `conversation.recordsPage` | `record_read` | Record | RecordReads | PassiveRecord |
| `conversation.list` | `conversation` | Ordinary | Requests | ManifestThenList |
| `conversation.observe` | `conversation` | Ordinary | Requests | ManifestThenList |
| `conversation.send` | `conversation` | Ordinary | Requests | Request(Drive) |
| `conversation.steer` | `conversation` | Ordinary | Requests | Request(Drive) |
| `conversation.remove` | `conversation` | Control | Controls | Request(CatalogueWrite) |
| `conversation.stop` | `conversation` | Control | Controls | Request(Drive) |
| `conversation.receipt` | `conversation` | Ordinary | Requests | Request(Drive) |
| `conversation.message` | `conversation` | Ordinary | Requests | Request(Comment) |
| `conversation.messageStatus` | `conversation` | Ordinary | Requests | Request(Comment) |
| `conversation.answer` | `conversation` | Control | Controls | Request(Drive) |
| `conversation.answerQuestion` | `conversation` | Control | Controls | Request(Drive) |
| `conversation.cancel` | `conversation` | Control | Controls | Request(Drive) |
| `conversation.close` | `conversation` | Control | Controls | Request(CatalogueWrite) |
| `conversation.archive` | `conversation` | Control | Controls | Request(CatalogueWrite) |
| `conversation.unarchive` | `conversation` | Control | Controls | Request(CatalogueWrite) |
| `conversation.rename` | `conversation` | Control | Controls | Request(CatalogueWrite) |
| `conversation.delete` | `conversation` | Ordinary | Deletions | Request(CatalogueWrite) |
| `conversation.reorder` | `conversation` | Control | Controls | Request(CatalogueWrite) |
| `artifact.read` | `attachment::dispatch_read` | Ordinary | ArtifactReads | ManifestThenArtifactRead |
| `attachment.begin` | `attachment` | Ordinary | UploadBegins | Request(CatalogueWrite) |
| `agents.list` | `socket` | Ordinary | Requests | Manifest |
| `agents.setNativeHooks` | `socket` | Ordinary | Requests | Manifest |
| `agents.subscribe` | `subscription` | Ordinary | ConnectionOwned | FollowAgents |
| `agents.unsubscribe` | `subscription` | Ordinary | ConnectionOwned | UnsubscribeAgents |
| `conversation.setApprovalMode` | `conversation` | Ordinary | Requests | Request(CatalogueWrite) |
| `conversation.switchAgent` | `conversation` | Ordinary | Requests | Request(CatalogueWrite) |
| `conversation.fork` | `conversation` | Ordinary | Requests | Request(CatalogueWrite), then Act on the source (its owner only) |
| `agents.installOptions` | `agent_install` | Ordinary | Requests | Manifest |
| `agents.install` | `agent_install` | Ordinary | Requests | Request(CatalogueWrite) |
| `conversation.catalogueHead` | `catalogue_read` | Record | RecordReads | PassiveCatalogue |
| `conversation.catalogueManifest` | `catalogue_read` | Record | RecordReads | PassiveCatalogue |
| `conversation.catalogueResolve` | `catalogue_read` | Record | RecordReads | PassiveCatalogue |
| `mcp.callTool` | `mcp_apps` | App | AppOwned | Request(CatalogueWrite) |
| `mcp.readResource` | `mcp_apps` | App | AppOwned | Request(CatalogueWrite) |
| `mcp.releaseApp` | `mcp_apps` | Control | Controls | Request(CatalogueWrite) |
| `mcp.sendMessage` | `mcp_apps` | App | AppOwned | Request(CatalogueWrite) |
| `mcp.updateModelContext` | `mcp_apps` | App | AppOwned | Request(CatalogueWrite) |
| `mcpServers.list` | `mcp_servers` | Ordinary | Requests | Manifest |
| `mcpServers.save` | `mcp_servers` | Ordinary | Requests | Manifest |
| `mcpServers.remove` | `mcp_servers` | Ordinary | Requests | Manifest |
| `mcpServers.inspect` | `mcp_servers` | Ordinary | Requests | Manifest |
| `mcpServers.authorize` | `mcp_servers` | Ordinary | Requests | Manifest |
| `mcpServers.revoke` | `mcp_servers` | Ordinary | Requests | Manifest |
| `pairing.create` | `pairing` | Ordinary | Requests | Manifest |
| `pairing.pending` | `pairing` | Ordinary | Requests | Manifest |
| `pairing.status` | `pairing` | Ordinary | Requests | Manifest |
| `pairing.approve` | `pairing` | Ordinary | Requests | Manifest |
| `pairing.deny` | `pairing` | Control | Controls | Manifest |
| `pairing.cancel` | `pairing` | Control | Controls | Manifest |
| `pairing.setLinking` | `pairing` | Ordinary | Requests | Manifest |
| `peer.enroll` | `peers` | Ordinary | Requests | Manifest |
| `peer.list` | `peers` | Ordinary | Requests | Manifest |
| `peer.forget` | `peers` | Ordinary | Requests | Manifest |
| `peer.relay` | `peers` | Ordinary | Requests | Manifest |
| `conversation.subscribe` | `subscription` | Ordinary | ConnectionOwned | FollowConversation |
| `conversation.subscribeList` | `subscription` | Ordinary | ConnectionOwned | FollowList |
| `conversation.unsubscribe` | `subscription` | Ordinary | ConnectionOwned | UnsubscribeConversation |
| `conversation.share` | `read_access::dispatch_share` | Ordinary | Requests | ManifestThenShare(Share) |
| `conversation.unshare` | `read_access::dispatch_share` | Ordinary | Requests | ManifestThenShare(Unshare) |
| `conversation.shares` | `read_access::dispatch_share` | Ordinary | Requests | ManifestThenShare(Shares) |
| `conversation.permissionRules` | `conversation` | Ordinary | Requests | Request(CatalogueWrite) |
| `conversation.revokePermissionRule` | `conversation` | Ordinary | Requests | Request(CatalogueWrite) |

Catalogue membership checked against all **71** current manifest methods.

## Verification plan

| Change or defect | Enforcement |
| --- | --- |
| Manifest method has no unique Rust identity | Generator collision/coverage test fails; generated-output drift check fails |
| Generated `ProductMethod` gains a variant with no method plan | Rust compilation fails on the non-exhaustive match |
| A method arm omits handler, lane, execution capacity, or admission plan | Required `MethodPlan` fields make that arm fail Rust compilation |
| A new method silently inherits a catch-all plan | The synthetic extension compile probe fails if the unedited classifier compiles |
| Existing method is assigned the wrong handler/lane/capacity/admission plan | Table-driven gateway tests compare the plan with reviewed expectations |
| A selected handler does not implement its assigned method | Per-method handler tests reject `unknown_method`; classification compilation alone cannot detect this |
| Generated names/actions disagree with the manifest | Generator tests and the renamed membership/action test fail |
| Unknown input becomes known or bypasses refusal | Socket test fails before handler effects and asserts `unknown_method` after successful existing admission prerequisites |
| Control work can be starved by ordinary saturation | Existing saturated-lane socket tests fail; the migration adds one typed-method counterpart |

The implementation proof adds a generator fixture with a synthetic granted
method, regenerates into a temporary tree, and compiles the real gateway
classifier module without editing it. The expected result is a
non-exhaustive-match compiler error. This proves omission detection for a new identity: it cannot inherit an
existing plan. Required struct fields separately check completeness of the
plan. It does not prove correct assignments or handler invocation. After a
deliberate plan is added, focused tests assert its exact handler, lane, capacity,
and admission plan. The fixture must not modify checked-in generated files or the live manifest. Keep
the classifier dependency-light enough to compile that real module with
`rustc` in the temporary fixture; do not compile a retyped example classifier.
This probe belongs in the existing product-protocol check. No additional source
parser or AST-shape guard is needed to restate the compiler's check.

Rename `every_manifest_method_has_one_runtime_dispatch_path` to describe its
actual guarantee: manifest/generated method membership and generated action
membership agree. The exhaustive `ProductMethod -> MethodPlan` compilation
proof establishes classification completeness. Classification tests establish
the selected plan;
per-method handler tests establish implementation, and socket tests establish
that admission and execution consume the selected plan.

## Migration and ordering plan

| Slice | Change | Behavior proof |
| --- | --- | --- |
| Identity | Teach the generator to emit `ProductMethod`, `as_str`, and fallible parsing, preserving current wire strings. This inert publication replaces no running classifier | Generator round trips each manifest name and refuses unknown/case-changed names and variant collisions |
| Gateway cutover | Add the exhaustive `MethodPlan` and switch socket admission, queue/lane and execution-pool classification, lifecycle routing and handler entry points to the parsed identity in the same PR. Delete `ResponseClass::for_method`, `paired_action`, the own-check string list, execution-pool string checks, paired-share action string checks and prefix routing as their consumers move; no merged intermediate state keeps two authorities | A reviewed per-method table checks handler/lane/capacity/admission plan; handler tests exercise each target; existing authorization, unknown-method, capacity and saturation suites pass |
| Extension proof (part of cutover) | Rename the overstated membership test and add the synthetic extension compile probe to the existing product-protocol check | A new manifest variant cannot compile against the unedited classifier; making a plan field optional/defaulted must be rejected in review and exercised by the classification tests |

The complete catalogue above supplies the cutover's expectations. Tests must
separately exercise response-slot and execution-pool saturation: an available
Deletions/UploadBegins/ArtifactReads execution pool does not bypass the Ordinary
response-slot bound, while full Requests execution capacity does not consume
those independent pools. AppOwned capacity remains with the conversation service
when a socket disappears.

The following ordering cases supply the cutover's socket regression matrix.
Each row needs a positive and refusal assertion where applicable; implementation
adds exact test names to the PR, rather than treating this design as executed proof.

| Input/order | Preserve at cutover | Evidence owner |
| --- | --- | --- |
| Authentication method before and after authentication | Existing challenge/authentication path; `already_authenticated` after authentication | Socket handshake tests |
| Valid known ordinary/app/record method | Its current handler, admission facts and lane; one parsed identity reaches execution | Per-method table, handler and socket tests |
| Unknown name, valid session and available Ordinary slot | `unknown_method`, no handler effects | Socket unknown-method test |
| Unknown name with invalid frame, expired session or saturated Ordinary lane | Existing validation/authentication/capacity refusal ordering; no early parser response that bypasses it | Socket validation, expiry and saturation tests |
| Ordinary or App work held while a control arrives | Control admission and bounded response delivery keep their independent capacity | Existing writer/control saturation tests plus typed-method counterpart |
| Paired Drive, Comment, catalogue Write and own-check method | Same target, initiator and Cedar question before effects; unchanged policy verdict authority | Paired-access and socket tests |
| Record head/page or artifact read with manifest grant refused, then with grant admitted but conversation read refused | Either stage prevents source/handler effects; valid grant plus admitted read succeeds | Passive-read and artifact socket tests |
| Share/unshare/shares from a paired party, including a credential holding the manifest grant | Preserve both the manifest grant and the distinct paired Share/Unshare/Shares question before effects; refusing either stage prevents handler effects | Paired share admission and socket tests |
| Subscription starts, refreshes and ends | Existing connection ownership, per-batch authority checks and teardown | Subscription socket tests |

Each slice updates generated output and consumers together. There is one current
wire contract: no alias parser, legacy enum, dual dispatch, schema-version bump,
or compatibility path. The migration neither adds a command bus nor moves every
gateway concern into the protocol crate. The plan contains only identity-based
classification; current credential, grant and conversation facts remain with
their existing admission owners and are read at the same boundaries as today.

