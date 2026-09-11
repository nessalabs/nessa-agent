# Nessa SDK

The first implemented slice of [ADR 0008](../../docs/adr/todo/0008-agent-client-api.md)
is the model metadata catalog. Conversation execution, bindings, effective
capabilities, and gateway/UI integration are still future work.

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

The example prints all metadata as JSON, or a single exact provider/model entry.
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

## DDD layers

- `domain/model_metadata/`: one feature boundary, organized by DDD role:
  `value_objects/` groups immutable identity, capability, and description values;
  `entities/` contains the model entity; `aggregates/` contains `Catalog`, which
  protects unique identities across its model entities. Private fields and
  constructors protect valid state. These types have no serde, application, or
  infrastructure dependencies.
- `application/`: catalog import/list/select use cases, DTOs, and explicit
  mappings to/from domain types. Import calls domain constructors; query results
  are projections. Application errors add entry context and setup guidance.
- `infrastructure/`: JSON parsing into application input DTOs, including required
  fields, unknown fields, and read errors. The host owns filesystem selection and
  injects the loaded catalog at composition.

The catalog aggregate is an immutable snapshot of model facts. It has no saved lifecycle,
so there is no repository or event machinery. Conversation aggregates and their
execution remain future work. When execution lands, binding/configuration
restrictions must narrow model facts before commands are accepted, and host
authorization remains required.

Tests exercise domain invariants without JSON, application projection/import
without infrastructure, and JSON parsing/file loading at the infrastructure edge.

```text
domain/
  common/
    value_objects/
      date.rs           Date, DateError
      url.rs            Url, UrlError
  model_metadata/
    value_objects/
      identity.rs       ModelProvider, ModelKey
      capabilities.rs   Modalities, ModelFeatures, TokenLimits
      description.rs    ModelDescription
    entities/
      model.rs          ModelMetadata
    aggregates/
      catalog.rs        Catalog
    error.rs
    mod.rs
```

Consumers import the feature and role explicitly, such as
`domain::model_metadata::value_objects::TokenLimits`. Related value objects share
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

`TokenLimits::context_usage_percent(used_tokens)` returns
`used_tokens / self.max_context_window * 100`. It uses the window already stored
on that instance and does not clamp results. For catalog entries, this is the
published model ceiling described above.
