# Linux desktop packaging

Status: packaged — releases build an x86_64 `.deb`
([#216](https://github.com/nessalabs/nessa-agent/issues/216)); installed
acceptance remains.

The release workflow builds the `.deb` on Ubuntu 22.04, verifies the
fingerprinted runtime inside it with `scripts/desktop/verify-linux-bundle.mjs`,
and publishes it in the draft release under the `linux-x86_64-deb` updater
key. What an installed Linux app does is described in
[Gateway chat](../guides/gateway-chat.md#installed-linux-runtime).

What remains:

- **Installed acceptance** ([#188](https://github.com/nessalabs/nessa-agent/issues/188)):
  on a fresh Ubuntu desktop, install the `.deb`, open the app, chat, quit,
  reopen, log out and back in, and chat again.
- **Running while logged out** ([#217](https://github.com/nessalabs/nessa-agent/issues/217)):
  setup offers linger explicitly and reports what logind confirms.
- **AppImage**: not released. linuxdeploy rewrites every ELF file under
  `usr/lib`, the runtime's executables included, so the runtime fails its
  fingerprint (the proof run on #219 showed `nessa`, `nessa-mcp`, `node`, the
  Claude agent binary, and Codex's tools all changed). Shipping one needs the
  runtime placed where linuxdeploy does not reach, and the app finding it there.
- aarch64 Linux, RPM, Flatpak, and Snap are not built.

The step-by-step plan, with diagrams and the Windows track, is in
[Ship the desktop app on Linux and Windows](desktop-linux-windows-plan.md).
