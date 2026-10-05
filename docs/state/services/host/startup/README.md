---
id: "host-startup"
title: "Startup and access"
kind: "feature"
status: "mixed"
summary: "Startup chooses the first screen, checks agent readiness and establishes access to the gateway."
parent: "host"
sources:
  - "justfile"
  - "scripts/preflight.mjs"
  - "scripts/free-gateway-port.mjs"
  - "package.json"
  - "scripts/dev-agent-config.mjs"
  - "docs/guides/local-auth.md"
  - "src-tauri/src/composition.rs"
  - "scripts/run-dev-app.sh"
  - "src/main.tsx"
  - "src/composition/browser.tsx"
  - "src/composition/dependencies.ts"
  - "vite.config.ts"
  - "src-tauri/src/main.rs"
  - "src-tauri/src/gateway/infrastructure/selection.rs"
  - "src-tauri/src/gateway/infrastructure/macos.rs"
  - "src-tauri/src/gateway/infrastructure/linux/mod.rs"
  - "src-tauri/src/gateway/infrastructure/linux/user_manager.rs"
  - "src-tauri/src/gateway/domain/value_objects/systemd.rs"
  - "src-tauri/src/gateway/infrastructure/unsupported.rs"
  - "protocol/defaults/gateway-ports.json"
  - "src-tauri/src/gateway/infrastructure/commands.rs"
  - "src-tauri/src/gateway_endpoint/entrypoint/command.rs"
  - "src-tauri/src/surface_credential.rs"
  - "src/onboarding/adapters/agent-installations.ts"
  - "src/onboarding/adapters/agents.ts"
  - "src/onboarding/adapters/agents.test.ts"
  - "src/onboarding/ui/onboarding-readiness-timeout.test.tsx"
  - "crates/nessa-server/src/agents/application/shared_readiness.rs"
  - "verification/desktop/scripts/onboarding-readiness.mjs"
  - "src-tauri/src/agent_credentials/infrastructure/mod.rs"
  - "src-tauri/src/agent_credentials/infrastructure/unsupported.rs"
  - "src/onboarding/ui/agent-api-key-form.tsx"
  - "src-tauri/src/panel.rs"
  - "src/startup/application/gateway-startup.test.ts"
  - "src/panel/ui/use-panel-startup.test.ts"
  - "src/session/adapters/lifecycle/supervisor.test.ts"
  - "docs/adr/done/0010-local-authentication.md"
  - "docs/adr/done/173-fetch-agent-runtimes.md"
  - "docs/ARCHITECTURE.md"
diagramLinks:
  F0: "host-startup-launch-and-choose-the-first-surface"
  F1: "host-startup-complete-setup-choose-an-agent-and-learn-the-shortcut"
  F2: "host-startup-save-provider-credentials-and-download-a-runtime"
  F3: "host-startup-recover-or-update-the-managed-gateway"
  F4: "host-startup-authenticate-reconnect-expire-or-revoke-a-surface-session"
  F5: "host-startup-sign-in-restore-renew-and-sign-out-in-the-browser"
  F6: "host-startup-bootstrap-and-diagnose-access-through-the-cli"
---

# Startup and access

Startup chooses the first screen, checks agent readiness and establishes access to the gateway. The native host can manage a local gateway; a browser connects to one already running.

Host ready, gateway ready and agent ready mean different things. A saved provider key is also different from a valid gateway token or a successful agent sign-in.

## Feature overview

These boxes open related pages. They are parts of the system, rather than mutually exclusive runtime states.

```mermaid
flowchart TB
    Feature["Browse this feature"]
    F0["Launch and choose the first surface"]
    Feature --> F0
    F1["Complete setup, choose an agent, and learn the shortcut"]
    Feature --> F1
    F2["Save provider credentials and download a runtime"]
    Feature --> F2
    F3["Recover or update the managed gateway"]
    Feature --> F3
    F4["Authenticate, reconnect, expire, or revoke a surface session"]
    Feature --> F4
    F5["Sign in, restore, renew, and sign out in the browser"]
    Feature --> F5
    F6["Bootstrap and diagnose access through the CLI"]
    Feature --> F6
```

## Browse flows

- [Authenticate, reconnect, expire, or revoke a surface session](authenticate-reconnect-expire-or-revoke-a-surface-session.md)
- [Bootstrap and diagnose access through the CLI](bootstrap-and-diagnose-access-through-the-cli.md)
- [Complete setup, choose an agent, and learn the shortcut](complete-setup-choose-an-agent-and-learn-the-shortcut.md)
- [Launch and choose the first surface](launch-and-choose-the-first-surface.md)
- [Recover or update the managed gateway](recover-or-update-the-managed-gateway.md)
- [Save provider credentials and download a runtime](save-provider-credentials-and-download-a-runtime.md)
- [Sign in, restore, renew, and sign out in the browser](sign-in-restore-renew-and-sign-out-in-the-browser.md)
