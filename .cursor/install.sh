#!/usr/bin/env bash
#
# Cloud Agent install for Nessa.
#
# Nessa's Rust/Tauri half is macOS-only — a transparent, undecorated
# NSVisualEffectView panel, a menu-bar tray, and objc2/AppKit bindings that are
# `cfg`-gated to macOS and referenced unconditionally from `main.rs`. It cannot
# link on the x86_64 Linux Cloud Agent VM. What *does* run here is the frontend:
# `pnpm dev` serves the exact same React chat surface in a plain browser, which
# is the design/iteration path the README documents ("pnpm dev alone opens the
# same UI in a plain browser"). This script prepares that surface.
#
# The @nessa-ui/react design system is consumed as *source* (see vite.config.ts)
# through a relative `link:` dependency:
#
#     ../nessa/.claude/worktrees/imessage-composer-chat-ui/packages/react
#
# Relative to the /workspace checkout that resolves to /nessa/..., so that
# sibling checkout has to exist, and have its own dependencies installed, before
# the app installs. The components the app imports (chat bubbles, pill composer,
# chat tabs, message stream, random avatar) live on nessa_ui's `main` branch.
#
# The script is idempotent: safe to re-run against a warm checkout.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# pnpm 11.9.0 is pinned via package.json "packageManager"; make it active.
corepack prepare pnpm@11.9.0 --activate

# --- The design-system checkout (the link: target) -------------------------
ds_root="/nessa"
ds="$ds_root/.claude/worktrees/imessage-composer-chat-ui"
ds_ref="main"

# /nessa sits next to /workspace at the filesystem root, so it needs one
# privileged mkdir; everything under it is owned by the install user afterward.
if [ ! -d "$ds_root" ]; then
  if ! mkdir -p "$ds_root" 2>/dev/null; then
    sudo mkdir -p "$ds_root"
    sudo chown "$(id -un):$(id -gn)" "$ds_root"
  fi
fi

# Reuse the workspace's already-authenticated remote, swapping the repo name, so
# no token handling lives here. nessa_ui is listed in repositoryDependencies so
# the environment's generated GitHub token can reach it.
ds_url="$(git -C "$repo_root" remote get-url origin \
  | sed 's#/nessa-agent\(\.git\)\{0,1\}$#/nessa_ui#')"

if [ ! -d "$ds/.git" ]; then
  # No vendored checkout yet: this must succeed, so a missing
  # repositoryDependencies grant surfaces as a hard failure here.
  rm -rf "$ds"
  git clone --depth 1 --branch "$ds_ref" "$ds_url" "$ds"
else
  # Already vendored (for example, restored from a snapshot). Refresh to the
  # branch tip when reachable, but keep the working checkout if the remote is
  # briefly unreachable rather than failing the whole install.
  git -C "$ds" remote set-url origin "$ds_url"
  if git -C "$ds" fetch --depth 1 origin "$ds_ref" 2>/dev/null; then
    git -C "$ds" checkout -q "$ds_ref"
    git -C "$ds" reset --hard "origin/$ds_ref"
  else
    echo "note: could not refresh $ds from origin; using existing checkout" >&2
  fi
fi

# Install just the react package and its dependencies (not the storybook app),
# so the app's Vite can resolve the design system's own imports when it
# transforms its source.
( cd "$ds" && pnpm install --filter "@nessa-ui/react..." --prefer-offline )

# --- The app ---------------------------------------------------------------
( cd "$repo_root" && pnpm install --prefer-offline )
