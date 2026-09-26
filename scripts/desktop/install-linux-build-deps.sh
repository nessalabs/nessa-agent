#!/usr/bin/env bash
# The Ubuntu packages the desktop host compiles and bundles against: WebKitGTK
# and GTK for the window, the appindicator library for the tray, librsvg for
# icons, libxdo for the global shortcut, and patchelf, which Tauri's documented
# Linux prerequisites list for its AppImage tooling and which is kept so the
# list stays Tauri's own. One list for every job that builds the app on Linux,
# so the build CI checks and the build a release ships cannot drift apart.
# Extra packages a job needs for itself are passed as arguments.
set -euo pipefail
sudo apt-get update
sudo apt-get install -y libwebkit2gtk-4.1-dev libgtk-3-dev \
  libayatana-appindicator3-dev librsvg2-dev patchelf libxdo-dev "$@"
