# Nessa SDK

The first implemented slice of [ADR 0008](../../docs/adr/todo/0008-agent-client-api.md)
includes model metadata, pure effective capability snapshots, and a local Claude
ACP execution binding. See the [Claude binding guide](docs/claude-acp.md) for typed
streaming, file permissions, Stop, process supervision, setup, and verified limits.
The Conversation aggregate/coordinator, durable execution records, harness settings
readers, and gateway/UI integration remain future work.

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

The example logs JSON metadata through `tracing`, either the catalog or one exact provider/model entry.
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
There is no resolver, discovery, global snapshot, or provider I/O. A future
coordinator must install matching configuration and snapshot together and keep
accepted turns stable; this slice does not enforce lifecycle or authorization.
Steering, Stop, binding availability, and provider-specific settings are not
inferred from model facts.

## DDD layers

- `domain/model_metadata/`: one feature boundary, organized by DDD role:
  `value_objects/` groups immutable identity, capability, and description values;
  `entities/` contains the model entity; `aggregates/` contains `Catalog`, which
  protects unique identities across its model entities. Private fields and
  constructors protect valid state. These types have no serde, application, or
  infrastructure dependencies.
- `domain/effective_capabilities/value_objects/`: immutable binding restrictions,
  capability snapshot, typed requirements, and local validation errors.
- `domain/agent_execution/`: reusable message fragments, prompt text, identities,
  file-tool descriptions, and execution outcomes in `value_objects/`; `entities/`
  owns sparse tool observation merging and scoped, once-only permission resolution.
  These types are used by the adapter and can also serve another binding or a
  transcript consumer without importing ACP or application contracts.
- `application/agent_binding/`: Nessa execution ports/events and the `Agent`
  admission use case. Hosts inject sessions and authorize calls; no ACP types or
  process handles enter this layer.
- `application/`: catalog import/list/select use cases, capability projections, DTOs, and explicit
  mappings to/from domain types. Import calls domain constructors; query results
  are projections. Application errors add entry context and setup guidance.
- `infrastructure/claude_acp/`: the pinned local Claude adapter, bounded stdio
  worker, wire translation, and restricted native process supervision.
- `infrastructure/`: JSON parsing into application input DTOs, including required
  fields, unknown fields, and read errors. The host owns filesystem selection and
  injects the loaded catalog at composition.

The catalog aggregate is an immutable snapshot of model facts. It has no saved lifecycle,
so there is no repository or event machinery. Conversation aggregates and durable
command acceptance remain future work. The implemented execution port checks its
capability snapshot before provider dispatch; host authorization remains required.

Tests exercise domain invariants without JSON, application projection/import and
execution adapter substitution, JSON loading, and the Claude protocol/process
boundary. The current SDK suite has 62 tests; live provider checks are recorded
separately in the binding guide.

```text
domain/
  common/
    value_objects/
      date.rs           Date, DateError
      url.rs            Url, UrlError
      token_limits.rs   TokenLimits, TokenLimitsError
  agent_execution/
    value_objects/
      identity.rs       ExecutionId, ToolCallId, PermissionId
      message.rs        PromptText, MessageChunk, PromptOutcome
      tool.rs           ToolCallUpdate, FilePath, FileLocation, FileToolInput, content
      permission.rs     PermissionDecision, PermissionState
    entities/
      tool_call.rs      ToolCall: merge observations within one execution
      permission_request.rs  PermissionRequest: resolve once for its execution
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

Consumers import the feature and role explicitly, such as
`domain::common::value_objects::TokenLimits`. Related value objects share
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
