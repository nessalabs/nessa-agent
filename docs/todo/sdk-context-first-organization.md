# SDK context-first organization

Status: TODO — explicitly deferred by the user on 2026-09-15. This is an
organization task, not an implemented API or a compatibility transition.

## Goal

Make each existing SDK use context/feature first, then DDD role, matching the
layout established for new gateway and MCP code. For example, move Rust agent
execution from `src/domain/agent_execution` and `src/application/agent_execution`
to `src/agent_execution/domain` and `src/agent_execution/application`.

## Scope

- Inventory `nessa-sdk`, `nessa-auth`, and `packages/nessa-client` before moving
  code; identify bounded contexts and their real dependency graph.
- Move source, tests, examples, and feature documentation together. Keep ports
  and DTOs in application, rules in domain, adapters in infrastructure, and
  backend selection in composition.
- Update every Rust/TypeScript caller, generated references, test include path,
  module map, documentation link, and CI/architecture/coverage script.
- Use one current public import path. Do not add forwarding modules, deprecated
  aliases, old-data readers, or version bumps for the move.
- Keep shared value objects shared only when their meaning is actually common.
  Establish the role layout from day one; put the module map beside each context
  so new agents can extend it without inventing a parallel structure.

## Acceptance

Review the complete caller inventory and both dependency directions. Compile all
workspace consumers and examples, run domain coverage and adapter/substitution
checks, run Rustdoc and TypeScript API docs, and test the CI package/platform
combinations. Record the reviewed revision and any unsupported platform checks.
The present review changes only newly added code; existing SDK paths stay current
until this task is implemented as a coordinated move.
