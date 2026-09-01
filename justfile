# Nessa. The OS-shaped work lives in scripts/launch/; this file is the
# entry so we do not keep a bash script and a cmd script.
#
#   just          list recipes
#   just dev      desktop app in dev mode (falls back to the browser UI)
#   just web      UI in a browser only; window controls no-op
#   just fast     testing-shaped release (macOS .app / Linux .deb / Windows nsis)
#   just release  shipping bundle (macOS .dmg / Linux .deb / Windows nsis)

# cmd so Windows does not need Git's sh. Unix still uses sh.
set windows-shell := ["cmd.exe", "/c"]

# List recipes. Bare `just` is not `just dev`.
[private]
default:
    @just --list

# Desktop app in dev mode (`tauri dev`).
dev:
    node scripts/launch/cli.mjs --dev

# UI in a browser only; window controls no-op.
web:
    node scripts/launch/cli.mjs --web

# Testing-shaped release (macOS .app / Linux .deb / Windows nsis).
fast:
    node scripts/launch/cli.mjs --fast

# Shipping bundle: fat LTO, stripped. macOS .dmg / Linux .deb / Windows nsis.
release:
    node scripts/launch/cli.mjs --release
