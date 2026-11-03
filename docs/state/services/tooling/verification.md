---
id: tooling-verification
title: Verification commands and model evidence
kind: guide
status: reference
summary: Choose catalog, model, scripted, browser and provider checks by the behavior they exercise.
parent: tooling
sourceRevision: "3f0392b148ee998bf2df949bced96f5a61059769"
sources:
  - package.json
  - scripts/state-docs/check.mjs
  - scripts/state-docs/check.test.mjs
  - scripts/state-docs/coverage.mjs
  - scripts/state-docs/coverage.test.mjs
  - verification/README.md
  - verification/desktop/CHECKLIST.md
  - verification/desktop/scripts/run-all.mjs
  - scripts/workspaces/README.md
  - scripts/subagent-contracts/README.md
diagramLinks: {}
---

# Verification commands and model evidence

Choose a check by what it observes. Catalog validation checks the state pages;
TLC explores an authored finite model; Rust and TypeScript tests exercise code;
browser scripts measure the rendered conversation and workspace. A passing model
does not establish that the program implements it. A source checkpoint records
reviewed code, not a test run.

## State pages and TLC

Follow [state documentation checks](../../../../scripts/state-docs/README.md)
for setup and flags. The default catalog check needs a built Nessa Docs checkout
but no Java. Model checking needs its verified cache, installed only with the
explicit `--install` flag. A normal run does not rewrite pages; `--write` saves
finished findings, including broken and unchecked rules.

The [feature verification map](../../feature-verification.md) accounts for every
feature page across app surfaces, host, gateway, SDK, auth, client, storage and
remote integrations. Each directly modeled owner states its source mapping and
abstraction. Shared-owner links identify the covered portion without claiming
that a backend model proves an entire UI journey. These are safety checks;
they do not prove eventual progress, physical provider termination or a complete
correspondence between the program and its models.

`pnpm docs:state:coverage` fails unmapped stateful features. It distinguishes
indexes, contracts, reference material and explicit external sibling boundaries.
It is a map-completeness check, not a model pass. Run
`pnpm docs:state:coverage && pnpm docs:state:models` for both checks. Linked
models are discovered from the owning pages under `docs/state/tla/`, without
adding a parallel model list to the runner.

## Current repository entry points

These commands are the entry points in `package.json` at this review. The
package manifest owns their implementation; this table is a navigation aid.
`docs:state:*` runs without installing this app's UI dependencies. Other commands
use the repository's normal setup and the prerequisites of their owning guide.

| Command | What it checks |
| --- | --- |
| `pnpm scripts:test` | Developer script tests. |
| `pnpm frontend:check` | Frontend formatting, lint, contracts, types, tests and build. |
| `pnpm architecture` | Module boundaries and architecture rules. |
| `pnpm agents:check` | Pinned agent runtime tooling. |
| `pnpm protocol:check` | Generated protocol and product contracts. |
| `pnpm auth:test` | Authentication and local storage tests. |
| `pnpm server:test` | Gateway, protocol, client core, device client and vault tests. |
| `pnpm sdk:check` | SDK format, lint, tests and public documentation. |
| `pnpm desktop:check` | Native desktop host checks. |
| `pnpm mcp:check` | Managed MCP server checks. |
| `pnpm database:check` | Local database format, lint and tests. |
| `pnpm images:check` | Image processing format, lint and tests. |
| `pnpm check:changed` | Choose local checks for the changed paths. |
| `pnpm test:e2e:scripted` | Conversation scenarios with a controlled provider. |
| `pnpm test:e2e:stress` | Conversation features under concurrent work and subscription churn. |
| `pnpm test:e2e:chaos` | Conversation interruption and recovery scenarios. |
| `pnpm test:scale` | Gateway load and client scale workload. |
| `pnpm verify:desktop` | Full desktop browser scenario inventory. |
| `pnpm verify:desktop:test` | Browser harness helper tests without browser startup. |
| `pnpm verify:desktop:smoke` | Desktop browser smoke scenarios. |
| `pnpm verify:desktop:perf` | Production browser performance budgets. |
| `pnpm verify:desktop:alpha` | Alpha browser performance scenarios. |
| `pnpm verify:desktop:safe-area` | Window controls and safe-area layout. |
| `pnpm verify:desktop:drag` | Dragging interactions and measurements. |
| `pnpm verify:desktop:focus` | Focus routing and keyboard behavior. |
| `pnpm verify:desktop:responsive` | Responsive browser layouts. |
| `pnpm docs:state:coverage` | Account for direct/shared model owners and expose unmapped stateful features. |
| `pnpm docs:state:check` | State catalog, source links, diagrams and model-file references. |
| `pnpm docs:state:probes` | Pass each original model, then falsify expected rules with finite safety weakenings. |
| `pnpm docs:state:models` | Linked TLC models; optional explicit install and save flags. |
| `pnpm docs:state:test` | Command behavior, synthetic catalogs and opt-in real TLC controls. |

## What to run next

The [browser verification guide](../../../../verification/README.md) owns CLI
options, dev versus production preview ownership and JSON/exit-status semantics.
The [desktop checklist](../../../../verification/desktop/CHECKLIST.md) owns the
scenario inventory and budgets, including checks discovered by the full runner
that have no dedicated package alias. Run the relevant scenario while iterating
and the required suite before handing off a UI change.

The scripted conversation runner uses a controlled provider; stress exercises
conversation features and subscription churn; chaos explores interrupted
operations. Their fixtures do not establish real-provider behavior. Parent Stop
has its own `verification/desktop/scripts/parent-stop.mjs` check and stress step;
it is separate from SDK attachment close. Inspect each script's options through
its owning guide before a long run.

[Provider contract capture](../../../../scripts/subagent-contracts/README.md)
is opt-in and records provider/runtime provenance. The
[Boat workspace check](../../../../scripts/workspaces/README.md) uses a real
account and disposable billable VMs; it is excluded from default CI. Neither
runs as part of state catalog or model checks. The scale harness also has its
own workload and evidence; model state counts are not throughput measurements.

## Native device and integration checks

The [native-device guide](../../../../verification/native-device/README.md)
owns the focused Swift queue and product-request probes introduced on main.
They compile production wrapper/lane code with narrow test stand-ins. Their
result is separate from a phone app, generated native package, TLS listener or
real provider. The [native submission owner](../client/native-submissions.md)
records the actual queue and downstream ticket boundaries. The Swift FIFO is
unbounded; finite TLA request counts are analysis bounds, not runtime admission
limits or queue-wait deadlines.

The updated [browser verification guide](../../../../verification/README.md)
also records context-admission stress steps from integrated main. Listing them
here does not claim that the whole stress suite ran locally in this pass.

## Review scope

This page inventories the manifest and the cited verification guides. The
[refresh record](../../provenance.md#state-verification-refresh-2026-11-02)
records actual local checks and exclusions. Listing an entry point here does not
claim its suite passed.
