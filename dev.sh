#!/usr/bin/env bash
#
# Run Nessa locally. Picks sensible defaults for the machine you're on.
#
#   ./dev.sh         Desktop app (tauri dev) when a GUI is available
#   ./dev.sh --web   UI in a browser only; window controls no-op
#
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
os="$(uname -s)"

die() { printf '%s\n' "$*" >&2; exit 1; }

usage() {
  cat >&2 <<'USAGE'
usage: ./dev.sh [--web]

  (default)  Run the desktop app — Vite on :1420 and tauri dev
  --web      Run the UI in a browser only; window controls no-op

On Linux without a display, the default falls back to --web automatically.
USAGE
  exit 1
}

# Optional compile cache — used when installed (brew/apt), ignored otherwise.
if command -v sccache >/dev/null 2>&1; then
  export RUSTC_WRAPPER=sccache
fi

has_gui() {
  [[ -z "${CI:-}" ]] || return 1
  case "$os" in
    Darwin) return 0 ;;
    *)
      [[ -n "${DISPLAY:-}" || -n "${WAYLAND_DISPLAY:-}" ]]
      ;;
  esac
}

# WebKitGTK can misbehave on software X / VNC when there is no DRM device.
configure_linux_webkit() {
  [[ "$os" == Linux ]] || return 0
  if [[ -e /dev/dri/card0 || -e /dev/dri/renderD128 ]]; then
    return 0
  fi
  export WEBKIT_DISABLE_DMABUF_RENDERER=1
  export WEBKIT_DISABLE_COMPOSITING_MODE=1
}

mode="${1:-}"
case "$mode" in
  -h | --help) usage ;;
  --web) ;;
  "") mode=app ;;
  *) die "unknown option: $mode (try --web)" ;;
esac

if [[ "$mode" == app ]] && ! has_gui; then
  printf '→ no GUI on this machine; starting web dev server instead\n' >&2
  mode=web
fi

cd "$repo_root"
command -v pnpm >/dev/null 2>&1 || die "pnpm not found — run: corepack enable && corepack prepare"

case "$mode" in
  web)
    if has_gui; then
      exec pnpm dev -- --open
    else
      printf '→ open http://127.0.0.1:1420 in a browser\n' >&2
      exec pnpm dev
    fi
    ;;
  app)
    configure_linux_webkit
    exec pnpm app
    ;;
esac
