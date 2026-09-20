# Nessa SDK

The first implemented slice of [ADR 0008](../../docs/adr/todo/0008-agent-client-api.md)
includes the public Agent entry point, session storage, hooks, queueing/steering,
idempotent retries, model metadata, and effective capability snapshots. Three local
ACP adapters implement the execution provider port, one each for Claude, Codex
and Opencode, over one shared runtime. See the [execution guides](docs/agent_execution/README.md) for lifecycle, permissions,
prompts, and transport; the [Claude guide](docs/claude-acp.md) covers provider setup
and verified native limits, and the [Codex guide](docs/codex-acp.md) and
[Opencode guide](docs/opencode-acp.md) cover what is each of those providers'
own.
Shared conversation coordination and its durable event stream, harness settings
readers, and gateway/UI integration remain future work; local session snapshots
are already implemented.

## Model data

[data/models.json](data/models.json) is the single catalog, checked against official
provider documentation on **2026-09-11**. It contains the current general-purpose
OpenAI lineup (GPT-6 Astra, GPT-5.6 Sol, Terra, Luna) and Anthropic lineup (Claude
Fable 5.1, Opus 5, Sonnet 5, Haiku 4.5). Each entry links its source. The lineup is
selected from the [OpenAI catalog](https://developers.openai.com/api/docs/models)
and [Claude catalog](https://platform.claude.com/docs/en/models/overview).

OpenAI is the provider key for the models behind ChatGPT; Anthropic is the provider
key for Claude. IDs are exact API model IDs. ChatGPT/Claude subscription access,
ACP harness availability, and permissions are not inferred from this data. Older
generations, convenience aliases, and specialized image/audio/embedding models
are excluded. Haiku 4.5 remains the latest Haiku despite its older release date.

`imageInput` is the one place image limits live: accepted media types, the largest
single image as base64 text, the longest edge accepted at all, the lower edge
ceiling a provider applies once a request holds many images (a conversation
resends its earlier ones, so a long one reaches it), and the long edge the model
actually sees. Each figure is the strictest published across the platforms
serving the model: Claude's come from the
[vision guide](https://platform.claude.com/docs/en/build-with-claude/vision),
where Amazon Bedrock and Google Cloud accept 5 MB where the direct API accepts
10 MB. Whatever prepares an image for a model reads these and carries no numbers
of its own. A model that lists image input without recorded limits, as the OpenAI
entries do today, is offered no images: nothing could prepare one for it.

Entries expose identity, display name, input/output modalities, tool use,
reasoning support, context window, standard maximum output, knowledge cutoff,
and documentation URL. Required features are booleans, including explicit false
values. Tool-mediated image generation does not mean native image output.
Reasoning support does not prescribe provider-specific effort parameters.
Knowledge cutoffs retain the precision each provider publishes. Token limits are
descriptive model ceilings. `maxContextWindowTokens` is not a runtime default.
Codex can configure its context window through local config/user settings; a
future binding must keep that configured value separate and restrict the effective
window to the model ceiling. This metadata slice does not read those settings.
For budgeting, the context window is not a maximum input allowance,
and reasoning/output share the provider's applicable budgets. Account quotas,
pricing, API request options, and beta/batch extensions are outside this catalog.

## Load and inspect

From the repository root:

```sh
cargo run -p nessa-sdk --example models -- crates/nessa-sdk/data/models.json
cargo run -p nessa-sdk --example models -- crates/nessa-sdk/data/models.json openai gpt-6-astra
cargo test -p nessa-sdk
```

The example writes JSON to standard output, either the catalog or one exact
provider/model entry. Diagnostics use tracing on standard error, so output can be
piped directly to a JSON reader.
It needs no credentials and performs no provider requests.

Host composition opens its selected file and calls
`infrastructure::model_metadata_json::load_catalog(reader)` once at startup. Pass the
resulting `application::model_catalog::ModelCatalog` by ownership or `Arc` to
consumers. Applications can import typed DTOs with `ModelCatalog::from_metadata`, or inject
a validated domain `model_metadata::aggregates::Catalog` into `ModelCatalog::new`. All construction paths
pass through the same domain invariants. The SDK neither
chooses a filesystem path nor embeds a second copy or a fallback catalog.

The domain catalog exposes immutable borrowed entities. The application returns
owned DTO projections in file order through `models()` and exact selection through
`select(provider, model_id)`; changing a DTO cannot mutate the catalog. Invalid JSON, missing or
unknown fields, malformed data, impossible limits, and duplicate provider/model
keys fail loading. Missing selection reports an error asking for an entry to be
added. Each application has its own snapshot. Edit the file and restart its
consumer to update data; there is no watcher, resolver service, or discovery.

## Effective capabilities

`domain::effective_capabilities::value_objects::EffectiveCapabilities::new`
accepts a selected `ModelMetadata`, typed `BindingRestrictions` (declared features
and token ceilings), and configured `TokenLimits`. Reuse the validated model from
the domain catalog; a selected application metadata DTO can also be mapped through
`ModelMetadata::try_from`.

The factory intersects model and binding input/output modalities, tool use, and
reasoning support. A binding can remove support but cannot enable a model-false
feature. No shared input or output modality is a setup error. Explicit configured
context/output limits above either ceiling fail with `ConfiguredLimitExceeded`;
valid smaller limits are retained exactly. The snapshot owns its model identity,
features, and limits independently of other selections.

For a model already selected from the catalog:

```rust
use nessa_sdk::application::dto::EffectiveCapabilitiesDto;
use nessa_sdk::domain::effective_capabilities::value_objects::{
    BindingRestrictions, CapabilityRequirement, EffectiveCapabilities, Modality,
};
use nessa_sdk::domain::common::value_objects::TokenLimits;
use nessa_sdk::domain::model_metadata::value_objects::{Modalities, ModelFeatures};

// `model` is a borrowed ModelMetadata. These limits must fit that model.
let text = Modalities::new(true, false, false)?;
let binding = BindingRestrictions::new(
    ModelFeatures::new(text, text, true, false),
    TokenLimits::new(100_000, 16_000)?,
);
let snapshot = EffectiveCapabilities::new(
    model, binding, TokenLimits::new(80_000, 8_000)?,
)?;
snapshot.validate(&[CapabilityRequirement::Input(Modality::Text)], 12_000, 4_000)?;
let view = EffectiveCapabilitiesDto::from(&snapshot);
```

`validate(requirements, input_tokens, output_tokens)` checks typed
`CapabilityRequirement` values, rejects unsupported content explicitly, and
requires a positive output budget within the effective output ceiling. Total
input plus reserved output must fit the effective context window. The caller
supplies all requirements and token counts; validation does not inspect content.
Counts must cover history, tools, and other input and include
reasoning in the applicable output budget. This is local budget validation, not
a tokenizer or provider-exact accounting. An empty requirements slice means no
feature requirement; it does not validate message structure.

`application::dto::EffectiveCapabilitiesDto::from(&snapshot)` projects the same
snapshot for future UI consumers. Changing that DTO cannot change validation.
This model-capability value performs no resolver, discovery, global-state, or
provider I/O work. Agent separately exposes negotiated
[operation capabilities](docs/agent_execution/agent.md#model-capabilities-and-provider-operations). A future
gateway coordinator must install matching configuration and snapshot together
and keep accepted turns stable. The Agent and execution domain enforce local
lifecycle rules; the host remains responsible for authorization.
Native steering, context restoration, binding availability, and provider-specific
settings are not inferred from model facts.

See [the Agent entry point](docs/agent_execution/agent.md) for provider selection,
automatic session storage, hooks, invocation, and UI integration.

## DDD layers

- `domain/model_metadata/`: one feature boundary, organized by DDD role:
  `value_objects/` groups immutable identity, capability, and description values;
  `entities/` contains the model entity; `aggregates/` contains `Catalog`, which
  protects unique identities across its model entities. Private fields and
  constructors protect valid state. These types have no serde, application, or
  infrastructure dependencies.
- `domain/effective_capabilities/value_objects/`: immutable binding restrictions,
  capability snapshot, typed requirements, and local validation errors.
- `domain/agent_execution/`: feature modules `sessions`, `executions`, `tools`,
  `permissions`, and `prompts`, with DDD roles beneath each feature. Sessions own
  live aggregate state; tools own immutable patches/snapshots and identity-bearing
  observations; permissions own once-only decisions/cancellations; prompts own
  attributed system instructions. Public imports name the feature explicitly.
- `application/agent_execution/`: `agents` exposes `Agent` and its errors,
  `providers` injected execution ports, `sessions` automatic snapshot management,
  `hooks` typed callbacks registered on Agent,
  `executions` the request/controller/event projections and mandatory execution audit port,
  `permissions` attribution and answer/cancellation evidence, and `tools` the original review input. The controller
  pairs input with accepted requests, bounds retention, and projects domain state.
  No provider JSON or process handles enter this layer. See the
  [execution guides](docs/agent_execution/README.md) for the current contracts.
- `application/`: catalog import/list/select use cases, capability projections, DTOs, and explicit
  mappings to/from domain types. Import calls domain constructors; query results
  are projections. Application errors add entry context and setup guidance.
- `infrastructure/claude_acp/`: Claude settings, model limits, system-prompt extension, and tool schemas.
- `infrastructure/codex_acp/`: Codex's harness identity, its approval preset, and the
  content normalization that keeps its terminal and resource-link updates inside the
  shared vocabulary.
- `infrastructure/opencode_acp/`: Opencode's recorded profile, the permission policy
  its launch pins, and the kind-only naming its tool wire records a review under.
  A fourth agent gets a fourth sibling here, not a branch inside one of these three.
  Shared ACP exchange, JSON-RPC framing, and process supervision live in
  `infrastructure/acp`, `infrastructure/json_rpc`, and `infrastructure/process.rs`,
  and so does verification more than one provider needs — ordered session
  configuration moved up from `codex_acp/` to `acp/sessions/configuration.rs` when
  Opencode turned out to need the same two options.
- `infrastructure/session_storage/`: in-memory snapshots and private JSONL session storage,
  exclusive leases, and explicit JSON evidence mapping.
- `infrastructure/`: JSON parsing into application input DTOs, including required
  fields, unknown fields, and read errors. The host owns filesystem selection and
  injects the loaded catalog at composition.

The catalog aggregate is an immutable snapshot of model facts. It has no saved lifecycle,
so it needs no repository or event machinery. Execution sessions and invocation
queues already enforce their local lifecycle and scheduling invariants. Agent saves
admission and settlement in local snapshots and checks capabilities before dispatch.
Shared gateway conversation coordination and a durable command stream remain future
work; host authorization is required today.

Tests exercise domain invariants without JSON, application projection/import and
execution adapter substitution, JSON loading, and the Claude protocol/process
boundary. Domain session, shared transport, and provider substitution tests run without
a live provider; live checks are recorded separately in the binding guide.

```text
domain/
  common/
    value_objects/
      date.rs           Date, DateError
      url.rs            Url, UrlError
      token_limits.rs   TokenLimits, TokenLimitsError
  agent_execution/
    sessions/
      aggregates/execution_session.rs
      value_objects/identity.rs
    executions/
      aggregates/invocation_queue.rs
      value_objects/identity.rs
      value_objects/message.rs
      value_objects/scheduling.rs
      value_objects/transition.rs
    tools/
      entities/tool_call.rs
      value_objects/identity.rs
      value_objects/tool.rs
    permissions/
      entities/permission_request.rs
      value_objects/identity.rs
      value_objects/permission.rs
    prompts/
      builders/system_prompt_builder.rs
      value_objects/prompt.rs
  model_metadata/
    value_objects/
      identity.rs       ModelProvider, ModelKey
      capabilities.rs   Modalities, ModelFeatures
      description.rs    ModelDescription
    entities/
      model.rs          ModelMetadata
    aggregates/
      catalog.rs        Catalog
    error.rs
    mod.rs
```

Consumers import execution concepts through feature exports, such as
`domain::agent_execution::tools::ToolCall`. Shared primitives keep their own boundary,
for example `domain::common::value_objects::TokenLimits`. Related value objects share
files; domain features do not accumulate in a flat namespace.

Each `mod.rs` is a module guide with plain-English context and ASCII diagrams,
plus module declarations and exports. Implementations live in named files.

Shared calendar validation lives in `domain/common/value_objects/Date`. Both
knowledge cutoffs and verification dates use it; the catalog separately requires
day precision for verification. Common values never depend on model metadata.

`Date` delegates calendar validation to `chrono` (clock features disabled).
`Url` delegates absolute URL parsing and normalization to the `url` crate and
provides `Url::validate`. Model documentation requires HTTPS; the shared URL
value itself supports other schemes. DTOs carry strings; domain descriptions
hold the validated values. Neither value performs network or clock I/O.

`ModelKey` holds a closed `ModelProvider` enum (`OpenAi` or `Anthropic`).
Application mapping accepts only `openai` and `anthropic` from DTOs. Unknown
providers and aliases fail import/selection; adding another provider requires an
explicit domain change. Model IDs remain provider-scoped strings.

`TokenLimits` and `TokenLimitsError` belong to the common domain. Positive
context/output ceilings and output bounded by context apply equally to published
metadata, binding declarations, and configured execution limits. Invalid metadata
imports wrap the common error with metadata context; binding configuration uses
the common error directly.

`TokenLimits::context_usage_percent(used_tokens)` returns
`used_tokens / self.max_context_window * 100`. It uses the window already stored
on that instance and does not clamp results. For catalog entries, this is the
published model ceiling described above.

## Domain coverage gate

Run from the repository root with `cargo-llvm-cov 0.6.16` and the current
Rust toolchain's `llvm-tools-preview` component installed:

```sh
bash scripts/check-sdk-domain-coverage.sh
```

The script runs every SDK test, then requires **100% lines, functions, and
regions for all SDK domain source**, including common values, model metadata,
and effective capabilities. Application, infrastructure, test, and example files
are excluded from this domain threshold. It uses a fresh temporary target and
removes only that directory; the normal/shared build target is untouched.
This stable-toolchain measurement does not report branch coverage.

Measured after the metadata/capability slice: **36 SDK tests passed**, and domain coverage is
**364/364 lines, 60/60 functions, and 445/445 regions**. Effective capabilities
contributes 107 lines, 10 functions, and 161 regions, all covered. The application
mapping file, including the new snapshot projection, separately measures
55/55 lines, 5/5 functions, and 103/103 regions; it is not part of the domain gate.
Tests cover all 2,401 independent nonempty model/binding input/output modality
combinations, boolean feature restrictions, configuration and input budget
boundaries, error diagnostics, and snapshot isolation.

Shared ACP execution infrastructure lives in `infrastructure/acp`, with provider
profiles injected by `infrastructure::claude_acp::sessions::ClaudeAcpProvider`,
`infrastructure::codex_acp::sessions::CodexAcpProvider` and
`infrastructure::opencode_acp::sessions::OpencodeAcpProvider`.
`infrastructure::acp::sessions::AcpConfig` supplies
common launch and runtime limits. JSON-RPC framing reuses the pinned event-stream
codec; process ownership lives in `infrastructure/process.rs`. See the
[session and boundary guide](docs/agent_execution/lifecycle.md).

Every provider's `new` requires an injected `ExecutionAudit`. The
host chooses its storage and durability contract. Answer records retain exact
decisions and attribution before wire effects, then their delivery observation.
Cancellation records retain the
original request, review input, reason, and client/provider/runtime origin.
Explicit `Agent::close` calls require an `ActionContext`. Audit delivery is
independent of the UI event reader, so dropping that reader does not discard
cancellation evidence. The smoke example supplies an explicitly non-durable
tracing sink and omits raw tool arguments from its output.

The same `Session` client automatically restores a closed context before its next
execution, using `session/resume` after verified cleanup. It never replaces missing
history with a new conversation. Each restored connection gets fresh domain and
wire state; permission IDs remain unique across those connections.
`Agent::cancel_permission` supports attributed guard withdrawals with validated
custom reason codes and explanations. See the guide's lifecycle and audit tables.

Tool values have separate roles: `ToolCallUpdate` is immutable sparse input
(`None` omits a field, an explicit empty value clears it); `ToolObservation`
is an immutable snapshot (`None` means not yet observed). `ExecutionSession`
constructs and updates its `ToolCall` entities after checking execution identity
and session state. Callers borrow tools and may clone their observation snapshots.
Providers build independent review snapshots with the consuming
`ToolObservation::with_update` replacement operation; this carries no identity or
session mutation authority. Unchanged payloads move without copying. Permission
review events carry a separate `tool_id` and captured `observation`; later tool
updates cannot change that review snapshot. These types describe provider-run
tools; they do not define or execute tools.

`PermissionScope::request`, `session`, and `application` construct immutable
scope values from validated identities. `PermissionScope::view` borrows those
identities for inspection; changing scope requires a replacement value.
Cancellation causes likewise expose immutable payloads through
`PermissionCancellationReason::view`. A request owns its resolution state;
`PermissionRequest::state` returns a borrowed `PermissionStateView` that keeps the
selected option and decision tied to that request.

Queue follow-ups with `Agent::enqueue`, prioritize a later correction with
`enqueue_steering`, or use advertised native injection with `steer`. Pending
inputs can be withdrawn through `remove_queued` without deleting their evidence.
Read `queued_ids` and use `reorder_queued` to change their complete order within
each priority class while keeping their IDs, receipts and audit history.
See [queueing and steering](docs/agent_execution/scheduling.md) for lifecycle,
audit, and provider capability guarantees. Gateway wiring remains separate.
