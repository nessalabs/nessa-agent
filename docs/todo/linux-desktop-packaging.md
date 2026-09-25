# Linux desktop packaging

Status: TODO — macOS gateway lifecycle and packaging are implemented; Linux
packaging remains unsupported.

`prepare.mjs` now assembles the native x86_64 Linux runtime resource, and local
Linux builds include its verified binaries, locked harnesses, model catalogue,
Node license, and fingerprinted manifest. The existing Ubuntu CI matrix leg
exercises that assembly. The native gateway adapter still reports the capability
as unsupported and the release targets remain Darwin-only, so Linux packaging is
not yet a shippable local-gateway experience even though Linux window support
exists. Provide a user-service implementation through `GatewayHost`, package it,
and verify install/start/quit/reopen behavior on Linux. Keep OS branches in
infrastructure/composition. A compiler check on macOS cannot validate this flow.

The implemented macOS runtime, retirement, readiness, and shutdown behavior is
documented in [Gateway chat](../guides/gateway-chat.md#installed-macos-runtime).

The step-by-step plan, with diagrams and the Windows track, is in
[Ship the desktop app on Linux and Windows](desktop-linux-windows-plan.md).
