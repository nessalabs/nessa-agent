# Linux desktop packaging

Status: TODO — macOS gateway lifecycle and packaging are implemented; Linux
packaging remains unsupported.

`prepare.mjs` currently rejects non-macOS targets, and the native gateway adapter
reports unsupported there. Linux desktop release packaging is therefore blocked,
even though Linux window support exists. Build matching bundled binaries and
Node/harness resources, provide a user-service implementation through `GatewayHost`,
and verify install/start/quit/reopen behavior on Linux. Keep OS branches in
infrastructure/composition. A compiler check on macOS cannot validate this flow.

The implemented macOS runtime, retirement, readiness, and shutdown behavior is
documented in [Gateway chat](../guides/gateway-chat.md#installed-macos-runtime).
