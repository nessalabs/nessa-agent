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
  path <name>     Where that branch's worktree is, or would be
  claude-hook     Not for people. Claude Code's WorktreeCreate and
  claude-hook-remove
                  WorktreeRemove hooks, wired up in .claude/settings.json, so
                  the worktrees it makes for itself share this build cache and
                  are cleaned up after.

<name> is a slug: letters, digits, dash, underscore, slash.
USAGE
  exit 1
}

# Keeps the name usable as both a branch and a directory.
#
# The alphabet is also load-bearing for `worktree_path` below: a dot is not in
# it, which is what lets a dot stand for a slash without ambiguity. Adding `.`
# here would make two branches able to want one directory again.
check_name() {
  [[ -n "${1:-}" ]] || usage
  [[ "$1" =~ ^[A-Za-z0-9_/-]+$ ]] || die "invalid name '$1': use letters, digits, - _ /"
}

# The directory a branch belongs in. One branch, one directory, both ways.
#
# A slash is fine in a branch and not in a directory name, so it has to become
# something. It used to become a dash, and that was the bug: a dash is also an
# ordinary character in a name, so `a/b` and `a-b` both arrived at `...-a-b`.
# Nothing downstream could tell them apart, and the hook handed an agent a
# checkout sitting on the other one's branch to commit to.
#
# A dot instead, because `check_name` does not admit one: every dot in a
# directory name got there from a slash, every dash was a dash, and the mapping
# is reversible. Collisions stop being something to detect and become something
# that cannot be expressed.
#
# Worktrees made before this are found by `worktree_for_branch`, which asks git
# where a branch is checked out rather than recomputing a path, so they keep
# working wherever they sit. This only decides where new ones go.
worktree_path() { printf '%s/%s-%s' "$parent" "$repo_name" "${1//\//.}"; }

# Read one string field from the hook payload on stdin.
#
# Node rather than jq: node is already required to run anything here — the
# `pnpm install` the create hook does is node — while jq is not declared
# anywhere in setup and a stock macOS may not have it. A hook that exits 127
# before creating the worktree would take Claude Code's worktrees with it.
#
# Prints nothing when the field is absent or is not a string, which each caller
# turns into its own error naming the payload it actually got.
hook_field() {
  node -e '
    let raw = ""
    process.stdin.on("data", (chunk) => (raw += chunk))
    process.stdin.on("end", () => {
      try {
        const value = JSON.parse(raw)[process.argv[1]]
        if (typeof value === "string") process.stdout.write(value)
      } catch {}
    })
  ' "$1"
}

# The ref a new worktree's branch starts from.
#
# Claude Code's own default is `worktree.baseRef: "fresh"` — the repository's
# default branch on the remote — and replacing its creation means owing it the
# same behaviour. `git worktree add -b <name> <path>` with no start-point does
# something quite different: it branches from whatever HEAD the clone happens to
# be sitting on. That is silent and wrong. A clone parked on a feature branch
# would hand every background agent that branch's commits, and the pull request
# they opened against main would carry them.
#
# Falls back the way the documented behaviour does: origin/HEAD, then the local
# default branch, then this checkout's HEAD when there is no remote at all.
default_base() {
  local ref
  ref="$(git -C "$repo_root" symbolic-ref --quiet refs/remotes/origin/HEAD 2>/dev/null || true)"
  if [[ -n "$ref" ]]; then printf '%s\n' "$ref"; return 0; fi
  for ref in refs/remotes/origin/main refs/remotes/origin/master; do
    if git -C "$repo_root" rev-parse --verify --quiet "$ref" >/dev/null; then
      printf '%s\n' "$ref"
      return 0
    fi
  done
  printf 'HEAD\n'
}

# Where git has a branch checked out, if anywhere. Empty when it is not.
worktree_for_branch() {
  git -C "$repo_root" worktree list --porcelain | awk -v want="refs/heads/$1" '
    /^worktree /  { path = substr($0, 10) }
    /^branch /    { if (substr($0, 8) == want) { print path; exit } }
  '
}

# The branch checked out at a path, if git knows the path as a worktree.
branch_at_worktree() {
  git -C "$repo_root" worktree list --porcelain | awk -v want="$1" '
    /^worktree /  { path = substr($0, 10) }
    /^branch /    { if (path == want) { sub("^refs/heads/", "", $2); print $2; exit } }
  '
}

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
  local path="$1" shared="$repo_root/target" link="$1/target"
  mkdir -p "$shared"

  if [[ -L "$link" ]]; then
    # Already a link, but not necessarily to the cache this checkout builds
    # into: a worktree copied from another clone, or one left dangling by a
    # move, would otherwise go on quietly compiling somewhere else. Replacing a
    # symlink only removes the link.
    [[ "$(readlink "$link")" == "$shared" ]] && return 0
    echo "→ repointing $link at $shared" >&2
    rm -f "$link"
  elif [[ -e "$link" ]]; then
    # A real directory holds artifacts already built into it, and a regular file
    # is something this has no business guessing about. Either way `ln -s` would
    # fail, and under `set -e` that would abort a worktree that git has already
    # created.
    echo "→ $link exists and is not a link to $shared; leaving it alone" >&2
    echo "  remove it and re-run to share $repo_name's build cache" >&2
    return 0
  fi

  ln -s "$shared" "$link"
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
# sessions, and subagents declaring `isolation: worktree` — and its default is a
# plain `git worktree add` with no shared target/: a cold build of ~600 crates
# and several gigabytes, every time. Sixteen had accumulated here, ten carrying
# their own target/, about 58 GB between them.
#
# This replaces that creation step, and replacing it means owing Claude Code the
# behaviour it would have had. Three parts of the payload matter, and taking
# them rather than inventing them is what keeps the two in step:
#
#   * `worktree_path` is where Claude Code was going to put it. Using it keeps
#     the location, the reuse-by-name semantics, and the gitignored
#     `.claude/worktrees/` layout, and means this hook never has to derive a
#     directory from a name — the whole class of two-names-one-directory bugs
#     simply is not reachable from here.
#   * `name` is a worktree slug, not a branch. Claude Code's default branch for
#     it is `worktree-<name>`, so that is what gets created. The prefix is also
#     what stops a slug that happens to match a person's branch from handing an
#     agent that person's checkout.
#   * the base is `default_base`, the repository's default branch, which is what
#     `worktree.baseRef: "fresh"` means and what Claude Code would have used.
#
# The protocol is the reason this is a command of its own rather than a flag on
# `create`: Claude Code writes JSON on stdin and reads the path from stdout, so
# stdout carries the path and nothing else, and every line meant for a person
# goes to stderr. A non-zero exit aborts creation and shows stderr to the user.
#
# `pnpm install` failing is reported, not fatal: a Rust-only or docs-only
# session should not lose its worktree to an unreachable registry.
cmd_claude_hook() {
  local payload name dir branch existing
  payload="$(cat)"
  name="$(printf '%s' "$payload" | hook_field name)"
  dir="$(printf '%s' "$payload" | hook_field worktree_path)"
  [[ -n "$name" ]] || die "WorktreeCreate: no .name in: $payload"

  # Only if Claude Code did not say where. Older versions did not send the
  # field, and a derived path is better than no worktree.
  [[ -n "$dir" ]] || { check_name "$name"; dir="$(worktree_path "$name")"; }
  branch="worktree-$name"

  # Claude Code reuses a worktree whose directory already exists, by name, and
  # the directory it named is the identity here — not a branch, which could be
  # checked out in a person's own worktree.
  if [[ -e "$dir" ]]; then
    echo "→ reusing $dir" >&2
    ensure_shared_target "$dir"
    printf '%s\n' "$dir"
    return 0
  fi

  # A branch cannot be checked out twice, so `worktree add` would fail. This is
  # reachable when a directory was deleted without git being told; the registry
  # entry survives. Reuse the live one, and prune a stale one out of the way.
  existing="$(worktree_for_branch "$branch")"
  if [[ -n "$existing" ]]; then
    if [[ -d "$existing" ]]; then
      echo "→ reusing $existing (branch $branch is checked out there)" >&2
      ensure_shared_target "$existing"
      printf '%s\n' "$existing"
      return 0
    fi
    echo "→ pruning the registry entry for the deleted $existing" >&2
    git -C "$repo_root" worktree prune >&2
  fi

  if git -C "$repo_root" show-ref --quiet --verify "refs/heads/$branch"; then
    echo "→ worktree $dir (existing branch $branch)" >&2
    git -C "$repo_root" worktree add "$dir" "$branch" >&2
  else
    local base
    base="$(default_base)"
    echo "→ worktree $dir (new branch $branch from $base)" >&2
    git -C "$repo_root" worktree add -b "$branch" "$dir" "$base" >&2
  fi

  ensure_shared_target "$dir"

  if ! (cd "$dir" && pnpm install --prefer-offline >&2); then
    echo "→ pnpm install failed; run it in $dir before building the frontend" >&2
  fi

  # The hook's answer. Nothing else may reach stdout.
  printf '%s\n' "$dir"
}

# Claude Code's WorktreeRemove hook.
#
# Required, not optional, once WorktreeCreate is set. Claude Code's periodic
# sweep only removes worktrees carrying the marker it writes itself, and a
# worktree a hook created has no marker — so without this, every worktree made
# here would stay on disk for ever, which is the accumulation this whole change
# is meant to stop.
#
# The symlink comes out first, and that ordering is the same load-bearing one
# `cmd_remove` documents: `target/` points into the main checkout's 99 GB build
# cache, and anything that deletes the directory recursively while the link is
# still in it takes every worktree's compiled artifacts with it.
cmd_claude_hook_remove() {
  local payload dir
  payload="$(cat)"
  dir="$(printf '%s' "$payload" | hook_field worktree_path)"
  [[ -n "$dir" ]] || die "WorktreeRemove: no .worktree_path in: $payload"
  [[ -e "$dir" ]] || return 0

  if [[ -L "$dir/target" ]]; then
    rm -f "$dir/target"
  fi
  git -C "$repo_root" worktree remove --force "$dir" >&2
}

# Where a branch's worktree is, or would be. Prints one path, nothing else.
#
# The mapping in `worktree_path` is the thing collisions used to come from, so
# it is worth being able to look at directly rather than only through the
# directories it happens to create. `worktree-path.test.mjs` asserts over this
# that no two names it accepts can reach one path.
cmd_path() {
  # `--derived` answers from the mapping alone, ignoring where git has anything
  # checked out. worktree-path.test.mjs needs that: asked the ordinary way, a
  # name that happens to be checked out somewhere answers with the registry, and
  # a regressed mapping could still look injective for that pair.
  local derived=""
  [[ "${1:-}" == --derived ]] && { derived=yes; shift; }
  check_name "${1:-}"
  local path
  if [[ -z "$derived" ]]; then
    path="$(worktree_for_branch "$1")"
  fi
  [[ -n "${path:-}" ]] || path="$(worktree_path "$1")"
  printf '%s\n' "$path"
}

cmd_remove() {
  check_name "${1:-}"
  local path
  # Asked of git, not recomputed. A worktree made under the old dash mapping,
  # or moved, is still that branch's worktree and still has the symlink below
  # that has to come out before anything deletes the directory. Recomputing the
  # path would simply not find it and would leave both behind.
  path="$(worktree_for_branch "$1")"
  [[ -n "$path" ]] || path="$(worktree_path "$1")"
  [[ -d "$path" ]] || die "no worktree for '$1' (looked at $path)"

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
  # Only ever the link. `rm -f` on a real directory fails rather than deleting
  # it, which under `set -e` would abort before the removal below and leave both
  # behind — the old `.claude/worktrees/*` copies have real target/ directories.
  if [[ -L "$path/target" ]]; then
    rm -f "$path/target"
  fi
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
  path)        shift; cmd_path "$@" ;;
  claude-hook) cmd_claude_hook ;;
  claude-hook-remove) cmd_claude_hook_remove ;;
  *)           usage ;;
esac
