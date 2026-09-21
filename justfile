# Nessa. just is the entry. OS differences live here, not in a second host.
# Windows recipes are written, not yet run on a Windows box.
#
#   just          list recipes
#   just start    desktop app + local nessa server
#   just dev      desktop app in dev mode (falls back to the browser UI)
#   just server   local nessa server only
#   just web      UI in a browser only; window controls no-op
#   just release fast  testing-shaped release (macOS .app / Linux .deb / Windows nsis)
#   just release  shipping bundle (macOS .dmg / Linux .deb / Windows nsis)

#   just worktree create <name>  feature checkout sharing the build cache
#   just worktree list           list checkouts

# cmd so Windows does not need Git's sh. Unix still uses sh.
set windows-shell := ["cmd.exe", "/c"]

# Tauri --bundles is per OS. `app` / `dmg` are macOS-only.
fast-bundle := if os() == "macos" { "app" } else if os() == "windows" { "nsis" } else { "deb" }
release-bundle := if os() == "macos" { "dmg" } else if os() == "windows" { "nsis" } else { "deb" }

# List recipes. Bare `just` is not `just dev`.
[private]
default:
    @just --list

# Local gateway (stage=dev, 127.0.0.1:7421). Creates the dev owner and chat
# credentials on first run; existing ones are never replaced. An installed Nessa
# keeps :7420 through its background service, so both can run at once.
server:
    pnpm server:run

# Desktop app + local nessa server (always restarts the dev port so code changes
# load). It restarts a dev server this checkout started and nothing else — see
# scripts/free-gateway-port.mjs for why a launchd service is not ours to kill.
[unix]
start:
    #!/usr/bin/env bash
    set -euo pipefail
    set -m
    # The stage the server will actually read, not an assumption of `dev`: it
    # takes `NESSA_STAGE` from this same environment, and `NESSA_PORT` ahead of
    # the stage's own. Naming `dev` here freed one socket and then waited for
    # health on another whenever the caller had selected anything else.
    stage="$(node -e 'import("./scripts/gateway-port.mjs").then(m => process.stdout.write(m.selectedStage()))')"
    port="$(node scripts/gateway-port.mjs)"
    server_pid=""
    app_pid=""
    # The app goes first, then the gateway it talks to: the other order leaves a
    # window up with its server pulled out from under it, which is the state this
    # cleanup exists to prevent.
    #
    # How a job and its subtree are stopped is one rule, shared with
    # `scripts/run-dev-app.sh`, which is inside this same process tree.
    source scripts/stop-job.sh
    # Runs twice on a signal — once for the signal, once for the EXIT it causes —
    # so each pid is forgotten as it is stopped. Otherwise the second pass
    # announces stopping things that are already gone, and a pid that has since
    # been reused would be signalled for nothing to do with us.
    cleanup() {
      local app="${app_pid}" server="${server_pid}"
      app_pid=""
      server_pid=""
      stop_job "the app" "${app}"
      stop_job "nessa-server" "${server}"
    }
    trap cleanup EXIT INT TERM

    node scripts/free-gateway-port.mjs "${stage}"

    echo "→ starting nessa-server"
    pnpm server:run &
    server_pid=$!
    ready=0
    for _ in $(seq 1 120); do
      if curl -sf --connect-timeout 0.3 "http://127.0.0.1:${port}/health" >/dev/null; then
        ready=1
        break
      fi
      if ! kill -0 "${server_pid}" 2>/dev/null; then
        wait "${server_pid}" || true
        echo "→ nessa-server exited before becoming healthy"
        server_pid=""
        exit 1
      fi
      sleep 0.5
    done
    if [[ "${ready}" -ne 1 ]]; then
      echo "→ nessa-server did not become healthy on :${port}"
      exit 1
    fi
    echo "→ nessa-server ready on :${port} (stage ${stage})"

    # Started as a job rather than run in the foreground, so its process group is
    # known and `cleanup` can take the whole subtree down. `wait` keeps this
    # recipe blocking until the app exits, exactly as the foreground call did,
    # and the trap fires either way round: quitting the app stops the gateway,
    # and stopping this run stops the app.
    just dev &
    app_pid=$!
    wait "${app_pid}"

# UI in a browser only; window controls no-op.
web:
    pnpm dev

# Desktop app in dev mode (`tauri dev`).
[linux]
dev:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ -n "${CI:-}" || ( -z "${DISPLAY:-}" && -z "${WAYLAND_DISPLAY:-}" ) ]]; then
      echo "→ no GUI on this machine; starting web dev server instead"
      exec pnpm dev
    fi
    if ! pkg-config --exists webkit2gtk-4.1 gtk+-3.0; then
      echo "Linux native deps missing. On Debian/Ubuntu:"
      echo "  sudo apt install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev patchelf fakeroot"
      exit 1
    fi
    if [[ -z "${WEBKIT_DISABLE_DMABUF_RENDERER:-}" && ! -e /dev/dri/card0 && ! -e /dev/dri/renderD128 ]]; then
      export WEBKIT_DISABLE_DMABUF_RENDERER=1
      export WEBKIT_DISABLE_COMPOSITING_MODE=1
    fi
    exec bash scripts/run-dev-app.sh

# Desktop app in dev mode (`tauri dev`).
[macos]
dev:
    bash scripts/run-dev-app.sh

# Desktop app in dev mode (`tauri dev`). cmd has no traps, so the app is not
# taken down with this command the way it is on Unix; quit it from the tray.
[windows]
dev:
    pnpm app

# Shipping bundle by default; macOS builds seal and verify the completed bundle.
# `just release fast` builds with faster settings.
[unix]
release mode="shipping":
    {{if mode == "fast" { "CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_PROFILE_RELEASE_OPT_LEVEL=1 CARGO_PROFILE_RELEASE_STRIP=false " } else if mode == "shipping" { "" } else { error("Use just release or just release fast") }}}node scripts/desktop/build.mjs --bundles {{if mode == "fast" { fast-bundle } else { release-bundle }}}

# Shipping bundle by default; `just release fast` builds with faster settings.
[windows]
release mode="shipping":
    {{if mode == "fast" { "set CARGO_PROFILE_RELEASE_LTO=false&& set CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16&& set CARGO_PROFILE_RELEASE_OPT_LEVEL=1&& set CARGO_PROFILE_RELEASE_STRIP=false&& " } else if mode == "shipping" { "" } else { error("Use just release or just release fast") }}}node scripts/desktop/build.mjs --bundles {{if mode == "fast" { fast-bundle } else { release-bundle }}}

# Manage feature worktrees: create <name>, list, remove <name>, or clean (requires Bash).
[unix]
[positional-arguments]
worktree +args:
    bash scripts/worktree.sh "$@"
