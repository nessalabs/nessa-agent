# Nessa. just is the entry. OS differences live here, not in a second host.
# Windows recipes are written, not yet run on a Windows box.
#
#   just          list recipes
#   just dev      desktop app in dev mode (falls back to the browser UI)
#   just web      UI in a browser only; window controls no-op
#   just fast     testing-shaped release (macOS .app / Linux .deb / Windows nsis)
#   just release  shipping bundle (macOS .dmg / Linux .deb / Windows nsis)

# cmd so Windows does not need Git's sh. Unix still uses sh.
set windows-shell := ["cmd.exe", "/c"]

# Tauri --bundles is per OS. `app` / `dmg` are macOS-only.
fast-bundle := if os() == "macos" { "app" } else if os() == "windows" { "nsis" } else { "deb" }
release-bundle := if os() == "macos" { "dmg" } else if os() == "windows" { "nsis" } else { "deb" }

# List recipes. Bare `just` is not `just dev`.
[private]
default:
    @just --list

# UI in a browser only; window controls no-op.
web:
    pnpm dev

# Desktop app in dev mode (`tauri dev`).
[linux]
dev:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! pkg-config --exists webkit2gtk-4.1 gtk+-3.0; then
      echo "Linux native deps missing. On Debian/Ubuntu:"
      echo "  sudo apt install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev patchelf fakeroot"
      exit 1
    fi
    if [[ -z "${WEBKIT_DISABLE_DMABUF_RENDERER:-}" && ! -e /dev/dri/card0 && ! -e /dev/dri/renderD128 ]]; then
      export WEBKIT_DISABLE_DMABUF_RENDERER=1
      export WEBKIT_DISABLE_COMPOSITING_MODE=1
    fi
    if [[ -n "${CI:-}" || ( -z "${DISPLAY:-}" && -z "${WAYLAND_DISPLAY:-}" ) ]]; then
      echo "→ no GUI on this machine; starting web dev server instead"
      exec pnpm dev
    fi
    exec pnpm app

# Desktop app in dev mode (`tauri dev`).
[macos]
[windows]
dev:
    pnpm app

# Testing-shaped release (macOS .app / Linux .deb / Windows nsis).
[unix]
fast:
    CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_PROFILE_RELEASE_OPT_LEVEL=1 CARGO_PROFILE_RELEASE_STRIP=false pnpm exec tauri build --bundles {{fast-bundle}}

# Testing-shaped release (macOS .app / Linux .deb / Windows nsis).
[windows]
fast:
    set CARGO_PROFILE_RELEASE_LTO=false&& set CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16&& set CARGO_PROFILE_RELEASE_OPT_LEVEL=1&& set CARGO_PROFILE_RELEASE_STRIP=false&& pnpm exec tauri build --bundles {{fast-bundle}}

# Shipping bundle: fat LTO, stripped.
[unix]
release:
    pnpm exec tauri build --bundles {{release-bundle}}

# Shipping bundle: fat LTO, stripped.
[windows]
release:
    pnpm exec tauri build --bundles {{release-bundle}}
