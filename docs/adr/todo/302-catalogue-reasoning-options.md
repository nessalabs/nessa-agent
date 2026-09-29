# 302. The model catalogue records each model's effort levels and fast mode

## Purpose

The desktop's thinking control needs to know, per model, which reasoning effort
levels exist and whether a fast mode does. This record puts those facts in the
SDK model catalogue as each provider publishes them, carries them to
`EffectiveCapabilities`, and says where mapping them onto one slider belongs.
It follows the "Reasoning and mode" row of
[the SDK shape](../../design/agent_execution/sdk-shape.md#how-the-superset-grows)
and replaces the placeholder tables [ADR 238](../done/238-desktop-workspace-frontend.md#the-thinking-control)
kept until the catalogue carried them.

- **Date:** 2026-09-29
- **Status:** accepted
- **Issue:** [#302](https://github.com/nessalabs/nessa-agent/issues/302)

## Context

- The catalogue recorded only `reasoning: true/false`. The desktop listed Ultra
  and Fast models by hand, and its Ultra list was a guess.
- Providers name and order their levels differently. OpenAI's GPT-5.6 models
  take `none` through `max`; GPT-6 Astra has no `none`; Claude takes `low`
  through `max` with `xhigh` between `high` and `max`; Haiku 4.5 takes a token
  budget, not a level. A shared list would invent equivalences the SDK shape
  forbids.
- No verified provider page lists a level past `max`. The slider's Ultra has no
  real model today.
- Fast mode is speed, not effort (ADR 238), and a binding may be unable to turn
  it on even where the model has it.
- Every binding declares reasoning unsupported today: none sends an effort level
  to its agent. An ACP agent can advertise its own level option at runtime
  (Codex's `reasoning_effort` session config option).

## Decision

A catalogue entry replaces `reasoning: bool` with `reasoning`, required and
either `null` (the model does not reason) or `{ "effortLevels": [...] }`: the
provider's own level names, least effort first, in its order, possibly empty
when the model reasons but no level is recorded. Beside it, `fastMode` is a
required boolean. Only verified values are recorded; an unverified model reasons
with no levels. The old boolean is not read.

In the domain, `ModelFeatures` keeps the `reasoning` flag and gains `fast_mode`,
because a binding declares the same flags as ceilings. The levels are an
`EffortLevels` value on `ModelMetadata`, allowed only where the features say the
model reasons, as `ImageInputLimits` requires image input. Each `EffortLevel` is
a lowercase letter then lowercase letters, digits, `-` or `_`, at most 32 bytes;
a list is non-empty and never repeats a name. `EffectiveCapabilities`
intersects both flags with the binding and keeps the levels exactly when
reasoning survives. The DTOs mirror the file, and an empty list maps to no
`EffortLevels`.

The SDK never maps one provider's levels onto another's. The desktop reads the
same file, as it already did, and words the names for its slider: it offers
exactly the listed levels, marks a level listed after `max` as Ultra, and shows
Fast where `fastMode` is true.

| Model `reasoning` | Binding reasoning | Effective levels | Desktop slider |
| --- | --- | --- | --- |
| `null` | either | none; `Reasoning` unsupported | chip disabled |
| `{ "effortLevels": [] }` | yes | none; `Reasoning` supported | chip disabled |
| `{ "effortLevels": [a, b, …] }` | yes | `[a, b, …]`, in order | those levels |
| any | no (every binding today) | none; `Reasoning` unsupported | still reads the catalogue |

| Model `fastMode` | Binding fast mode | Effective `FastMode` | Desktop |
| --- | --- | --- | --- |
| false | either | unsupported | no Fast toggle |
| true | no (every binding today) | unsupported | Fast toggle |
| true | yes | supported | Fast toggle |

The desktop's last column reads the catalogue, not the effective snapshot,
because nothing yet sends its choice to an agent. Each row has a test:
`effort_levels_are_the_models_own_and_go_with_reasoning`,
`restrictions_only_remove_features_for_every_boolean_combination`, and the
catalogue-driven `thinkingLevelsFor` and `responsive.mjs` checks.

**Runtime narrowing.** The catalogue is the published ceiling, as with the
context window. When a binding reads a level option its agent advertises, that
option narrows the effective levels and never widens them. The effective levels
are the catalogue's levels the agent also offers, matched by exact name and
kept in catalogue order. An agent's level the catalogue does not list is not
offered, and no match leaves no levels. This change builds none of it: it needs
a binding that sends an effort level, which none does yet, so it is follow-up
work with its own issue.

## Alternatives considered

- **One shared level list in the SDK** (low to max, then ultra). It lost because
  it invents equivalences across providers and puts the slider's look in the
  SDK.
- **A separate `reasoningEffortLevels` field beside `reasoning: bool`.** It
  lost because `reasoning: false` with levels could be written, and a
  constructor would then have to refuse it.
- **Fast mode inside `reasoning`.** It lost because fast mode is speed, and a
  model that does not reason could still have it.
- **An optional `reasoning` key, where a missing key means `null`.** It lost
  because a forgotten key would silently turn a reasoning model into one that
  does not reason.
- **Narrow by the agent's advertised options now.** Deferred, because no binding
  applies a level to narrow.

## Consequences

- Easier: a new model's levels are one edit to `models.json` with its source,
  and the desktop follows with no table of its own. The shipped levels are
  asserted in
  `shipped_catalog_records_each_models_published_levels_and_fast_mode`.
- Harder: every level name the desktop shows is worded in `composer-options.ts`.
  An unworded name shows as the provider writes it until someone words it.
  A choice carried to another model ranks only worded names, so an unworded
  one (Ultra included) opens on that model's least level.
- Ultra appears on no model until a provider publishes a level past `max`. The
  control's Ultra look is held by unit tests with a stand-in model, not by
  real data.
- Watch for: a provider renaming levels, publishing a level past `max`, or a
  binding starting to send effort. The last one is when runtime narrowing
  above gets built.
