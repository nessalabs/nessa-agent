# nessa-sdk server runtime research

Third-party research checked 2026-09-07 (UTC); Nessa design alignment updated
2026-09-11. This document supports
[accepted ADR 0008](../adr/todo/0008-agent-client-api.md), the authoritative design
for one reusable Rust agent runtime embedded in the server. NessaClient calls
server APIs; no TypeScript SDK is planned. The third-party examples below are
research references, not proposed Nessa client interfaces. Today the gateway and
NessaClient connection exist, with a temporary `conversation.echo()` operation.
The Rust model metadata catalog and pure effective capability snapshots are implemented; conversation execution and the
Claude ACP binding remain planned. See the [SDK guide](../../crates/nessa-sdk/README.md).

## Recommendation

Build `nessa-sdk` as the reusable Rust runtime embedded in the server. Agent
execution, selection, preflight, and authoritative lifecycle live on the server
side. NessaClient sends API requests and observes server results/events. The UI
chooses whether follow-up input queues, steers, or stops active work.

Nessa should grow into a **superset of supported features across integrations**. A shared lifecycle is the starting point, not a ceiling on functionality. Add typed operations for capabilities such as structured output, approval requests, forks, checkpoints, subagents, and artifacts as real integrations earn them. A capability being part of Nessa's API does not mean every backend can provide it. Unsupported requests must explain what is missing before work starts where possible.

Start with local ACP and a Claude agent binding. Interpret “Claude and Anthropic” here as the Claude agent using Anthropic models through an ACP adapter. A direct Anthropic Messages integration is a separate future model route. The [Claude ACP adapter](https://github.com/agentclientprotocol/claude-agent-acp) uses the Claude Agent SDK; it is not an ACP interface on the Anthropic Messages endpoint. Select and test a concrete adapter release during implementation.

## What the SDKs teach us

The comparison separates three kinds of integration:

- **Model API:** generates messages and tool-call requests. Nessa must supply an agent loop if it wants an agent built on that API.
- **Agent library:** supplies an orchestration loop inside an application or worker. Its tools, state, and lifecycle still need an owner.
- **Agent harness or service:** owns a running agent, its workspace, tools, and conversation state. Nessa connects to it and preserves its behavior.

The same company can offer more than one of these. Selecting “OpenAI” or “Anthropic” alone cannot identify the integration.

The calls below are abbreviated examples of documented interfaces, not a complete API inventory. Model names are variables deliberately: this is a study of API shape, not a model catalog. Sources are official documentation or maintainer repositories. Documentation and default-branch source can move independently of package releases; no SDK compatibility was runtime-tested in this research.

| Integration | Representative call shape | Lifecycle and configuration | Judgment for Nessa |
| --- | --- | --- | --- |
| Strands (Python agent library) | `Agent(model=model, tools=tools)`; `agent(prompt)`; `await agent.invoke_async(prompt)`; `agent.stream_async(prompt)` | An agent object supplies the loop; synchronous calls, async results, and event iteration are distinct interfaces. [Python quickstart](https://strandsagents.com/docs/user-guide/quickstart/python/), [agent interface](https://strandsagents.com/docs/api/python/strands.agent.base/) | Borrow the reusable agent/configuration idea and separate event/result views. A Nessa-owned Strands agent requires a worker and explicit state/tool ownership. |
| OpenAI Responses (model API) | `client.responses.create({ model, input, stream: true })` | Typed output items and streamed events; continuation can use `previous_response_id`. Manual history must retain relevant non-message output items. [Official TypeScript SDK](https://github.com/openai/openai-node) | Borrow object arguments and typed content. A Responses call does not give Nessa the Codex harness. |
| OpenAI Agents (TypeScript agent library) | `new Agent({ name, instructions, model })`; `await run(agent, input, { stream: true })` | Runner/model-provider configuration is separate from the agent. Results expose events, `completed`, state and interruptions; approval can pause execution and later resume that state. [Running agents](https://openai.github.io/openai-agents-js/guides/running-agents/), [streaming](https://openai.github.io/openai-agents-js/guides/streaming/) | Borrow a turn handle and correlated interactions. A paused stream is not necessarily a finished task. |
| Codex SDK (agent harness) | `codex.startThread(options)`; `codex.resumeThread(id)`; `thread.run(input)`; `thread.runStreamed(input)` | A thread carries a native agent conversation across calls; running and streaming are separate helpers. [SDK source and examples](https://github.com/openai/codex/tree/main/sdk/typescript) | Borrow conversation/turn separation. Preserve native thread identity behind Nessa IDs; do not equate it with Responses continuation. |
| Anthropic Messages (model API) | `client.messages.create({ model, max_tokens, messages })` | Request options carry model selection and message input; the result contains content blocks. [Official TypeScript SDK](https://github.com/anthropics/anthropic-sdk-typescript) | Familiar message input is useful, but this interface is not the Claude agent's workspace, tools, or session lifecycle. |
| Claude Agent SDK (Python interactive client) | `ClaudeSDKClient(options)`; `await client.query(prompt)`; `client.receive_response()`; `await client.interrupt()` | A bidirectional client controls Claude Code conversations. Interactive controls include model and permission-mode changes; their availability depends on the streaming connection. [Maintainer client source](https://github.com/anthropics/claude-agent-sdk-python/blob/main/src/claude_agent_sdk/client.py) | Borrow explicit send/receive/control operations. Expose only controls supported by the selected ACP or SDK binding; the SDK's full surface is not automatically reachable over ACP. |
| Vercel AI SDK (model abstraction and agent library) | `streamText({ model, prompt, tools })`; `new ToolLoopAgent({ model, tools })`; `agent.generate({ prompt })`; `agent.stream({ prompt })` | Rich stream results coexist with a text view; provider-specific options extend common parameters. Tool-loop agents add reusable orchestration. [streamText reference](https://ai-sdk.dev/docs/reference/ai-sdk-core/stream-text), [ToolLoopAgent](https://ai-sdk.dev/docs/reference/ai-sdk-core/tool-loop-agent) | Borrow typed options and rich events. Keep extensions validated; do not copy arbitrary provider options into Nessa's domain. |
| OpenCode SDK (agent service client) | `createOpencode(...)` or `createOpencodeClient({ baseUrl })`; `client.session.create(...)`; `client.session.prompt(...)`; `client.event.subscribe()` | One factory starts a server/client pair; the other connects to an existing server. Sessions and event subscriptions have separate APIs; the SDK is generated from the server specification. [Official SDK guide](https://opencode.ai/docs/sdk/) | A useful model for Nessa's facade. Keep worker ownership separate from connecting a surface, and scope shared events to the correct conversation/turn. |
| ACP (agent protocol) | `initialize`; `session/new`; `session/prompt`; `session/update`; `session/cancel` | Negotiated capabilities, native sessions, streamed updates, and reverse permission requests form a stateful protocol. [Prompt lifecycle](https://agentclientprotocol.com/protocol/v1/prompt-turn) | First binding transport. Nessa still owns its public API and durable event envelope. |
| Kimi Code (agent harness) | `kimi acp`, then ACP session methods | The documented CLI exposes JSON-RPC over stdin/stdout with its own capability matrix and optional extensions. [Official ACP reference](https://www.kimi.com/code/docs/en/kimi-code-cli/reference/kimi-acp) | Another future ACP binding, distinct from calling a Moonshot/Kimi model API. Negotiate the installed release; do not assume the Claude binding's features. |
| DeepSeek (model APIs) | Chat Completions accepts `{ model, messages, stream }`; Responses accepts `{ model, input, ... }` | Separate documented model endpoints and model-specific controls. Tool calls require the consuming application to handle tool execution. [Chat API](https://api-docs.deepseek.com/api/create-chat-completion/), [Responses API](https://api-docs.deepseek.com/api/create-response/), [tool calls](https://api-docs.deepseek.com/guides/tool_calls/) | Future direct model adapter or model choice inside another agent. Familiar wire syntax does not imply interchangeable reasoning, tools, state, or cancellation. |

### A closer look at OpenCode

The documented SDK form uses nested `path` and `body` arguments:

```ts
const result = await client.session.prompt({
  path: { id: sessionId },
  body: {
    model: { providerID, modelID },
    parts: [{ type: "text", text: prompt }],
  },
});
const events = await client.event.subscribe();
for await (const event of events.stream) handle(event);
```

This illustrates why integration provider and model provider should be separate concepts. OpenCode also exposes `session.promptAsync` and `session.abort` in its [generated SDK source](https://github.com/anomalyco/opencode/blob/dev/packages/sdk/js/src/gen/sdk.gen.ts). An asynchronous prompt acknowledgement is not completion; event correlation and terminal status remain necessary. Use one matching server/SDK release when implementing, rather than mixing these documented nested arguments with a different generated API version.

### State and stopping differ across libraries

Strands documents a `cancel_signal` for invocation methods in its [agent loop guide](https://strandsagents.com/docs/user-guide/concepts/agents/agent-loop/). OpenAI Agents uses run cancellation and exposes resumable state; its [session guide](https://openai.github.io/openai-agents-js/guides/sessions/) separately describes history persistence. Claude's interactive client has an interrupt command, while OpenCode has a session abort endpoint. These are related controls, but their cleanup, resumption, and persistence guarantees are not identical. Nessa must define its own observable outcomes and test each mapping.

No source reviewed establishes universal cross-provider resume or durable replay of every streamed event. Those guarantees must come from Nessa's implemented contracts and its chosen stream dependency. Familiar SDK signatures are evidence for ergonomics, not evidence that all their execution semantics match.

## Selection has several independent parts

| Concept | Meaning | Example, not a fixed identifier |
| --- | --- | --- |
| Nessa gateway | Where this client authenticates and sends product operations | Local Nessa server |
| Integration provider | How the server reaches the agent | `acp`; later a registered SDK worker or service adapter |
| Agent | The harness or configured agent to run | Claude agent, OpenCode, Kimi Code, a Nessa-owned agent |
| Model provider | The inference service used by that agent, if exposed | Anthropic, OpenAI, DeepSeek, Moonshot |
| Model | An exact provider/model entry in the injected metadata catalog | A configured OpenAI or Anthropic model ID |
| Binding | One installed integration for one agent, with its actual capabilities | Server-owned binding ID |
| Execution profile | Server-owned workspace, process, credentials, and permitted settings | A local workspace profile |

The host configures available agent bindings and their model selection. The model
catalog is separate from the integration registry: its provider identifies the
inference service, not an ACP transport. Selection uses an exact provider/model
key from the immutable JSON catalog. A missing entry is a configuration error;
it does not trigger provider discovery. Binding adapters translate configured
selection into their native controls. ACP does not guarantee a universal
vendor/model pair. Missing or ambiguous workspace/profile context fails explicitly.

Backend factories remain selected and injected at server composition. A request chooses among already registered bindings; it cannot construct an arbitrary backend. This preserves [typed dependency injection](dependency-injection.md), including its rule against choosing infrastructure implementations from request parameters. Future bindings are added explicitly, without a global service locator.

## Our own reusable runtime: nessa-sdk

Build `nessa-sdk` as a Rust agent runtime library embedded in the server.
NessaClient calls the authenticated server API, whose handlers invoke the SDK. Another authorized Rust host can embed the library
with its own injected adapters. The crate currently implements model metadata and effective capabilities;
the execution architecture below remains planned.

```mermaid
flowchart TD
    UI["Panel, CLI, or another surface"] --> Client["NessaClient<br/>Server API calls"]
    Client --> Gateway["Nessa gateway<br/>Authentication and product authorization"]
    Gateway --> SDK["nessa-sdk<br/>Agent runtime, lifecycle, and commands"]
    Host["Other Rust host<br/>Own application authorization"] --> SDK
    SDK --> Bindings["Injected bindings<br/>ACP first; workers and model adapters later"]
    SDK --> Records["Injected committed record port"]
```

| Component | Owns |
| --- | --- |
| UI/orchestrator | Pickers, presentation, pending input, and queue/steer/stop decisions |
| NessaClient | Server API calls, request IDs, authentication, connection/reconnection, event observation, and remote command reconciliation |
| Nessa gateway | Wire translation, authenticated caller context, and Nessa product authorization before SDK invocation |
| nessa-sdk | Selection/preflight through bindings, effective run configuration, conversation/turn admission and lifecycle, execution coordination, normalized events/results, durable receipts and recovery semantics |
| Host composition and adapters | Concrete bindings, credentials, OS facilities, optional isolation implementations, storage backends, and owned resource startup/shutdown |

The SDK owns typed application ports and DTOs. Host composition injects bindings,
persistence/events, content, time, and required execution facilities. Host
application entry points enforce resource/action authorization before SDK access;
the SDK receives verified action context, not policy grants or an auth interface.
Domain rules do not depend on transport, UI, provider SDKs, or application DTOs.
Each runtime instance is independent; no global backend or service locator is
introduced. Tests can replace the ports without sockets or real agent processes.

Gateway commands invoke SDK application operations. The gateway does not maintain
a second turn coordinator. Durable admission, receipts, lifecycle transitions,
and recovery have one owner in nessa-sdk; concrete persistence uses the planned
external stream dependency. SDK event observers and client projections do not
become competing sources of truth.

NessaClient shares its existing authenticated connection for API calls and event
subscriptions. Direct Rust embedding enforces access policy and supplies trusted action context
through its own application boundary. No TypeScript SDK is planned.

External ACP harnesses keep their own loops and tools. For Nessa-owned agents,
the runtime can coordinate model calls and tools through injected adapters.
Isolation requirements and cleanup policies belong to execution contracts;
concrete OS/container implementations belong to hosts or optional adapters.
Do not add all future engines or sandbox implementations in the first ACP slice.

## Server operation contracts

The planned server API exposes configured capabilities, conversation creation/retrieval,
turn submission, steering, Stop, interaction responses, and status/event/result retrieval.
The Rust SDK owns their execution semantics. Wire names follow ADR 0008.

Creation resolves registered agent/model selection and workspace/profile context
inside the SDK, including preflight and effective configuration. Missing or
ambiguous context fails explicitly. NessaClient handles request bookkeeping;
the gateway supplies authenticated context and applies product authorization.

Conversation identity, admitted turn identity, mutation request identity, and
replay cursor remain separate. Mutation IDs survive identical retries and
reconciliation. The server commits admission before acknowledging success and
never treats reconnecting as permission to rerun an uncertain prompt.

Steering targets one active turn and may be unsupported. A steering receipt does
not prove model consumption. Stop acknowledgement does not prove completion
or roll back tool effects. Closing a client connection does not stop execution.
Server state and committed records are authoritative; client projections identify
stale state and stream gaps. Retrieval/replay does not implicitly restart or
resume a provider process.

The UI owns its follow-up queue and decides when to submit it. The server admits
one active turn atomically and rejects competing new turns with `turn_busy`.
Stop-and-send waits for both the terminal outcome and confirmed binding readiness.
Failed cleanup keeps the draft and shows the error. Peer inbox `next_turn` delivery is
a separate contract and does not authorize starting a future turn. See
[surfaces and collaboration](surfaces-and-collaboration.md).

Detailed durability, cleanup, delivery, and acceptance requirements live in
[ADR 0008](../adr/todo/0008-agent-client-api.md).

## Creator and surface metadata

ADR 0008 includes immutable conversation creator/surface provenance, separate
provenance on each turn and control action, and an extensible `SurfaceId` with
well-known Nessa constants generated from the shared protocol. Actor identity is
derived from trusted context and claimed surface namespaces are validated.
`surfaceId` identifies the stable surface; `surfaceInstanceId` identifies an
individual registered instance. The UI owns visibility and grouping; authorization
continues to govern access independently of origin filters. See
[the ADR provenance contract](../adr/todo/0008-agent-client-api.md#creator-and-surface-provenance)
for delivery and validation requirements.

## How the superset grows

Expose a small lifecycle plus typed feature operations as real integrations need
them. Model metadata uses required booleans for supported/unsupported features.
The implemented factory combines the selected entry with binding restrictions and
configured agent settings into one immutable `EffectiveCapabilities` snapshot.
Restrictions can disable features; they cannot enable a model feature marked
false. The UI reads that snapshot and SDK commands validate against it locally.
Host authorization and conversation lifecycle checks remain separate decisions.
The catalog's context window is the published model ceiling; a harness's configured
window may be narrower. The implemented catalog does not read harness settings.

| Feature family | Nessa representation to grow toward | What must stay honest |
| --- | --- | --- |
| Messages and multimodal input | Typed text, image, file/resource parts and output content | Reject unsupported parts; do not silently drop them |
| Tools and approvals | Tool lifecycle events plus correlated interaction requests/responses | External harnesses keep tool execution and approval semantics; Nessa policy can further restrict access |
| Questions and elicitation | Typed interaction variants, including structured forms where supported | Asking a question is distinct from authorizing a tool |
| Output schema | Typed structured-output request and validated result | Native support and Nessa-owned validation/retry are different mechanisms with different costs |
| Reasoning and mode | Advertised options and effective configuration | Do not invent equivalent effort levels across providers or fabricate hidden reasoning |
| Memory and recovery | Transcript reading, native resume, fork, and checkpoint as separate capabilities | A replay cursor is not an agent checkpoint; transcript copying is not native resume |
| Subagents and handoffs | Child identities, relationships, lifecycle and delegated output | Preserve provider ownership; orchestration features are not universal |
| Files and artifacts | Authorized resource references, edits, citations, provenance | Provider paths/URLs need translation and access checks, not blind forwarding |
| Usage, budgets, tracing | Reported usage and typed controls with source/availability | Hard enforcement and best-effort estimates must be distinguishable |
| Provider-specific behavior | Named, schema-validated extension operations and event payloads | No raw SDK object, arbitrary method invocation, or unvalidated options bag |

Promote a feature to a common API when its meaning can be stated and tested across relevant bindings. Preserve distinct semantics through typed variants when they differ. An adapter can expose a namespaced feature before it becomes common. Extensions still use Nessa authorization, validation, attribution, and event normalization. They are not a tunnel around the gateway.

Rust bindings use typed configuration and declared support. Required protocol setup does not populate or override model metadata. Commands perform no capability discovery or negotiation. Future extension schemas must be validated at the boundary and mapped to application-owned DTOs. Do not create a universal `Record<string, any>` or ship empty adapters for every vendor now.

## ACP mapping for the first slice

The [ACP prompt lifecycle](https://agentclientprotocol.com/protocol/v1/prompt-turn) separates setup, prompt submission, session updates, permission interaction, and terminal completion. Map that lifecycle into Nessa's turn model. A stream of text alone loses tool progress and user interaction.

[ACP session configuration](https://agentclientprotocol.com/protocol/v1/session-config-options) advertises option IDs, values, ordering, and current state. Prefer `configOptions`; use `session/set_config_option` to apply supported choices and consume the full returned configuration. Option categories help presentation but are not required for correctness. Do not assume an option is literally named `model` or that its value is a portable model ID. The binding maps the configured model selection to the actual native option. These protocol values do not replace Nessa model metadata. Implement only the selected adapter contract; compatibility support requires an explicit request.

Install configuration and its capability snapshot at a serialized command boundary. Keep them stable for an accepted turn; changes apply to future work. Record requested and effective settings. If the configured integration cannot deliver the selected configuration, return an explicit error; do not substitute another model or provider.

The concrete Claude ACP binding must pass a compatibility probe before it is advertised as ready: initialize, authentication/setup, session creation, advertised configuration, prompt/update completion, permission response, cancellation, and process cleanup. Test resume only if advertised. Pin the selected adapter/protocol dependency during implementation and record its supported optional extensions. This research does not establish compatibility with every release.

## Ownership behind the facade

```mermaid
flowchart LR
  Surface[Panel, CLI, or application] --> Client[NessaClient]
  Client --> Gateway[Authenticated Nessa gateway]
  Gateway --> Coordinator[nessa-sdk conversation and execution application]
  Coordinator --> Binding[Injected agent binding]
  Binding --> ACP[Local ACP agent]
  Binding -. future .-> Worker[SDK worker or service]
  Coordinator --> Stream[External committed stream dependency]
  Stream --> Gateway
```

NessaClient calls nessa-sdk through the authenticated gateway. The gateway owns product authorization and wire translation. The pure conversation aggregate decides transitions and returns domain events. The application commits them, applies committed state, then starts effects; replay applies records without agent or tool calls. The SDK application coordinates conversation/turn identities and admission; NessaClient receives the observed state through the API. Binding adapters feed provider events into Nessa's Rust normalizer, which produces the canonical event payloads defined by the shared schema. The external stream dependency owns committed ordering and replay under [ADR 0009](../adr/todo/0009-reusable-event-stream-crate.md). Use its real API when available; do not implement a temporary second stream runtime or claim that today's `client.on()` replays missed events.

Rust cannot directly import a TypeScript or Python agent SDK. A future SDK integration may use a supervised worker with a typed protocol, or call an existing service through an adapter. Composition owns startup/shutdown, bounded calls, crash handling, and credentials. Browser clients do not spawn agent processes or receive model secrets. All local use remains possible without hosted Nessa signup.

External harnesses retain their prompts, tools, config files, credentials, approval behavior, and agent loops under [ADR 0012](../adr/todo/0012-agent-harnesses-and-optional-tools.md). Nessa-owned agents may use Strands, OpenAI Agents, Vercel, or model APIs later. That is an explicit integration, not an accidental replacement for Claude Code, Codex, OpenCode, or Kimi Code.

## Alternatives and judgment

| Option | Judgment |
| --- | --- |
| Copy `chat.completions.create()` as the whole API | Familiar for inference, but too narrow for long-running sessions, tools, permission requests, and replay |
| Expose each provider's SDK directly | Full access initially, but leaks runtimes, credentials, event schemas, and lifecycle into every surface |
| Make ACP the public Nessa API | Good first transport; tying the public contract to it would constrain richer future integrations |
| A generic `invoke(provider, method, args)` | Easy routing, weak types, unclear authority, and no dependable common behavior |
| Build all SDK adapters before the first turn | Delays learning and invents unused dependencies; retain their requirements in the design instead |
| Shared conversation lifecycle plus typed capabilities | Recommended: familiar calls, one gateway boundary, and room for features beyond ACP |

## Delivery and validation

The metadata foundation is complete: strict JSON loading, validated domain values,
exact selection, and immutable catalog queries are implemented with 22 tests and
Rust 1.89 support. Pure effective capability construction and validation are also
implemented, bringing the SDK to 36 tests. Host wiring and harness settings readers
remain planned.

1. Verify one concrete Claude ACP adapter release and the external stream library's
   real local SQLite adapter. Test required controls, process cleanup, atomic
   acceptance records, append retries, and reads after restart before building on them.
2. Deliver one complete conversation through the SDK, authenticated gateway,
   NessaClient, and panel. Add the required aggregate, injected ports, the existing effective
   capability snapshot, saved receipts/events, interactions, Stop, retrieval, and
   origin metadata. Follow ADR 0008 and the generated protocol for wire names;
   retain `requestId` as the mutation identity and add no aliases or version bumps.
3. Verify recovery, races, and reuse: lost replies, duplicate/conflicting request IDs,
   replay without effects, storage failures, slow subscribers, process failure,
   cleanup, and two isolated SDK instances. Test host authorization at every entry
   point separately from SDK capability and lifecycle validation. Stop-and-send
   requires terminal state and confirmed binding readiness; cleanup failure retains
   the draft. Unsupported steering starts no replacement work.
4. Add another binding when needed. OpenCode or Kimi ACP can test harness variation;
   a later direct SDK/service integration tests transport variation. Test adapters
   prove substitution, not production-provider support. Add feature contracts when
   a real integration needs them.

The open implementation choices are the concrete Claude adapter release, which of its optional capabilities to ship first, and the external stream crate's actual API. Supporting all providers and every feature is a direction for incremental work, not the acceptance criterion for the first ACP slice.
