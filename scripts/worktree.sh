#!/usr/bin/env bash
#
# Feature worktrees that do not pay for a rebuild.
#
# Every agent working on a feature starts here:
#
#     ./scripts/worktree.sh create add-something
#     ./scripts/worktree.sh list
#     ./scripts/worktree.sh remove add-something
#
# Two things make a fresh worktree cheap:
#
#   * The Rust target directory is shared with the main checkout, so cargo
#     reuses ~500 already-compiled dependency crates instead of starting over.
#     Cargo takes a lock on it, so two worktrees building at once queue rather
#     than corrupt each other — but a `cargo clean` in one empties it for all.
#   * pnpm hardlinks from its global store, so `pnpm install` in a new worktree
#     costs seconds and no disk.
#
# Worktrees are created as SIBLINGS of this checkout, which is load-bearing:
# the design system is a relative `link:../nessa/…` dependency, so a worktree
# nested any deeper would resolve that path to nothing and fail to install.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
repo_name="$(basename "$repo_root")"
parent="$(dirname "$repo_root")"

die() { printf '%s\n' "$*" >&2; exit 1; }

usage() {
  cat >&2 <<'USAGE'
usage: ./scripts/worktree.sh <command> [name]

  create <name>   New branch <name> in a sibling worktree, ready to build
  remove <name>   Delete that worktree (the branch is kept)
  list            Show every worktree

<name> is a slug: letters, digits, dash, underscore, slash.
USAGE
  exit 1
}

# Keeps the name usable as both a branch and a directory.
check_name() {
  [[ -n "${1:-}" ]] || usage
  [[ "$1" =~ ^[A-Za-z0-9_/-]+$ ]] || die "invalid name '$1': use letters, digits, - _ /"
}

# A slash is fine in a branch but not in a directory name.
worktree_path() { printf '%s/%s-%s' "$parent" "$repo_name" "${1//\//-}"; }

cmd_create() {
  check_name "${1:-}"
  local branch="$1" path
  path="$(worktree_path "$branch")"

  [[ -e "$path" ]] && die "already exists: $path"
  git -C "$repo_root" show-ref --quiet --verify "refs/heads/$branch" \
    && die "branch '$branch' already exists — use it, or pick another name"

  echo "→ worktree $path (branch $branch)"
  git -C "$repo_root" worktree add -b "$branch" "$path" >/dev/null

  # Share the compiled dependencies rather than rebuilding them. A symlink
  # rather than CARGO_TARGET_DIR so it applies however cargo is invoked —
  # directly, through pnpm, or by the Tauri CLI.
  local shared="$repo_root/src-tauri/target"
  mkdir -p "$shared"
  ln -s "$shared" "$path/src-tauri/target"
  echo "→ sharing src-tauri/target with $repo_name"

  echo "→ pnpm install"
  (cd "$path" && pnpm install --prefer-offline >/dev/null)

  cat <<EOF

Ready:

  cd $path
  pnpm app

EOF
}

cmd_remove() {
  check_name "${1:-}"
  local path
  path="$(worktree_path "$1")"
  [[ -d "$path" ]] || die "no worktree at $path"

  # Drop the symlink first: 'git worktree remove' would otherwise refuse, and
  # a careless recursive delete here would follow it into the shared target.
  rm -f "$path/src-tauri/target"
  git -C "$repo_root" worktree remove --force "$path"
  echo "removed $path (branch '$1' kept)"
}

case "${1:-}" in
  create) shift; cmd_create "$@" ;;
  remove) shift; cmd_remove "$@" ;;
  list)   git -C "$repo_root" worktree list ;;
  *)      usage ;;
esac
