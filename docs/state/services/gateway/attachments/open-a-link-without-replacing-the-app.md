---
id: "gateway-attachments-open-a-link-without-replacing-the-app"
title: "Open a link without replacing the app"
kind: "operation"
status: "mixed"
summary: "In a native Nessa window, external web and mail links are handed to the system opener."
parent: "gateway-attachments"
sourceRevision: "728e92aba6a4608cfeaee629b72ebbeaa7d6a0ca"
sources:
  - "src-tauri/src/links.rs"
  - "src-tauri/src/platform/mod.rs"
  - "src-tauri/src/platform/macos/mod.rs"
  - "src-tauri/src/platform/linux/mod.rs"
  - "src/desktop/widgets/app/model/csp.ts"
  - "src/desktop/widgets/app/sandbox/proxy.html"
  - "src/panel/adapters/use-link-notice.ts"
  - "src/panel/application/link-notice.ts"
  - "src/panel/application/link-notice.test.ts"
  - "src-tauri/tauri.conf.json"
diagramLinks: {}
---

# Open a link without replacing the app

In a native Nessa window, external web and mail links are handed to the system opener. App-owned addresses stay inside Nessa; unsupported schemes are refused. A failed handoff shows a dismissible notice in the panel.

An accepted handoff does not prove the destination loaded. Some new-window, download and platform navigation paths fall outside this hook. Plain browsers have different navigation behavior, and another native surface needs its own notice subscription.

An MCP app runs inside a sandbox proxy. Its default CSP blocks the app's own
external frame navigation before the native hook decides whether to hand a URL
to the system opener. A bundled macOS WKWebView probe saw CSP violations for
script navigation, meta refresh and a rewritten document targeting HTTP, and
for script navigation targeting HTTPS and `mailto:`. None reached the hook;
a clicked link in the host page did. The [native probe evidence](../../../../../verification/desktop/evidence/1192-webkit-navigation/README.md)
records its debug-bundle scope and screenshots. This review checked the hook,
platform openers, panel notice, sandbox CSP and proxy at the source revision
above. It did not retest new-window or download paths.

```mermaid
stateDiagram-v2
    [*] --> ProposedNavigation
    ProposedNavigation --> InApp: Exact application origin
    ProposedNavigation --> ExternalHandoff: External http, https or mailto
    ProposedNavigation --> Refused: Other scheme / LinkNotOpened notice
    ExternalHandoff --> Accepted: Platform opener accepted
    ExternalHandoff --> Failed: Opener failed / LinkNotOpened notice
    InApp --> [*]: Navigate inside application
    Accepted --> [*]: Cancel webview navigation
    Refused --> [*]: Cancel webview navigation
    Failed --> [*]: Cancel webview navigation
    note right of Accepted
        Acceptance is not proof the external page loaded.
        This chart models the native navigation hook.
        Plain browser navigation is a separate environment.
    end note
```

## Further reading

[Source](../../../../../src-tauri/src/links.rs) · [Related source](../../../../../src-tauri/src/platform/mod.rs) · [Related tests](../../../../../src/panel/application/link-notice.test.ts)
