# Nessa application framework: the developer experience

**Status:** proposal, 2026-10-14. Tracked by [#910](https://github.com/nessalabs/nessa-agent/issues/910).
This is a product and public-interface direction, not an accepted ADR or an
implementation specification. All new commands, package names, and APIs below
are illustrative. Existing contracts remain authoritative until deliberately
changed. The implementation plan must follow the repository's
[coding standards](../../CODING_STANDARDS.md), rather than treating this proposal
as a second standards document.

## The experience we want

A developer should be able to create a Nessa project, build their application,
and get a capable agent experience without assembling agent infrastructure.
The application might be a dashboard, editor, research workspace, or calendar.
Chat is an available way to work with that application, alongside its own UI.

The same application's actions, data, and views should support three targets:

| Target | What the user opens | How it reaches Nessa |
| --- | --- | --- |
| Plugin | An installed app inside Nessa desktop | The host brokers authorized operations through its gateway connection |
| Web app | The project's own browser application | The client connects to an explicitly authorized Nessa gateway |
| Desktop app | The project's own installed desktop application | A small native host connects the client to an authorized Nessa gateway |

An independent app does not require Nessa desktop to be open. It does require
a running gateway and a supported execution environment. Independence from
desktop is not independence from the gateway. A future gateway distribution
should make installation and operation possible without installing the desktop UI.

The initial promise is deliberately local: a developer can use the gateway on
their machine. Building a web bundle is different from making a deployed website
securely reach a user's local gateway. HTTPS, browser origin restrictions,
credential provisioning, and remote reachability need explicit supported flows;
publishing a website does not supply them automatically.

Nessa should first make this path excellent with TypeScript, React, the existing
UI components, and the existing gateway. Support for additional frontend
frameworks, hosted deployments, and alternate backends can follow. Developers
should retain ownership of their code and data throughout.

## Work backward from the first fifteen minutes

The proposed entry point is one command, with interactive and reproducible forms:

```sh
nessa init revenue-workspace

# Equivalent non-interactive choices; proposed syntax.
nessa init revenue-workspace --target web --template blank
nessa init revenue-workspace --target desktop --template blank
nessa init revenue-workspace --target plugin --template blank
```

The interactive flow asks for a project name, target, and template. React and
TypeScript are the supported first path. A blank project is small but already
works: an empty app canvas, a connected chat experience, and a connection screen.
A separate example template demonstrates shared actions and data without making
sample business logic part of every new project.

After initialization, the developer follows the generated instructions:

```sh
cd revenue-workspace
pnpm install
pnpm dev
```

The development launcher discovers a local gateway through the supported
discovery mechanism, asks the user to connect and approve the project's requested
access, and opens the app. If no gateway is available, it gives a concrete way to
start or install one. It never claims that a fixture is a live agent connection.
An explicit mock mode supports UI work without a provider account.

The first screen offers the app canvas with side chat and a full conversation
view. Both show the same conversation. Switching presentation neither submits a
prompt nor opens another agent session. App authors can hide chat or build their
own presentation over the same controller.

The developer can immediately ask the configured agent a question, inspect a
tool result, answer a permission request, and reconnect after a temporary gateway
failure. They do not need to configure another model SDK, duplicate tool setup,
or build a transcript renderer.

For people and coding agents, the generated project includes a short README,
task-oriented agent instructions, typed examples, and commands with useful help.
Non-interactive initialization and machine-readable diagnostics make the same
workflow usable from automation.

## What a project looks like

The starter should make the app's responsibilities visible. For an example
dashboard project, a possible layout is:

```text
nessa.config.ts             app identity, targets, entry views, requested access
src/
  main.tsx                 composition: connection, app controller, root view
  dashboards/
    actions.ts             operation definitions and public input/output schemas
    service.ts             dashboard rules and storage port
    view.tsx               dashboard UI
    context.ts             meaningful selection and filter context
server/
  main.ts                  action service composition and storage adapter
tests/                     app-owned integration scenarios
README.md                  start, connect, develop, build, diagnose
AGENTS.md                  how to work on this generated project
```

This is an example after adding a feature, not a requirement to generate empty
layers. Browser bundles contain public contracts, never server handlers or
secrets. A plugin build packages its UI as an MCP App resource; a standalone build
packages the same app views for its own host. Native window setup belongs to the
desktop target. Business operations stay outside that window setup.

The first release can choose a single target at initialization. Supporting the
same reference app in all targets should establish the shared contracts before
we promise a universal target-conversion tool.

## The public building blocks

### 1. Connect to Nessa once

Developers should see one connection lifecycle: discovering, connecting,
connected, reconnecting, or needing user action. The framework supplies a ready
connection UI and leaves a lower-level client available for custom applications.

The gateway remains the owner of configured agents, provider sessions,
connections, and tool authorization. An app requests access to that setup; it
does not copy provider credentials, OAuth tokens, or desktop configuration files.
The gateway exposes the capabilities available to that app and explains missing
access or unsupported operations.

Each app has a stable identity and its own approved access. The default starter
creates app-associated conversations; it does not silently expose the user's
desktop history. Opening an existing conversation is explicit and subject to its
current resource access. App identity is attribution, not proof of authority.

A plugin uses the host's supported bridge; it does not receive a raw gateway
token. Standalone browser and desktop targets use their supported connection
adapters. Project identity can be source-controlled; connection secrets cannot.

### 2. Use a complete chat experience or just its parts

Reuse the desktop's interaction behavior through a public conversation controller
and components, rather than exporting desktop stores, panes, or Tauri code.

```tsx
// Proposed shape, not an existing package/API.
const conversation = useConversation({ resource: dashboardRef })

return (
  <AppLayout>
    <Dashboard />
    <ConversationPanel conversation={conversation} placement="sidebar" />
  </AppLayout>
)

// A full conversation route uses the same controller.
// <ConversationPanel conversation={conversation} placement="page" />
```

The complete experience includes transcript, composer, tool activity, permission
reviews, attachments where supported, model/capability choices, stop, queueing,
and honest connection/error states. It reflects what the selected agent supports.
Applications can compose a transcript and composer separately or use headless
state and commands. Existing Nessa UI components supply styling and accessibility.

The app owns route selection and local drafts. The gateway owns accepted work.
Unmounting chat disconnects its view; it does not silently cancel that work.
Reconnect does not replay uncertain mutations. The controller should carry the
existing client semantics into the UI rather than invent another retry policy.

### 3. Share operations without choosing a UI state library

The foundation is a small operation contract: a name, description, input/output
schemas, target, and an execution binding. It has no dependency on React, a state
container, or a native window framework. React hooks and connected chat components
are conveniences on top; an existing project can use the plain TypeScript API.
The first starter remains React, without making React a requirement for calling
operations.

Two execution owners matter. A **UI action** belongs to a particular live
surface, such as selecting a chart. A **durable operation** belongs to an app
service, such as saving a dashboard. Both have typed contracts and can become
MCP tools, but their lifetimes and acknowledgements are different.

#### A UI action: select a chart in an open view

```ts
// Shared public contract; proposed APIs and schema notation.
export const selectChart = defineUiAction({
  name: "dashboards.selectChart",
  description: "Select a chart in an open dashboard view",
  input: chartSelectionSchema,
  output: selectionResultSchema,
})
```

```ts
// In a running app: view is the app's own controller, in any UI framework.
const registration = await surface.register({
  tools: [bindUiAction(selectChart, async (input) => {
    await view.selectChart(input.chartId)
    return { selectedChartId: view.selectedChartId() }
  })],
})

// A button can call the same registered action through the client.
await nessa.call(selectChart, { chartId }, { surface: surface.ref })

// On unmount, withdraw this surface's registration.
await registration.dispose()
```

The surface adapter carries this registration through the plugin host or the
standalone client's authenticated gateway connection. The gateway presents it
as an MCP tool to authorized agents and routes a call to that exact surface.
The public MCP input includes an explicit surface target supplied by the adapter,
alongside the action's input schema. The target is an identity, not a credential.
The gateway checks authority to control it. With several views, the agent must
target one; the framework does not pick an arbitrary window.

The handler returns what it actually applied. Acknowledging selection is not
proof that a frame has painted. If the view is closed or its connection is gone,
the caller receives a typed unavailable outcome; Nessa does not silently queue
the action for a future window. A lost reply after dispatch is uncertain. Surface
registration and cleanup must fence late calls from a replaced view instance.

#### A durable operation: save a dashboard title

```ts
// Shared public contract: safe to import into a browser or another client.
export const renameDashboard = defineOperation({
  name: "dashboards.rename",
  description: "Rename a dashboard",
  input: renameInputSchema,
  output: dashboardSchema,
  effect: "write",
  resource: (input) => dashboardResource(input.dashboardId),
})
```

```ts
// Server composition only: dashboards is an injected app service.
const service = await serveOperations({
  tools: [bindOperation(renameDashboard, (input, invocation) =>
    dashboards.rename(input, invocation),
  )],
})
```

```ts
// Any client: import the public contract, never the server handler.
const receipt = await nessa.call(renameDashboard, {
  dashboardId,
  title: "Quarterly revenue",
})
```

The local development launcher registers the app service with the gateway from
the approved project configuration. `serveOperations` exposes its registered
operations over MCP; `bindOperation` supplies their single app-owned
implementation. The same contract generates the tool name, description, and
input/output schemas. A UI call and a model's MCP `tools/call` dispatch to the same
handler. The developer writes no second MCP handler or manually duplicated schema.
Only explicitly registered tools are exposed; importing a function does not
publish it. Exposure does not grant access to every agent.

Here `invocation` is verified execution context, including the durable command
identity needed by the app service to recognize a retry. The backend commits its
data and receipt before acknowledging the write. The client receives a typed
receipt distinguishing a confirmed result from a refused or uncertain outcome.
Disconnecting a UI does not cancel accepted service work. The service must still
be running: durable does not mean that an arbitrary process survives a machine
failure or that the framework makes an external side effect exactly once.

The operation kit should provide schema publication, typed calls, MCP exposure,
typed failures, mutation receipts, and correlation. The gateway evaluates Cedar
policy using authenticated caller and resolved resource facts. A resource
selector supplies facts, not an authorization verdict; effect metadata does not
grant permission. The backend receives verified invocation context through a
defined trust boundary, rather than trusting a principal string from the browser.

The app's service owns business invariants and its transaction. The operation
descriptor does not put dashboard logic into the gateway. UI and agent calls
may have different authorized scope or review requirements while sharing that
implementation. Existing gateway tools are exposed only where policy permits.

For the first target, use a local action service through the existing MCP
transport where it fits. An installed app needs an explicit service lifetime
independent of a particular conversation's tool call. We must design that owner
and trusted dispatch context before treating current `mcpApps.callTool` as a
general app API. Arbitrary developer code should not execute inside the gateway.

| Caller path | Execution owner | Meaning of a successful reply |
| --- | --- | --- |
| Button or agent calls `selectChart` with a surface target | That registered live view | The handler applied and reported the selection; no persistence claim |
| Button or agent calls `renameDashboard` with a resource target | App operation service | The service committed its write and receipt |
| UI target disappears before dispatch | Gateway refuses dispatch | Typed unavailable; no later automatic dispatch |
| Either owner may have acted but its reply is lost | Owner-specific reconciliation | Uncertain; no blanket mutation replay |

Start with these two bindings and one plain client call API. Additional frontend
integrations can be written by consumers; dedicated state-library adapters are
outside this plan. The SDK should not require a second client-side execution
system just to make an existing function callable.

### 4. Keep application data shared and app-owned

A dashboard is a durable app resource, not a formatted assistant message.
The app chooses its storage and owns its records. Both agents and UI controls
modify those records through app actions. A later conversation can reopen the
same dashboard without treating its transcript as the database.

The framework should offer typed queries and resource change notifications.
An acknowledged mutation identifies its result and affected resource revisions;
views invalidate or refresh from the authoritative app service. Reconnecting
reconciles current revisions, including changes made by another view or agent.
Notification delivery alone is not durable storage or proof a view is current.

Start with one simple local persistence example and explicit refresh semantics.
Do not require a universal Nessa ORM, database, or generic record store. An app
can later replace its storage adapter without rewriting chat or its action API.

### 5. Tell the agent what the user means by “this”

Apps publish small semantic context: active resource, selected IDs, current
filters, and relevant short excerpts. Context is scoped to the app surface and
conversation, with ownership and freshness explicit when more than one view is
open. The backend reads authoritative records when acting.

```ts
// Proposed shape; disposal releases only this surface's contribution.
const context = conversation.context.attach({ surfaceId })
context.update({
  resource: dashboardRef,
  selection: [{ kind: "chart", id: selectedChartId }],
  filters: { period: "quarter" },
})
// On surface teardown: context.dispose()
```

The SDK should distinguish context held for future turns from context captured
by an admitted turn, and specify how updates interact with queued input. The
first version can adopt a simple documented capture boundary. It must not imply
that a context update rewrites an already running prompt.

Agents can request navigation or open a resource through host callbacks. The
host chooses how to present that resource. A background agent cannot assume
that a window exists, and requested navigation is separate from confirmed display.

### 6. Install and open an app independently of a tool result

The project manifest describes app identity, entry views, requested capabilities,
action service, and resource links. Installation registers that description and
obtains user approval; a manifest declaration is not executable authority.

Inside Nessa, the user can open the app from an app entry point, return to a saved
resource, show chat beside it, or open an agent-produced resource from a message.
All routes resolve stable resource references to app-owned data. Removing a
conversation does not implicitly delete a dashboard. Uninstalling code, revoking
access, and deleting app data are separate, explicit operations.

Keep MCP Apps for sandboxed plugin UI and plain MCP tools usable without rich UI.
Do not require third-party plugins to become trusted native React plugins.
Standalone apps use the same app contracts with a different presentation and
connection adapter; sandbox transport and native window APIs need not be identical.

### 7. Develop, inspect, and ship with useful feedback

The development loop should reload app code, preview plugin UI in a reference
host or Nessa, and show schema errors, missing capabilities, permission refusals,
and backend failures in actionable language. A mock gateway and fake plugin host
allow deterministic tests without making real model calls.

Diagnostics should connect a UI command or agent tool call to its gateway
admission, backend receipt, and resource revision. An app backend must participate
in this contract: going through Nessa does not automatically audit its database
transaction. Explain uncertain outcomes and keep tokens and sensitive content out
of default diagnostics.

Generated scripts should cover development, type checking, tests, and target
builds. Plugin installation into the developer's Nessa is an explicit step;
building is not publication. Releases should publish a tested, compatible set of
packages and state the supported gateway range. Exact release mechanics are a
follow-up design.

## A concrete shared interaction

This proposed sequence is the same whether a button or an agent requests a rename.
It illustrates ownership, not an implemented wire protocol.

```mermaid
sequenceDiagram
    participant Caller as App UI or agent
    participant Gateway as Nessa gateway
    participant Policy as Cedar evaluator
    participant Backend as App action service
    participant Views as Open app views
    Caller->>Gateway: Rename resource with action identity
    Gateway->>Policy: Evaluate caller, action, resource facts
    alt Refused or policy unavailable
        Gateway-->>Caller: Typed refusal; no backend dispatch
    else Allowed and any required review approved
        Gateway->>Backend: Verified scoped invocation
        Backend->>Backend: Validate business rule and commit app data
        Backend-->>Gateway: Receipt and resource revision
        Gateway-->>Caller: Acknowledged result
        Gateway-->>Views: Resource changed
        Views->>Gateway: Refresh through authorized query path
        Gateway->>Backend: Query current resource
        Backend-->>Gateway: Current resource
        Gateway-->>Views: Current resource
    end
```

If acknowledgement is lost after commit, the result is uncertain until reconciled
with the action service's durable identity/receipt. The framework cannot promise
atomicity across the gateway log and an arbitrary application database. That
boundary needs its own detailed design before shared writes ship.

## What we can reuse now

This assessment is based on the current source, not a new runtime verification.
Older design documents sometimes describe gateway work as future work; current
implementations are the baseline for this proposal.

| Existing foundation | Use it for | Missing developer-facing layer |
| --- | --- | --- |
| [`@nessa/client`](../../packages/nessa-client/README.md) | Typed gateway operations, connection recovery, conversation subscriptions | Distribution, app onboarding, app-scoped integration, headless React bindings |
| [`nessa-sdk`](../../crates/nessa-sdk/README.md) and [execution guides](../../crates/nessa-sdk/docs/agent_execution/README.md) | Execution lifecycle, queueing, controls, providers and hooks | App-facing composition over the gateway; not a second execution runtime |
| [Desktop conversation composition](../../src/desktop/dependencies.ts) and [conversation vertical](../../src/conversation/index.ts) | Real interaction behavior and gateway adapters | A supported controller/component boundary independent of desktop state |
| [Nessa UI](../../.vendor/nessa_ui/README.md) | Components, themes, transcript and composer building blocks | Connected conversation composites and app starter composition |
| [Widget contract](../../src/desktop/widgets/ui/plugin.ts) and [MCP Apps host](../../src/desktop/widgets/index.ts) | Sandbox hosting, display modes, tools, messages and model context | Installed app entry points and service/resource lifetimes beyond tool calls |
| [Local database utility](../../crates/nessa-local-database/README.md) | Existing private persistence mechanism | A useful app example; not automatically a TypeScript application database API |
| Extension authoring scaffolds | Intended browser app-shell and server-kit ownership | Their checked local source currently exports no implementation |

The extension scaffolds belong to `nessa-extensions`; UI components belong to
`nessa_ui`; gateway/client capabilities belong here. Package and repository
boundaries should follow those owners. The public product may feel like one SDK
without collapsing these responsibilities into one package.

## Delivery plan, starting now

Each milestone should produce something a developer can use. This ordering is
independent of how internal modules are being reorganized: build against public
contracts, and scope any required internal extraction separately.

| Milestone | Deliverable | Evidence that it is useful |
| --- | --- | --- |
| 1. Connected app starter | A publishable client package, minimal React web starter, explicit local gateway connection, and a reusable conversation controller/panel extracted with a real consumer | From a clean project, connect and chat with Nessa desktop closed; switch side/full chat without losing the conversation; reflect a lost connection honestly |
| 2. Shared operations and live UI actions | A plain TypeScript contract/call API, a dashboard reference app with one service query/write and one targeted UI action, MCP exposure, receipts and refresh | Button and agent calls reach the same implementations; another view refreshes; rename works with the UI closed while selection returns unavailable; lost write acknowledgement reconciles without a duplicate write |
| 3. Context and persistent plugin entry | App/resource registration, semantic context, sandboxed app launch independent of a tool result, and extension helpers | Open a saved dashboard directly in Nessa, select a chart, ask about it, close/reopen it, and retain app data independently of the conversation |
| 4. Independent desktop target | A small native starter using the same app contracts, views and gateway-backed conversation behavior | The reference app works in its own installed window without importing Nessa desktop stores or requiring the Nessa desktop UI |
| 5. Cohesive `nessa init` release | Package the proven web, desktop and plugin starters; docs, agent guidance, mocks, diagnostics and compatible releases | An unfamiliar person or coding agent follows only the generated instructions and completes the same app flow in each target |

Start milestone 1 with a hand-maintained starter. Introduce the CLI generator
once it generates a known working project; do not start by building a template
engine. Prototype the command's help and choices alongside that starter so the
intended developer journey remains visible.

For milestone 2, implement one query/write and one live UI action before a general
operation catalogue or framework adapters. For milestone 3, build on MCP Apps rather than introducing a new rich
UI protocol. For milestone 4, let the starter own native packaging; do not bundle
a second gateway by default. Complete the core reference flow before expanding
to a marketplace, arbitrary frontend frameworks, hosted multi-tenant gateways,
cross-app automation, or a direct-model execution adapter.

## First implementation briefs to write

1. **Connection and app identity:** local provisioning, supported origins,
   browser/desktop/plugin credential handling, capability discovery and revocation.
2. **Conversation controller and components:** public state/commands, ownership of
   views and drafts, sidebar/page composition, and desktop adoption without copying
   its rules. Separate visual primitives from connected behavior.
3. **Shared operation contract:** plain client calls, explicit MCP registration,
   live surface targeting versus service execution, typed schemas, verified caller
   context, Cedar resource facts, review, durable receipts and change refresh.
4. **App/resource lifecycle:** stable identities, installation and reopening,
   context capture, and what disconnect, uninstall and deletion each do.

Each brief needs its own states, failure/order table, and acceptance evidence
before implementation. They should be small enough to coordinate with current
internal restructuring without making that restructuring a prerequisite for the
entire developer experience.

## How we know the framework is ready

The reference app is the acceptance test for the product promise. It should
demonstrate the same actions and app data through plugin, web, and desktop hosts;
side chat and full chat over one conversation; inherited authorized gateway tools;
meaningful selection context; and reopening a resource after restart.

The operation examples must also work from a plain TypeScript consumer without
React or a state library. Exercise two open surfaces to verify explicit targeting,
close one to verify unavailability, and close all views while a service operation
finishes. Tool discovery and invocation must reflect current authorization and
must not publish unregistered application internals.

It also needs visible behavior when the gateway is absent, a permission is denied
or revoked, a backend is unavailable, an acknowledgement is lost, or a view
disconnects while an agent continues. Relevant component behavior is measured in
real browsers, including the desktop's WebKit engine, as required by the existing
verification standards.

The desired outcome is that developers spend their time defining their app's
capabilities and UI. Nessa supplies a connected agent experience, with clear
ownership and useful failure behavior, and leaves the app's domain and data in
the developer's hands.
