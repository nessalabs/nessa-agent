---
id: "host-startup-sign-in-restore-renew-and-sign-out-in-the-browser"
title: "sign in, restore, renew, and sign out in the browser"
kind: "statechart"
status: "implemented"
summary: "Browser sign-in establishes a server-backed session cookie."
parent: "host-startup"
sources:
  - "src/composition/browser.tsx"
  - "src/session/adapters/client/browser-auth.ts"
  - "src/session/adapters/client/browser-auth.test.ts"
  - "src/session/adapters/lifecycle/browser-renewal.ts"
  - "src/session/adapters/lifecycle/browser-renewal.test.ts"
  - "src/session/application/browser-session.ts"
  - "src/session/application/browser-session.test.ts"
  - "crates/nessa-server/src/browser_session/entrypoint/http.rs"
  - "crates/nessa-server/src/browser_session/application/session.rs"
  - "crates/nessa-server/src/browser_session/domain/value_objects/lifetime.rs"
  - "crates/nessa-server/tests/browser_session/persistence.rs"
diagramLinks: {}
model:
  spec: tla/services/host/startup/BrowserLogout.tla
  config: tla/services/host/startup/BrowserLogout.cfg
modelCheck:
  checker: tlc-1.7.4
  date: 2026-11-03T03:28:33.470Z
  config: tla/services/host/startup/BrowserLogout.cfg
  modelDigest: e4a5696d2ea184aab7c89149dccc4a04741332e377c66938b0ca209b6084f8c5
  states: 5
  held:
    - LocalTeardownFirst
    - AcknowledgedLogout
  broke: []
  unchecked: []
---
# sign in, restore, renew, and sign out in the browser

Browser sign-in establishes a server-backed session cookie. The page does not keep the gateway token in local or session storage. Reload can restore the session while it remains valid.

Renewal extends the browser's idle lifetime within the credential's current validity. Trusted-origin checks still apply, and expiry or revocation can require sign-in again. Sign-out first tears down local access and retains logout intent so a failed request can be retried. A local sign-out view alone does not prove server-side logout finished.

```mermaid
stateDiagram-v2
    [*] --> Restoring: Validate transport policy
    Restoring --> LoggingOut: Retained logout intent
    Restoring --> SignedOut: No valid cookie session
    Restoring --> Active: Persisted session validated
    SignedOut --> SigningIn: Submit issued browser token
    SigningIn --> SignedOut: Refused or unavailable
    SigningIn --> Active: Origin-bound cookie persisted
    state Active {
        [*] --> SetupGate
        SetupGate --> ConnectedScope: Finish or dismiss in-place setup
        ConnectedScope --> ConnectedScope: Check succeeds / renew cookie
        ConnectedScope --> ConnectedScope: Service outage / keep scope
    }
    Active --> SignedOut: Check returns 401 / dispose scope
    Active --> LoggingOut: Disconnect / tear down scope and retain logout intent
    LoggingOut --> SignedOut: Logout acknowledged / clear intent
    LoggingOut --> LogoutUnconfirmed: Service unavailable
    LogoutUnconfirmed --> LoggingOut: Retry or later restoration
    note right of Active
        Visible check every five minutes and on focus.
        Tokens are not persisted by the page.
        Cookie authentication and product authorization
        remain separate current-state decisions.
    end note
```

## Further reading

[Source](../../../../../src/composition/browser.tsx) · [Related source](../../../../../src/session/adapters/client/browser-auth.ts) · [Related tests](../../../../../src/session/adapters/client/browser-auth.test.ts)

## Bounded rule model

Owns browser sign-out ordering and retained retry intent after an unconfirmed request. Restore consumes pending intent before restoring access. Login replacement, server expiry/renewal, origin verification and revocation are separate server contracts excluded here. This finite abstraction checks safety only. It does not execute or verify the implementation, promise eventual completion, or qualify external effects. Existing source checkpoints and historical test receipts retain their earlier scope. The scoped model review inspected `src/session/application/browser-session.ts`, `src/session/adapters/client/browser-auth.ts`.
