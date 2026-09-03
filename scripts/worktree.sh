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
#     than corrupt each other.
#
#     A bare `cargo clean` in any worktree does empty it for all of them, and
#     that is not preventable — cargo offers no way to protect a shared target.
#     It is bounded rather than fixed: sccache's cache lives outside the target
#     directory entirely, so the recovery is one ~45s rebuild, not a cold one.
#     `./scripts/worktree.sh clean` is the non-destructive version and is what
#     you almost always want.
#   * pnpm hardlinks from its global store, so `pnpm install` in a new worktree
#     costs seconds and no disk.
#
# Worktrees are created as SIBLINGS of this checkout so they can share this
# repo's workspace `target/`. The design system is vendored by
# scripts/ensure-nessa-ui.mjs, so install no longer depends on a relative path
# sitting next to the checkout.

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
  clean           Rebuild this repo's crates only, keeping dependencies
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
  #
  # The workspace root relocates cargo's build dir to <repo>/target (not
  # src-tauri/target). Symlink that path.
  #
  # LOAD-BEARING: this is a symlink, not a copy. `cmd_remove` must delete it
  # before removing the worktree — see the warning there.
  local shared="$repo_root/target"
  mkdir -p "$shared"
  ln -s "$shared" "$path/target"
  echo "→ sharing target/ with $repo_name"

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

  # ┌──────────────────────────────────────────────────────────────────────┐
  # │ DO NOT CHANGE THE ORDER OF THE NEXT TWO LINES.                       │
  # │                                                                      │
  # │ target/ is a SYMLINK into the main checkout's build cache, shared by │
  # │ every worktree. It must be unlinked BEFORE anything deletes the      │
  # │ worktree directory: a recursive delete would otherwise follow it and │
  # │ wipe gigabytes of compiled artifacts for every worktree at once,     │
  # │ turning every next build into a cold one.                            │
  # │                                                                      │
  # │ `rm -f` on the link itself (no trailing slash, no -r) removes the    │
  # │ link and never touches what it points at. Keep it that way.          │
  # └──────────────────────────────────────────────────────────────────────┘
  rm -f "$path/target"
  git -C "$repo_root" worktree remove --force "$path"
  echo "removed $path (branch '$1' kept)"
}

# The safe counterpart to `cargo clean`. The target directory is shared, so a
# bare clean throws away every worktree's dependency builds; this drops only
# this repo's own crates, which is what is actually stale after a code change,
# and costs ~27s to rebuild instead of minutes.
cmd_clean() {
  echo "→ cargo clean -p nessa-app -p nessa-server (dependencies kept)"
  (cd "$repo_root" && cargo clean -p nessa-app -p nessa-server)
}

case "${1:-}" in
  create) shift; cmd_create "$@" ;;
  clean)  cmd_clean ;;
  remove) shift; cmd_remove "$@" ;;
  list)   git -C "$repo_root" worktree list ;;
  *)      usage ;;
esac
