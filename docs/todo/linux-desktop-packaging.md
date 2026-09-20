# Linux desktop packaging

Status: TODO — macOS gateway lifecycle and packaging are implemented; Linux
packaging remains unsupported.

`prepare.mjs` leaves managed-runtime preparation disabled on non-macOS targets so
the existing desktop build can still compile there. The native gateway adapter
reports that capability as unsupported, so Linux release packaging is not yet a
shippable local-gateway experience even though Linux window support exists. Build
matching bundled binaries and Node/harness resources, provide a user-service
implementation through `GatewayHost`, and verify install/start/quit/reopen behavior
on Linux. Keep OS branches in infrastructure/composition. A compiler check on
macOS cannot validate this flow.

The implemented macOS runtime, retirement, readiness, and shutdown behavior is
documented in [Gateway chat](../guides/gateway-chat.md#installed-macos-runtime).

The step-by-step plan, with diagrams and the Windows track, is in
[Ship the desktop app on Linux and Windows](desktop-linux-windows-plan.md).
