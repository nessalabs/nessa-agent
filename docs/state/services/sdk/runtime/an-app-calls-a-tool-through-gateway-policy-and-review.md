---
id: "sdk-runtime-an-app-calls-a-tool-through-gateway-policy-and-review"
title: "an app calls a tool through gateway policy and review"
kind: "operation"
status: "mixed"
summary: "An app can ask the gateway to call a tool on its originating server."
parent: "sdk-runtime"
sources:
  - "crates/nessa-server/src/product/mcp_apps.rs"
  - "crates/nessa-server/src/conversation/application/service/app_calls.rs"
  - "crates/nessa-server/src/mcp_servers/domain/app_call.rs"
  - "crates/nessa-server/src/conversation/application/app_reviews.rs"
  - "crates/nessa-server/src/mcp_servers/infrastructure/apps.rs"
  - "packages/nessa-client/src/presentation/mcp-apps-api.ts"
  - "crates/nessa-server/tests/conversation/app_calls.rs"
  - "crates/nessa-server/tests/conversation/app_reviews.rs"
  - "crates/nessa-server/tests/conversation/mcp_app_audit.rs"
  - "packages/nessa-client/src/presentation/mcp-apps-api.test.ts"
diagramLinks: {}
---

# an app calls a tool through gateway policy and review

An app can ask the gateway to call a tool on its originating server. The gateway checks the conversation, originating tool, app visibility and request before sending anything. Destructive tools require an app-specific approval even if the conversation uses a different approval preset.

Approval expires after five minutes and applies only to the matching request. Completion audit is separate from the tool result. A tool can return an error as its answer. A lost answer after dispatch is uncertain, so the app must not automatically repeat the call. Gateway-backed desktop apps call through their workspace client; sample-mode apps use fixtures.

```mermaid
stateDiagram-v2
    [*] --> Policy
    Policy --> Refused: Invalid owner, app, server, target or bounds
    Policy --> WaitingReview: Destructive target / audit approval request
    Policy --> Dispatching: No review required / audit admission
    WaitingReview --> Dispatching: Exact allow-once consumed / audit approval
    WaitingReview --> Refused: Deny, expiry, withdrawal or conversation end / audit
    Dispatching --> Completing: Same-session upstream call returns
    Dispatching --> Completing: Upstream failure or lost caller
    Completing --> Returned: Completion audit accepted / return outcome
    Completing --> Uncertain: Completion audit fails after possible effect
    note right of WaitingReview
        Caller loss withdraws a waiting review.
        A prior accepted answer keeps its attribution.
    end note
```

## Further reading

[Source](../../../../../crates/nessa-server/src/product/mcp_apps.rs) · [Related source](../../../../../crates/nessa-server/src/conversation/application/service/app_calls.rs) · [Related tests](../../../../../crates/nessa-server/tests/conversation/app_calls.rs)
