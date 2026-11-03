---
id: desktop-provider-sign-in
title: Recover provider sign-in
kind: statechart
status: implemented
summary: A provider login launch leaves the refused input visible until source
  evidence changes.
parent: desktop-workspace
sourceRevision: "77a6b620af4d94b234294f6d273fd74b3dfb44c7"
sources:
  - src/provider-authentication/model/recovery.ts
  - src/provider-authentication/ui/provider-sign-in.tsx
  - src/provider-authentication/ui/provider-sign-in.test.tsx
  - src/conversation/adapters/store/authentication-recovery.test.tsx
diagramLinks: {}
model:
  spec: tla/services/desktop/workspace/provider-sign-in.tla
  config: tla/services/desktop/workspace/provider-sign-in.cfg
modelCheck:
  sourceRevision: "77a6b620af4d94b234294f6d273fd74b3dfb44c7"
  checker: tlc-1.7.4
  date: 2026-11-03T03:27:58.144Z
  config: tla/services/desktop/workspace/provider-sign-in.cfg
  modelDigest: 0b2cf68f1d32d1373f5559c21fef2a2fc085ef9871d66c27604bcd68c7c7f768
  states: 10
  held:
    - RecoveryCausality
  broke: []
  unchecked: []
---
# Recover provider sign-in

Claude and Codex can offer a sign-in card. Other providers retain their normal refusal. The refusal must still identify the latest published input, and no local submission may have observed that same input since. This causality check uses input identity, not elapsed time.

The card checks host capability and binds the answer to both check and action callbacks. Unsupported or failed capability checks disable launch. A synchronous in-flight owner prevents repeated launch before the disabled render. Launch failure allows retry and says why. Launch success does not establish authenticated provider state, replay the input, or dismiss the refusal. The external login flow is outside this source tree.

```mermaid
stateDiagram-v2
 [*] --> Checking
 Checking --> Available: Current capability supports login
 Checking --> Unavailable: Unsupported or check fails
 Available --> Launching: Press / claim in-flight owner
 Launching --> Available: Launch settles / release owner
```

## Bounded rule model

Two input identities cover recovery eligibility and a newer local submission. Launch preserves refusal evidence. Capability callbacks, launch-slot serialization and the external login protocol are excluded from this model. The model checks design safety, not the implementation. It asserts no liveness and abstracts transport timing into nondeterministic completion order. Regression sources are evidence of intended behavior; this documentation change does not claim their execution.

## Model coverage

Disposition: direct model. The bounded rule model section names the modeled owner slice and finite abstraction.

Excluded: All exclusions stated in the owning Bounded rule model section remain outside verification.

Source review scope: owner entry points and relevant regression declarations were inventoried against frozen source `77a6b620af4d94b234294f6d273fd74b3dfb44c7`. Existing source checkpoints are retained where this was a partial review. A model result establishes bounded design safety only. Browser appearance and interaction were not checked.
