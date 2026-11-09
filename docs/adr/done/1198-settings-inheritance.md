# 1198. Resolve typed settings from broadest scope to nearest override

## Purpose

Give settings one inheritance rule as Nessa adds scopes. A missing choice uses
its parent's effective value; an explicit choice wins, even when the setting's
value is nullable. This follows [Boat lifetime settings](../todo/1168-boat-vm-lifetime.md)
without making Boat's storage or authorization a shared settings system.

- **Date:** 2026-10-28
- **Status:** accepted

## Context

Boat lifetime is the first saved setting with a gateway default and a channel
override. Its config parser supplies a built-in value when no default is saved,
and its domain value selects a channel override when one is saved. Another
setting or a future parent scope would otherwise copy that selection rule.
The current channel model has no parent project identity, so it cannot yet
load a project choice.

## Decision

Use a pure typed resolver. The owning feature supplies its concrete built-in
value and optional choices in order from broadest scope to most specific scope.
Each present choice replaces the inherited value. An absent choice leaves it
alone. An optional value is represented as a present optional value, keeping
explicit null distinct from inheritance. Boat uses this rule for its built-in,
saved default and channel choice. The Boat adapter continues to own its JSON
section, shared file lock, validation, authorization, audit and publication.
When a project scope exists, its owning feature must establish the parent
identity and read its choice before including it in the ordered chain.

## Alternatives considered

- A generic JSON settings registry would combine unrelated schemas and audit
  contracts and move product rules into shared infrastructure.
- A recursive scope tree would invent a hierarchy the current channel model
  does not hold, with cycle and missing-parent behavior nobody has specified.
- Keeping two local fallback expressions would make the next setting implement
  the same rule again.

## Consequences

Another gateway setting can reuse the selection rule with its own validated
value type and scopes. Adding a real parent level changes the feature's
identity, storage and authorization path, then supplies one more ordered choice;
the resolver itself stays unchanged. Frontend presentation and wire contracts
remain feature-owned. The helper does not create shared persistence or
cross-language code reuse.
