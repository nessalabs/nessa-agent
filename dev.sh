#!/usr/bin/env bash
#
# Run Nessa locally. Picks sensible defaults for the machine you're on.
#
#   ./dev.sh          Desktop app (tauri dev) when a GUI is available
#   ./dev.sh --web    UI in a browser only; window controls no-op
#   ./dev.sh --fast   Testing-shaped release (host bundle, slow opts off)
#
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
os="$(uname -s)"

die() { printf '%s\n' "$*" >&2; exit 1; }

usage() {
  cat >&2 <<'USAGE'
usage: ./dev.sh [--web|--fast]

  (default)  Run the desktop app — Vite on :1420 and tauri dev
  --web      Run the UI in a browser only; window controls no-op
  --fast     Release build with the slow optimisations off
             (macOS .app / Linux .deb / Windows nsis)

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

# A missing webkit2gtk-4.1-dev fails later as a cargo/pkg-config error that
# does not mention the apt line. Check here so `./dev.sh` is the whole story.
need_linux_native() {
  [[ "$os" == Linux ]] || return 0
  if pkg-config --exists webkit2gtk-4.1 gtk+-3.0 2>/dev/null; then
    return 0
  fi
  die "Linux native deps missing. On Debian/Ubuntu:
  sudo apt install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev patchelf fakeroot"
}

mode="${1:-}"
case "$mode" in
  -h | --help) usage ;;
  --web) mode=web ;;
  --fast) mode=fast ;;
  "") mode=app ;;
  *) die "unknown option: $mode (try --web or --fast)" ;;
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
    need_linux_native
    configure_linux_webkit
    exec pnpm app
    ;;
  fast)
    need_linux_native
    configure_linux_webkit
    exec pnpm app:fast
    ;;
esac
