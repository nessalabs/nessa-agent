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

script_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Worktrees, and the build cache they share, belong to the original clone —
# never to whichever checkout this copy of the script happens to sit in.
#
# This used to be `dirname $0/..` and nothing more, which was right for as long
# as only people ran it, from the clone. Claude Code runs it from inside a
# worktree: a session already in one that asks for another would otherwise
# nest worktrees inside worktrees and point target/ at the inner one, which is
# a fresh cold build and exactly the problem this script exists to avoid.
#
# `--git-common-dir` is the question "where is the real repository", and every
# linked worktree answers with the original clone's .git. Its parent is that
# clone. A path can come back relative, so it is resolved against the checkout
# that asked.
git_common="$(git -C "$script_root" rev-parse --git-common-dir 2>/dev/null || true)"
[[ -n "$git_common" ]] || { printf '%s\n' "not a git checkout: $script_root" >&2; exit 1; }
[[ "$git_common" = /* ]] || git_common="$script_root/$git_common"
repo_root="$(cd "$git_common/.." && pwd)"
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
  claude-hook     Not for people. Claude Code's WorktreeCreate hook, wired up
                  in .claude/settings.json, so the worktrees it makes for
                  itself share this build cache too.

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

# Point a worktree's target/ at the one the main checkout builds into.
#
# Share the compiled dependencies rather than rebuilding them. A symlink rather
# than CARGO_TARGET_DIR so it applies however cargo is invoked — directly,
# through pnpm, or by the Tauri CLI. The workspace root relocates cargo's build
# dir to <repo>/target (not src-tauri/target), so that is the path linked.
#
# LOAD-BEARING: this is a symlink, not a copy. `cmd_remove` must delete it
# before removing the worktree — see the warning there.
#
# A real directory is never replaced. A worktree that already has one has
# already built into it, and swapping it for a link would strand gigabytes
# where nothing will look for them again.
ensure_shared_target() {
  local path="$1" shared="$repo_root/target"
  mkdir -p "$shared"
  [[ -L "$path/target" ]] && return 0
  if [[ -d "$path/target" ]]; then
    echo "→ $path/target is a real directory; leaving it alone" >&2
    echo "  remove it and re-run to share $repo_name's build cache" >&2
    return 0
  fi
  ln -s "$shared" "$path/target"
  echo "→ sharing target/ with $repo_name" >&2
}

cmd_create() {
  check_name "${1:-}"
  local branch="$1" path
  path="$(worktree_path "$branch")"

  [[ -e "$path" ]] && die "already exists: $path"
  git -C "$repo_root" show-ref --quiet --verify "refs/heads/$branch" \
    && die "branch '$branch' already exists — use it, or pick another name"

  echo "→ worktree $path (branch $branch)"
  git -C "$repo_root" worktree add -b "$branch" "$path" >/dev/null

  ensure_shared_target "$path"

  echo "→ pnpm install"
  (cd "$path" && pnpm install --prefer-offline >/dev/null)

  cat <<EOF

Ready:

  cd $path
  export NESSA_STAGE=dev
  export NESSA_INSTANCE=${branch//\//-}
  pnpm app

Local data for this worktree lands under the app config dir at
dev-\$NESSA_INSTANCE (see docs/adr/done/0005-stage-scoped-local-data.md).

EOF
}

# Claude Code's WorktreeCreate hook.
#
# Claude Code makes its own worktrees — for background agents, isolated
# sessions, and subagents declaring `isolation: worktree` — and its default is
# a plain `git worktree add` under .claude/worktrees/. Correct, and with no
# shared target/: a cold build of ~600 crates and several gigabytes, every
# time. Sixteen such worktrees had accumulated here, ten of them carrying their
# own target/ totalling ~58 GB, before anyone noticed.
#
# Configuring this as the WorktreeCreate hook replaces that default with the
# same worktree `create` makes. The hook's protocol is the whole reason this is
# a separate command rather than a flag: Claude Code writes a JSON object on
# stdin and reads the worktree's path from stdout, so stdout carries the path
# and nothing else, and every line meant for a person goes to stderr.
#
# Two differences from `create`, both because the caller is a program:
#
#   * A branch or worktree that already exists is reused rather than refused.
#     Claude Code chooses the branch name and will ask for a worktree on a
#     branch it made earlier; `create` refuses because for a person that is
#     nearly always a typo.
#   * `pnpm install` failing is reported, not fatal. A Rust-only or docs-only
#     session should not be denied a worktree by the registry being
#     unreachable, and the message says what to run.
#
# A name outside the slug rule is fatal, deliberately. The hook cannot answer
# with "use your default" — it can only print a path — so a name this cannot
# place is better as a loud failure than as a directory nobody expects.
cmd_claude_hook() {
  local payload name path
  payload="$(cat)"
  name="$(printf '%s' "$payload" | jq -r '.name // empty')"
  [[ -n "$name" ]] || die "WorktreeCreate hook: no .name in: $payload"
  check_name "$name"
  path="$(worktree_path "$name")"

  if [[ -d "$path" ]]; then
    echo "→ reusing $path" >&2
  elif git -C "$repo_root" show-ref --quiet --verify "refs/heads/$name"; then
    echo "→ worktree $path (existing branch $name)" >&2
    git -C "$repo_root" worktree add "$path" "$name" >&2
  else
    echo "→ worktree $path (new branch $name)" >&2
    git -C "$repo_root" worktree add -b "$name" "$path" >&2
  fi

  ensure_shared_target "$path"

  if ! (cd "$path" && pnpm install --prefer-offline >&2); then
    echo "→ pnpm install failed; run it in $path before building the frontend" >&2
  fi

  # The hook's answer. Nothing else may reach stdout.
  printf '%s\n' "$path"
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
  create)      shift; cmd_create "$@" ;;
  clean)       cmd_clean ;;
  remove)      shift; cmd_remove "$@" ;;
  list)        git -C "$repo_root" worktree list ;;
  claude-hook) cmd_claude_hook ;;
  *)           usage ;;
esac
