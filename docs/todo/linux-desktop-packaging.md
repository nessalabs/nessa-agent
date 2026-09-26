# Linux desktop packaging

Status: packaged — releases build an x86_64 `.deb` and AppImage
([#216](https://github.com/nessalabs/nessa-agent/issues/216)); installed
acceptance remains.

The release workflow builds both packages on Ubuntu 22.04, verifies the
fingerprinted runtime inside each with `scripts/desktop/verify-linux-bundle.mjs`,
and publishes them in the draft release with one updater key per format. What
an installed Linux app does is described in
[Gateway chat](../guides/gateway-chat.md#installed-linux-runtime).

What remains:

- **Installed acceptance** ([#188](https://github.com/nessalabs/nessa-agent/issues/188)):
  on a fresh Ubuntu desktop, install the `.deb`, open the app, chat, quit,
  reopen, log out and back in, and chat again.
- **Running while logged out** ([#217](https://github.com/nessalabs/nessa-agent/issues/217)):
  setup offers linger explicitly and reports what logind confirms.
- aarch64 Linux, RPM, Flatpak, and Snap are not built.

The step-by-step plan, with diagrams and the Windows track, is in
[Ship the desktop app on Linux and Windows](desktop-linux-windows-plan.md).
