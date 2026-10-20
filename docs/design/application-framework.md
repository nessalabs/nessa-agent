# Application framework

Status: proposal. The pull request that adds this document does not build it.
Public names in the examples are proposals. An ADR, when one is needed, takes
the number of the issue that settles that decision. This document does not
add one.

A developer has two ways to run one package. A **plugin** is a view the
desktop hosts. A **standalone app** is the same package in its own window.
One **application process** holds the gateway session. Cedar decides what
that principal may do with the [access grant](access-grants.md). The
package's own data stays the package's.

Harness scope stays Claude and Codex. Alpha has no migration.

## What is built

### Two clients already talk to one gateway

`@nessa/client` (`packages/nessa-client`) opens a WebSocket to `/session`,
answers `session.challenge`, and sends `session.authenticate`. The
credential decides the principal. The `surface` field is metadata.
[ADR 0010](../adr/done/0010-local-authentication.md) separates that
authentication from Cedar authorization on each later call. The package is
private and is not published. Its README says there is no separate plugin
SDK.

The desktop reads `auth/surfaces/nessa-panel.token` through the host
(`src-tauri/src/surface_credential.rs`) and connects with that owner
credential. Another process connects the same way with its own credential.
`crates/nessa-client-core/examples/read_only_sync.rs` is that second
process for a paired read credential: pair, sync, follow. It does not run
an agent.

`nessa-sdk` runs inside the gateway. A package does not embed it.

Credential kinds that exist (`crates/nessa-auth/src/domain/models.rs`):
`human`, `agent`, `integration`, `gateway`. Pairing is what creates a
`gateway` principal (`gateway:<key>`). `credential.issue` refuses one.
`hold.person` in `crates/nessa-auth/src/adapters/cedar/policies.cedar`
lets a human, agent, or integration hold `server.read`,
`credential.manage`, `conversation.read`, and `conversation.write`.
`hold.peer` lets a gateway principal hold `conversation.read` and
`environment.serve`. Those credential grants name no conversation and no
directory ([access grants](access-grants.md)). `issue.member` requires a
member role. `AuthenticateSession` refuses an inactive membership
(`crates/nessa-auth/src/application/session.rs`).

A second `credential.issue` with a new request id adds another credential
on that principal (`crates/nessa-auth/src/adapters/local/registry.rs`).
The webhook proposal's one-registration refusal is not this registry.

`nessa auth token` is already an integration. The CLI issues one with
`server.read` and `conversation.write`, a new member, and no receiver
(`crates/nessa-server/src/cli/infrastructure/gateway.rs`). It is admitted
by the no-binding owner path. An application cannot be "an integration"
with that path turned off, or those tokens stop being able to write.

### Admission when nothing is paired

`admit_conversation` returns success when the session has no receiver
binding (`crates/nessa-server/src/product/read_access.rs`). `reader` maps
that absence to `Reader::Owner`. `admit_request` then takes the owner
path: the credential's manifest grant, and `Initiation::owner()`. The
comment on that module records why: a gateway that pairs no device has no
binding, so every session reads as the owner.

A paired device is the other path. It has a receiver. Its credential holds
`conversation.read`. Comment and drive are the share row, and Cedar's
`grant.comment` and `grant.drive` also require `signedInAs == bindingOwner`
(`crates/nessa-server/src/conversation/infrastructure/access_policy/cedar/readers.cedar`).
A device meets that because it signs in as the owner. A peer gateway signs
in as itself, so those two permits do not match it. The grant lookup is
`receiver_id`. A row with no receiver is not that lookup
(`crates/nessa-server/src/conversation/infrastructure/store/share_grants.rs`).

`owner.read` and `owner.act` permit the principal whose id is the
conversation's owner
(`crates/nessa-server/src/conversation/infrastructure/access_policy/cedar/ownership.cedar`).
The no-binding success returns before that question is asked.

`mcp.callTool`, `mcp.readResource`, `mcp.sendMessage`, and
`mcp.updateModelContext` require the credential grant `conversation.write`
(`protocol/product/manifest.json`). They are not in
`ADMITTED_BY_ITS_OWN_CHECK`.

Revoking or expiring the credential closes a live socket with
`CredentialRevoked` or `CredentialExpired`
(`crates/nessa-server/src/product/socket.rs`).

### A plugin view is a widget, and an MCP App is a tool call

[ADR 326](../adr/done/326-widgets.md) hosts a view as a `WidgetRef`
(`plugin`, `id`) in a message, a pane, or over the panes. The window place
replaces whatever widget is there
(`openWidget` in `src/desktop/workspace/adapters/store/commands.ts`).
Two kinds share that host (`src/desktop/widgets/ui/plugin.ts`):

- `native` — React the window trusts, registered in composition.
  `widgetRegistry` in `src/desktop/dependencies.ts` registers the sample
  and subagents plugins only beside the sample workspace. A gateway
  window passes none.
- `app` — an MCP App, HTML in the sandbox, registered while the window
  runs.

An `app` plugin id is `mcp:` plus the server name
(`src/desktop/widgets/app/model/app-ref.ts`). Its widget id is the
conversation, the execution, and the tool call. The desktop registers the
plugin when a conversation view has a tool with UI
(`src/desktop/widgets/app/adapters/gateway/gateway-apps.ts`). The gateway
refuses `mcp.callTool` unless that call exists and declared a UI
(`crates/nessa-server/src/mcp_servers/domain/app_call.rs`,
`AppRefusal::AppUnknown`). `app_facts` selects the call by `executionId`
and `toolId`. Policy and audit name the app by those two.
`instanceId` is one mount
(`packages/nessa-client/src/generated/product.ts`, `McpAppReference`).

The mount speaks through the caller's credential. The frame does not
receive it. `ui/message` and `ui/update-model-context` are that mount's
conversation port ([MCP App calls](mcp-app-calls.md)). `requestMode` in
`src/desktop/widgets/app/model/places.ts` opens a pane when an inline view
asks for fullscreen. A ready app pane can still be opened in the window
place. The side rail (`src/desktop/workspace/ui/chrome/side-rail.tsx`) is
a prototype. It is not an install list.

`ui/open-link` and `ui/download-file` have ports. Production composition
does not supply them (`appPorts` in `dependencies.ts`).

### The grant row

[Issue 889](https://github.com/nessalabs/nessa-agent/issues/889) made a
conversation share a capability set: `read`, `read+comment`, or
`read+comment+drive`, plus constraints on a drive grant, plus the grant
epoch (`policy_revision`). Cedar's `grant.read`, `grant.comment`, and
`grant.drive` decide. `ShareGrant.receiver_id` is optional in the struct
for an integration
(`crates/nessa-server/src/conversation/application/share_grants.rs`).
`conversation.share` still refuses a target that is not an active paired
device or peer gateway (`share_target_not_paired`).

[The access-grant proposal](access-grants.md) (merged as
[#888](https://github.com/nessalabs/nessa-agent/issues/888)) is the rest
of that row: subject kind `principal` for an integration or a later
agent, one resource (one conversation, or one directory), capabilities
`read`, `comment`, `drive`, `write`, `exec`, constraints, epoch. Cedar
permits or refuses. `ResourceKind::Directory` is reserved and this build
does not write one. Webhook delivery is that proposal's outbox, not a
second grant. Its share target is an integration credential. This plan
uses that same target and does not give it a URL or a MAC.

```mermaid
flowchart TD
  panel[Desktop with the panel token]
  other[Another process with its own credential]
  gateway[Gateway]
  sdk[nessa-sdk Agent]
  call[Tool call that declared a UI]
  mount[MCP App mount]
  panel --> gateway
  other --> gateway
  gateway --> sdk
  call --> mount
  mount --> panel
  panel --> gateway
```

## What this proposes

One package. One integration principal. One install row, owned by the
gateway. One process. The process holds the one gateway socket and every
operation. The plugin view, the standalone window, and a harness tool
call ask that row to attach. The gateway is the only starter.

```mermaid
flowchart TD
  owner[Owner]
  grant[Access grant on the principal]
  cedar[Cedar]
  row[Install row on the gateway]
  process[The one application process]
  plugin[Plugin view]
  standalone[Standalone window]
  harness[Harness tool call]
  mount[MCP App mount of a tool call]
  gateway[Gateway]
  owner --> grant
  grant --> cedar
  cedar --> gateway
  owner --> row
  row -->|start when the endpoint is absent| process
  plugin -->|attach| row
  standalone -->|attach| row
  harness -->|attach| row
  process -->|one socket| gateway
  mount --> gateway
```

The plugin view is the widget window place, drawn in the sandbox, opened
with no tool call. The host asks the gateway to attach and proxies the
frame. It does not exec the package. The frame does not receive the
secret (row H1). The standalone window is the same attach. The MCP
App mount stays a tool call on the caller's session, drawn by the
desktop with the panel token, as today.

Third-party code is the sandbox or this process. The `native` widget kind
stays the trusted views the window registers in composition.

## One process

The application process is the only process that runs the package's
operations, and it holds the one gateway socket. The gateway owns the
install row and is the only starter. A plugin view, a standalone window,
and a harness tool call reach that process by asking the row to attach.
None of them execs the package. Opening the plugin while the standalone
window is already up is an attach, not a second start. Each installed
package has its own row, its own process, and its own socket. A second
package is a second row. It is not a second process for the first.

The row names the package, the one version, the UI document, the process
entry, and the endpoint. The endpoint is a loopback record of the same
kind as the gateway's `gateway-endpoint.json`
(`packages/nessa-client/src/transport/local-gateway-endpoint.ts`): a
private file, and a record whose process does not answer is absent. The
gateway treats that record as absent and starts the process (R6). That
is not the gateway client's error when the gateway itself does not
answer. This plan does not add a
second discovery protocol.

The desktop and the standalone window do not read the endpoint in
order to start. They ask the gateway to attach, and the gateway returns
the live endpoint. The gateway's own tool path reads the row and dials
that endpoint. Attach then follows the record.

- The endpoint is live and the credential is current. The caller joins
  the process that published it. The desktop loads the UI into the
  plugin frame, or the standalone window loads the same UI. The harness
  dials the endpoint as a remote MCP URL. No process is started. A
  closed credential is not this join.
- The endpoint is absent. The gateway starts the process entry from the
  row, once. That start issues the process credential and writes the
  secret once on the pipe to the process. The process reads it before it
  publishes the endpoint, then authenticates. The caller then attaches.
- A second start is already in flight, or it finds the endpoint live. It
  exits before it issues and before `session.authenticate`. The process
  that published the endpoint keeps the socket.
- The endpoint is live and the credential is still current, and the
  socket has dropped. The process authenticates again with the secret it
  already holds. The gateway does not issue another credential and does
  not start another process.

The socket in the state table is that gateway session. A harness
connection is a client of the published endpoint. Two harness sessions
are two of those connections, and still one process and one gateway
socket. A harness session ending closes its connection. The process
stays.

A stored stdio server is a different process: one child per harness
session, ended with that session (`open_admitted` in
`crates/nessa-server/src/mcp_servers/infrastructure/relay.rs`). The
application process is not that child.

## Authentication

The process uses the existing handshake. The credential names the
application principal, an active member of kind `integration`. The panel
token stays the owner's. A `gateway:<key>` credential stays a peer.

The owner installs the package, which mints that principal, its active
membership, and an application registration. Install does not issue a
credential and does not return a secret.

The gateway issues the process credential when it starts the process,
and the gateway is the only issuer of that credential. The process
presents the secret. It does not call `credential.issue` or
`credential.revoke`. `hold.person` already permits an integration to
hold `server.read` and `conversation.read`. The issue does not put
`conversation.write` or `credential.manage` on it.

The issue uses a new request id. The registry stores the verifier only
(`crates/nessa-auth/src/adapters/local/registry.rs`). The secret is in
the issue result and nowhere else the gateway keeps. A repeated request
id returns `ExistingSecretUnavailable` and no secret, so a start does
not retry an old id to recover one. The gateway writes the secret once,
on the pipe to the process it just exec'd, and does not write it to the
install row, the endpoint file, or a credential file. The process reads
it into memory before it publishes the endpoint. The frame does not see
the pipe (H1).

That secret's lifetime is the process. The start closes any earlier
process credential for this principal before it issues, because that
secret died with the earlier process. The process then has one current
credential. A start that finds the old credential already closed still
issues. That replacement is not the webhook's refusal of a second
credential, and it is not surface provisioning. Surface provisioning is
the panel token's rotation, and it supersedes whatever credential the
principal holds. The grant is on the principal.

While the process is up and the credential is current, a dropped socket
is that process authenticating again with the secret it holds. The
gateway does not issue a replacement for the drop, and it does not
inject again.

Revoke, or the socket's built expiry close, closes the socket. The
gateway then stops the process named by the endpoint record and removes
that record. Nothing runs at `expires_at` beyond that built close. The
credential stays closed. A re-authenticate that finds the credential
closed is this stop, not another try with the same secret.
Grants stay. A later attach does not join the old endpoint. It is a
start, and the start issues a new credential. Uninstall removes the
row, so there is no next start.

The registration is the fact Cedar sees. It is not a fifth principal
kind. Slice 1 asks Cedar on the no-binding path and passes that fact:

- No registration. The owner fallback stays. That is the panel token,
  the owner, and `nessa auth token`.
- An application registration. The fallback does not match. A
  principal-subject grant on that resource permits, and anything else
  is refused. `owner.act` does not match this principal.
- A paired receiver stays on the receiver path already built.
- `mcp.*` is not a grant capability. The application process is not
  given a way to present those methods.

`grant.comment` and `grant.drive` as written require a binding owner.
They do not match an integration that signs in as itself. Slice 1 adds
the principal-subject permits for a principal that has an application
registration. Rust gathers the registration, the row, and the action,
and acts on Cedar's answer.

`conversation.share` accepts a principal that has an application
registration or, when that slice exists, a webhook registration. It
does not accept a browser token, which has neither. The application
target does not need a process credential yet. Install can share before
the first start, and the credential a later start issues is asked that
grant. The check does not read a webhook URL or a package path. The two
registrations are two kinds of one record per principal. Each command
refuses a principal that already has the other kind.

A directory grant (`write`, `exec`) stays the directory slice of #888.

```mermaid
sequenceDiagram
  participant Owner
  participant Process as Application process
  participant Gateway
  participant Cedar
  participant Frame as Sandboxed frame
  Owner->>Gateway: Install the package and share one conversation
  Gateway->>Cedar: Ask before writing the grant
  Note over Gateway,Process: An attach finds the endpoint absent
  Gateway->>Gateway: Issue; the secret stays in this start
  Gateway->>Process: The secret, once, on the start pipe
  Process->>Gateway: session.authenticate
  Gateway->>Cedar: This principal, this resource, the row
  Cedar-->>Gateway: Permit or refuse
  Frame->>Process: A UI action, or a read the process will relay
  Process->>Gateway: The product method on its socket
```

## Actions

The base contract is plain TypeScript. It does not import React or a
state library. Redux adapters are out of scope. Two functions, illustrative:

```ts
defineUiAction({ name: "selectChart" })
defineOperation({ name: "saveDashboard" })
```

The public module is what a browser or a sandbox may import. The service
binding lives on a second entry of the same package,
`packages/nessa-app`. The browser entry does not re-export it. The slice
that adds the package holds that with an import test.

A UI action names one live view. The process runs it there. A closed
view, or a different one, is `surface_unavailable`, and the call does not
become an operation and does not write the package's data.

An operation runs in the process. Closing every view leaves it running.
The acknowledgement is the process's. A view that has gone does not
report success (O3). The same request id and the same body are the same
attempt. A second body under that request id is refused, and the first
attempt is left as it was.

The attempt is a record in the process's own store: request id, body,
and outcome. For app-owned data the outcome record is the write, so
there is no gap between the effect and the record. For a Nessa resource
the process stores the gateway receipt before it acknowledges, and a
restart repeats that request id. The gateway already treats that id as
the same command. Restart reads the record and does not start a second
effect (O9–O11).

The package's own dashboard record is app-owned data. Cedar is not asked,
because the record is not a Nessa resource. An operation that both writes
that record and names a Nessa resource asks Cedar before either effect. A
refusal writes neither (row O8). An admitted attempt keeps the epoch it
read.

Registering an operation as an MCP tool is explicit. An unregistered name
is not a tool. A registered tool travels the gateway's existing MCP path
to Claude or Codex. The remote server's URL is the row's endpoint. If
that endpoint is absent, R2 starts the process before the dial.
Its UI, when the tool declares one, is the MCP App mount already built,
on the caller's session. The turn's tool policy is unchanged.

The reference package is a dashboard at `examples/dashboard`. It is not
imported by `src/desktop`. Selecting a chart is the UI action. Saving the
dashboard is the operation.

## Context

A live view may publish a bounded context: what is selected. Replacing it
drops the previous value. Closing the view drops it, and a later send
does not attach it. Two views keep two values.

That context is not `ui/update-model-context`. That method stays the
tool-call mount's port. The bytes a person-send carries from a plugin
view are for the plugin-entry slice to fix. This document fixes when the
value exists.

## States

Rows marked **built** are current behavior. The other rows are the
contract the slice named in [Slices](#slices) tests. Issue, bump, and
revoke of a grant stay the grant table in
[access grants](access-grants.md). The facts below are independent. A
flat status such as "granted" is not a state.

| Fact | Values |
| --- | --- |
| Principal | absent, or present as an active member |
| Credential | none, current, or closed (revoked or expired), per credential |
| Process secret | absent, or held by the running process |
| Grant on resource R | absent, or active at an epoch |
| Install row | absent, or present at one version |
| Endpoint | absent, or live |
| Process | down, or up — at most one |
| Socket | the process's one socket, closed or open |
| View | absent, or live with an id |
| Window place | empty, or showing one widget |
| Operation | idle, running, or uncertain |

### Principal, credential, grant, socket

| Row | Before | Event | After | Effect |
| --- | --- | --- | --- | --- |
| A1 | principal absent | owner installs | principal present, application registration written, credential none, no grants | membership active |
| A2 | credential none, process down | the gateway's start issues, new request id | one credential current, secret held by that process | verifier stored; the secret is not on the row, the endpoint, or a later read of that request id |
| A3 | a process credential current, process down, secret absent | the next start | that credential closed, one new credential current, secret held by the new process | the new request id is not the old one; the closed secret is not replayed |
| A4 | principal present, application registration written | owner shares conversation R, Cedar permits | grant R active | no receiver and no process secret; the epoch of that commit |
| A5 | grant R active | owner shares a set Cedar denies | unchanged | nothing written; the policy ids |
| A6 | credential current | that credential authenticates | its socket open | inactive membership is refused, as `AuthenticateSession` does today |
| A7 | socket open, grant R active | read or follow R | unchanged | Cedar permits; the epoch just read is stamped |
| A8 | socket open, grant R absent | read or follow R, or list it | unchanged | Cedar refuses; no row created |
| A9 | grant R active | owner revokes R | grant R absent | sockets stay; the next admission misses R |
| A10 | credential current, socket open, process up, endpoint live | revoke or expire that credential | credential closed, socket closed, process down, endpoint absent | **built** close reasons; grants and epochs stay; the next attach is R2, not R3 |
| A13 | process up, endpoint live, socket closed, credential current, secret held by the process | the socket drops and the gateway is back | socket open | the process authenticates with the secret it holds; no issue, no injection, no second process |
| A11 | principal present | owner uninstalls | principal absent, install row absent, process down | credentials closed, grants removed, sockets closed; a row from slice 2 is removed and its process is stopped |
| A12 | credential current, no grants | authenticate | socket open | A8 on every conversation |

A4 applies again for a second conversation. The result is a second row.
A9 on one row leaves the other. A6 is the process's socket. A second
process is R4: it exits before this authenticate. A10 does not leave
the endpoint live. A13 is the same process and the same credential.

### The install row

| Row | Before | Event | After | Effect |
| --- | --- | --- | --- | --- |
| R1 | no row | owner installs a directory | row present, endpoint absent, process down | one version |
| R2 | endpoint absent, principal present | plugin, window, or a tool call asks to attach | process up, endpoint live, one socket, secret held by the process | A3 when a process credential is current, otherwise A2; the gateway then starts the process once; the caller does not exec |
| R3 | endpoint live, credential current | another of those asks, including the plugin while the window is up | unchanged | the view attaches; a closed socket under that current credential is A13; a closed credential is A10, then R2 |
| R4 | endpoint absent | two starts at once | one process up | one in-flight start; the process that publishes the endpoint keeps the socket; the other exits before it issues and before `session.authenticate` |
| R5 | process up | the process exits | process down, credential closed; endpoint absent if the process removed the file | the secret died with the process, so the gateway closes that credential; a clean exit removes the endpoint file; the attempt record remains (O9); the next attach is R2; a file left behind is R6 |
| R6 | endpoint present, process does not answer | the gateway reads it | endpoint absent | then R2; the caller still does not exec |

### Views, the window, and context

| Row | Before | Event | After | Effect |
| --- | --- | --- | --- | --- |
| U1 | no live view with that id | invoke the UI action | unchanged | `surface_unavailable`; the package writes nothing |
| U2 | view S live | invoke names S | S live | the action runs on S |
| U3 | view S live | invoke names another id | S live | `surface_unavailable` |
| U4 | action running on S | S closes first | S absent | `surface_unavailable`; the result is dropped; no operation starts |
| U5 | S and T live | each invoke names its own | both live | each runs on its view |
| U6 | S live | the selection changes | S live | S's context is replaced |
| U7 | S live | S closes | S absent | S's context is dropped |
| H1 | the frame is showing the plugin view | the frame reads its document and its bridge messages | unchanged | the credential bytes are not among them |
| M5 | install row present | the person opens the plugin entry | window place shows the plugin | R2 or R3; no tool call; the desktop asks the row to attach and does not exec |
| M6 built | window place showing any widget | `openWidget` to the window, or M5 | window place shows the new widget | the previous view closes, so U7 applies to it; a tool-call mount that lost the window is still that tool call |

### The operation

| Row | Before | Event | After | Effect |
| --- | --- | --- | --- | --- |
| O1 | process up, operation idle | invoke an operation, no view open | running in that process | the process owns it |
| O2 | running | the last view closes | running | it continues |
| O3 | running, attempt record stored | the caller disconnects before the ack | uncertain | the same request id and the same body are this attempt; a different body is refused and this attempt stays |
| O4 | process up | the operation names only a Nessa resource the grant does not admit | idle | refused before the effect; the refusal is the stored outcome |
| O5 | admitted | the grant epoch changes | running | this attempt keeps the epoch it read |
| O6 | — | a tool call for a name that was not registered | — | not a tool |
| O7 | registered | a turn calls the tool | one remote MCP connection to that process | if the endpoint is absent, R2 runs first; the URL is the endpoint, not a stdio command; the session ending closes the connection; the process stays; the mount, if the tool has UI, is the caller's |
| O8 | process up | the operation writes app-owned data and names a Nessa resource | running, or idle | Cedar is asked first; a refusal writes neither and is stored |
| O9 | running or uncertain | the process exits | the attempt record remains | no second effect is started by the exit |
| O10 | record holds the outcome | the process starts again and the same request id and body arrive | idle | the stored outcome is the ack |
| O11 | record holds the attempt and no outcome | the process starts again | running or uncertain | that attempt continues; a different body is refused |

### The mount

| Row | Before | Event | After | Effect |
| --- | --- | --- | --- | --- |
| M1 built | a tool call with UI | the desktop applies the view | plugin `mcp:<server>` | the widget id is conversation, execution, tool |
| M2 built | no such call | `mcp.callTool` | — | `mcp_app_unknown` |
| M3 built | a mount | an `mcp.*` call from the desktop | the caller's session | recorded as the app, on behalf of that caller; the credential grant is `conversation.write` |
| M4 | the application socket | an `mcp.*` method | — | refused; those methods are not granted to this principal, including when a tool call with that id exists |

## Rules the slices test

Each rule names the rows that hold it.

1. Cedar decides an application principal's use of a Nessa resource.
   The no-binding owner fallback stays for a principal with no
   registration, which is how `nessa auth token` writes today. It does
   not match an application registration. Rows A7, A8, A12, O4, O8.
2. One grant is one principal, one resource, the capability set, and the
   constraints. A second conversation is a second row. A4 applied twice,
   then A9 on one of them.
3. The process credential names an active integration member and holds
   `server.read` and `conversation.read`. It is not the panel token and
   not a peer gateway. Rows A1, A2, A6.
4. A UI action runs on the one live view it names. Rows U1–U5.
5. The gateway's install row owns the one process that runs every
   operation. A plugin view, a standalone window, and a tool call attach
   to it and do not exec. The gateway starts that process when the
   endpoint is absent. A second start exits before it issues and before
   it opens a socket. The attempt record survives the process exiting,
   and the next start does not apply the effect twice. Rows R1–R6,
   O1–O3, O9–O11.
6. A view's context does not outlive that view. Rows U6, U7, M6.
7. An MCP App mount stays the tool call it is, on the caller's session.
   Rows M1–M4.
8. The browser entry imports the public contract only. The import test
   holds this.
9. The frame does not receive the credential. Row H1.
10. A registered tool is a remote MCP connection to the running
    application process, on the gateway's Claude and Codex path.
    OpenCode is not added. A stdio launch is not this tool. Row O7.
11. The install list slice 2 writes has one version. Another version is
    refused and not rewritten. Startup does not delete it. Alpha: no
    migration.
12. The process credential's secret is held by that process and nowhere
    else. A start issues it and injects it on the start pipe. A live
    endpoint is a current credential. Revoke, expiry, and process exit
    end that credential and the endpoint together. A socket that drops
    under a current credential is the process authenticating again, not
    a new issue. Rows A2, A3, A6, A10, A13, R2, R3, R5.

## Packaging

Today a native plugin is compiled into the window, an MCP server is a
stored command (`mcpServers.save`), and the Tauri updater replaces the
desktop binary (`src-tauri/src/updater.rs`). None of those is a package
a third party installs.

The package is a directory: a manifest (display name, process entry, UI
entry), the public actions, and the UI document the sandbox loads. The
owner installs it on this gateway. The install list is the owner's,
beside the stored MCP servers, not a channel of the updater. The first
install is a local path. That install is one owner action: it mints the
principal in [Authentication](#authentication) and writes the row in
[One process](#one-process). It does not issue the process credential.
That issue is the start. The row is how every entry point reaches the
process, and it lands before any view. A signed feed is out of scope.

A stored stdio server is one process per harness session, ended with
that session (`open_admitted` in
`crates/nessa-server/src/mcp_servers/infrastructure/relay.rs`). The
application process is not that child. Its tools are a remote server
whose URL is the row's endpoint. If that endpoint is absent, R2 starts
the process before the dial. The session's end closes the connection.

`nessa init` scaffolds a directory the install command already accepts.
Views do not wait on the scaffold. They install a directory of that shape
with the same command.

First-party research views stay MCP servers in `nessa-extensions`
([ADR 344](../adr/done/344-mcp-ui.md)). This plan does not move them
into the desktop tree.

## Reference acceptance

The dashboard, in order, once the slices have landed:

1. The process authenticates as the application with the secret the
   start injected. The page it serves reads the one granted conversation
   and does not open a gateway socket. Another conversation is refused,
   and so is a list of the rest. Revoking the credential removes the
   endpoint, and the next open authenticates with a new one. A gateway
   restart that leaves the process up re-authenticates with the secret
   the process still holds (A6–A8, A10, A12, A13, R2, R3).
2. Chart selection runs on the open view and is refused when that view
   is closed, with nothing saved (U1, U2, U4).
3. Save runs with no view open. Dropping the view leaves the
   acknowledgement uncertain until the process answers. A retry with a
   different body is refused (O1–O3).
4. The save is an MCP tool only after it is registered. The remote
   server's URL is the row's endpoint, and an absent endpoint starts the
   process once before the dial. An unregistered name is not listed.
   Ending the harness session does not stop the process (O6, O7, R2).
5. The plugin entry opens the dashboard with no tool call. The desktop
   asks the row to attach and does not exec, including when the
   standalone window already has the process up. Opening it while a
   tool-call widget has the window place replaces that widget and drops
   its view context. Closing the entry does not stop the process
   (M5, M6, R3, O2, U7).
6. The standalone window asks the same row to attach and does not exec.
   When the process is already up, the window joins it and does not open
   an application socket. When the endpoint is absent, the gateway starts
   the process once and the window joins that process (R2, R3).
7. `nessa init` produces a package that can do 1.

## Slices

Each slice is its own issue. None of them is this pull request.
Each slice depends only on slices above it.

| Slice | Delivers | Rows |
| --- | --- | --- |
| 1. Principal, grant, and admission ([#1052](https://github.com/nessalabs/nessa-agent/issues/1052)) | The active member and the application registration at install, with no process secret returned. Issue and revoke of an integration credential holding `server.read` and `conversation.read`, so a start can issue and can close the one it replaces. `conversation.share` accepts that registration and refuses a browser token. No-binding admission asks Cedar with the registration fact: no registration keeps today's owner fallback; an application registration matches a principal-subject grant or is refused. `mcp.*` stays off this principal. | A1, A4–A9, A11, A12, M4 |
| 2. Install row and the one process ([#1067](https://github.com/nessalabs/nessa-agent/issues/1067)) | The gateway owns the versioned row: UI document, process entry, and the endpoint. The gateway is the only starter and the only issuer of the process credential. A start injects the secret on the process pipe and does not store it. Credential closure stops the process and removes the endpoint. A current credential re-authenticates in the same process. Uninstall removes the row and stops the process. The remote MCP URL is that endpoint. A stdio child of a harness session is not this process. | A2, A3, A6, A10, A11, A13, R1–R6, rule 11, rule 12, O7 |
| 3. Connected web view ([#1053](https://github.com/nessalabs/nessa-agent/issues/1053)) | `examples/dashboard` is a page the process serves. The page does not open a gateway socket. It renders the granted conversation. No desktop import. | A6–A8, A12, R3 |
| 4. Action contract and the dashboard save ([#1054](https://github.com/nessalabs/nessa-agent/issues/1054)) | `packages/nessa-app`. UI action, operation, the attempt record, explicit MCP registration, the import test. UI actions run on the page slice 3 serves. The process from slice 2 runs the operations. | U1–U7, O1–O11 |
| 5. Plugin entry ([#1055](https://github.com/nessalabs/nessa-agent/issues/1055)) | The window place asks the row to attach and does not exec, whether or not the endpoint is already live. H1. M6 against a tool-call widget already in the window place. Mounts stay M1–M3. | H1, M5, M6, R2, R3, U6, U7 |
| 6. Standalone window ([#1056](https://github.com/nessalabs/nessa-agent/issues/1056)) | The window asks the same row to attach and does not exec. A live endpoint is a join. An absent endpoint is the gateway's one start, then the same join. | R2, R3, acceptance 6 |
| 7. `nessa init` ([#1057](https://github.com/nessalabs/nessa-agent/issues/1057)) | The scaffold writes a directory slice 2 already installs. It does not add a second start. | acceptance 7 |

[#1067](https://github.com/nessalabs/nessa-agent/issues/1067) is the
install row and the start that issues the process credential. It is not
a later packaging step that the views assume.
[#1052](https://github.com/nessalabs/nessa-agent/issues/1052) as filed
issues the credential at install and returns the secret once. The
process credential is issued at the start, and the secret is not
returned to the owner.
[#1057](https://github.com/nessalabs/nessa-agent/issues/1057) as filed
still bundles that install, the versioned list, and a stored-server
launch with the scaffold, and it says it depends on the action contract.
Those moved to #1067. What remains of #1057 is the scaffold. O7's tool
is the endpoint, not a stdio launch.
[#1055](https://github.com/nessalabs/nessa-agent/issues/1055) as filed
says the host starts the process. M5 and R3 are the contract: the
desktop asks the row to attach and does not exec.
[#1056](https://github.com/nessalabs/nessa-agent/issues/1056) as filed
says a second process is a second socket. R2 and R3 replace that: the
window joins the process the gateway started.

## Alternatives

**Give the package the panel token.** The process would connect as the
owner. `conversation.write` on that credential is the conversation
authority for the gateway. The frame would be one step from that secret.
The grant is the scope.

**Treat a stored grant row as enough.** That is the paired-device path,
and this principal does not walk it. No receiver means
`admit_conversation` returns success and `reader` says `Owner`.
`grant.comment` requires `signedInAs == bindingOwner`. A row with a null
`receiver_id` is not the lookup. Slice 1 asks Cedar with the registration.
It does not store a row and stop.

**Turn the owner fallback off for every integration.** `nessa auth token`
issues an integration with `conversation.write` and no registration, and
that fallback is how it writes. An application is the registration, not
the kind.

**Add a principal kind `application`.** `PrincipalKind` already has
`integration`, and #888 names an integration as a grant subject with no
receiver. The registration is the fact that separates an application
from a browser token. A fifth kind needs a hold policy beside
`hold.person`.

**Pair the package as a device.** Device pairing pins a key and a
receiver. `DeviceRead` signs in as the owner. A peer signs in as
`gateway:<key>`, and `hold.peer` is the limit on that kind. Only pairing
creates a gateway principal. Linked devices already own that enrollment.

**Make the plugin entry a synthetic tool call.** The widget id is the
conversation, the execution, and the tool. `known_app` refuses anything
else, and an opening's end ends that mount. The entry is a view (M5).

**Let the package decide its own allow list.** That is a second decider
beside Cedar.

**Extract the desktop transcript before a view exists.** The transcript
is a projection in `src/desktop/workspace`. The client already returns a
conversation view. A shared React kit, and any Redux adapter, wait until
a second app needs them.

**Ship packages through the Tauri updater.** The updater replaces the
desktop binary. The install list sits beside stored MCP servers. The
tool route is the row's endpoint, not a stdio command, because stdio's
process ends with the harness session.

**Keep the bearer on the install row or in the registry.** The registry
stores a verifier. A repeated request id does not return the secret.
The row is the rendezvous a view attaches through. Neither place can
supply `session.authenticate` on a later start.

**Store a panel-token file per package.** The panel file is the owner's
credential, read back by the desktop. A package file would be a second
persisted bearer. Revoke would have to delete it, or the next start
would present a dead secret. Surface provisioning is that panel
rotation, and it supersedes the principal's credentials. The process
credential is an ordinary issue, and its secret lives only as long as
the process.

**Leave the process up after the credential closes.** The endpoint
would stay live. The next attach would be R3 and would join a process
that cannot authenticate. Closure removes the endpoint, and the next
attach starts again with a new credential.

**Let each entry point start the package.** The desktop would spawn a
process when the plugin opens, and the standalone window would spawn
another if it did not find one. Those are two sockets and two attempt
records. The gateway is the only starter. A plugin opened while the
window is already up attaches (R3). A second start exits before it
authenticates (R4).

**Make the plugin and the standalone window mutually exclusive.** That
drops the rendezvous by allowing only one of the two views. Both views
attach to the one process.

**One grant for the catalogue.** A grant by channel, by a folder of
chats, or by default sync is already refused. Two conversations are two
rows.

**One share check owned by whichever slice merges first.** Share accepts
a principal that has an application registration or a webhook
registration, and it does not read a URL. The registration record is
the one place a principal is one of those, or neither, and neither is a
browser token.

## Out of scope

- Redux or another state-library adapter.
- Publishing `@nessa/client` or `packages/nessa-app`.
- A marketplace or a signed package feed.
- Directory `write` and `exec`.
- The owner phone ([#811](https://github.com/nessalabs/nessa-agent/issues/811)).
- Moving `nessa-extensions` into this repository.
- Supplying `links` and `downloads` on the production app ports.
- OpenCode as a host.
- Loading third-party React as a `native` widget.
