#!/usr/bin/env bash
# The Ubuntu packages the desktop host compiles and bundles against: WebKitGTK
# and GTK for the window, the appindicator library for the tray, librsvg for
# icons, and libxdo for the global shortcut. No patchelf: only the AppImage
# tooling uses it, and no build here makes an AppImage. One list for every job
# that builds the app on Linux, so the build CI checks and the build a release
# ships cannot drift apart. Extra packages a job needs for itself are passed as
# arguments.
#
# A stalled Ubuntu mirror held local-auth for hours twice on 2026-10-07: after
# azure.archive.ubuntu.com failed over to archive.ubuntu.com, `apt-get update`
# printed `Get:5 https://archive.ubuntu.com/ubuntu noble-security InRelease`
# and never printed again (runs 37668027204 and 37670890766, #653). That stall
# was silence, not a slow transfer. `update` stays three attempts of five
# minutes. `install` is allowed 45 minutes: two green installs that day were
# still moving and finished in 30.6 and 41.5 minutes (the slower one fetched
# 61.4 MB at 24.8 kB/s). A transfer at that pace passes. A mirror that goes
# silent, or one slower than this ceiling, fails.
#
# DEBIAN_FRONTEND is a sudo assignment on purpose. Preserving the whole
# environment would hand apt the release job's updater signing key.
set -euo pipefail
apt_options=(
  -o Acquire::Retries=3
  -o Acquire::http::Timeout=30
  -o Acquire::https::Timeout=30
)
for attempt in 1 2 3; do
  if sudo DEBIAN_FRONTEND=noninteractive timeout --kill-after=30s 5m apt-get "${apt_options[@]}" update; then
    break
  fi
  if [ "$attempt" -eq 3 ]; then
    echo "apt-get update failed on all three attempts" >&2
    exit 1
  fi
  sleep $((attempt * 15))
done
sudo DEBIAN_FRONTEND=noninteractive timeout --kill-after=30s 45m apt-get "${apt_options[@]}" install -y \
  libwebkit2gtk-4.1-dev libgtk-3-dev \
  libayatana-appindicator3-dev librsvg2-dev libxdo-dev "$@"
