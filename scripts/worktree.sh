#!/usr/bin/env bash
#
# Feature worktrees with checkout-owned build output.
#
# Every agent working on a feature starts here:
#
#     ./scripts/worktree.sh create add-something
#     ./scripts/worktree.sh list
#     ./scripts/worktree.sh remove add-something
#
# Two things keep a fresh worktree practical:
#
#   * Every checkout owns its Rust target directory. Cargo artifacts include
#     source-dependent workspace output, so a target shared by divergent
#     worktrees can leave one checkout launching another checkout's binary.
#     sccache can still reuse compiled dependencies across those directories
#     when RUSTC_WRAPPER=sccache is set.
#   * pnpm hardlinks from its global store, so `pnpm install` in a new worktree
#     costs seconds and no disk.
#
# Worktrees are created as SIBLINGS of this checkout. The design system is vendored by
# scripts/ensure-nessa-ui.mjs, so install no longer depends on a relative path
# sitting next to the checkout.

set -euo pipefail

# `pwd -P` throughout: git records a worktree by its physical path, and these
# paths are compared against that registry as strings. Reached through a
# symlinked parent, a logical path would match nothing — the hook would refuse
# to reopen its own worktree, and would skip deleting its branch on removal.
script_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"

# Worktree registration belongs to the original clone, never to whichever
# checkout this copy of the script happens to sit in.
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
repo_root="$(cd "$git_common/.." && pwd -P)"
repo_name="$(basename "$repo_root")"
parent="$(dirname "$repo_root")"

die() { printf '%s\n' "$*" >&2; exit 1; }

usage() {
  cat >&2 <<'USAGE'
usage: ./scripts/worktree.sh <command> [name]

  create <name>   New branch <name> in a sibling worktree, ready to build
  remove <name>   Delete that worktree (the branch is kept)
  isolate         Replace this checkout's old shared target link with an empty,
                  checkout-owned target directory; shared artifacts are kept
  clean           Rebuild this checkout's crates only, keeping dependencies
  list            Show every worktree
  path <name>     Where that branch's worktree is, or would be
  claude-hook     Not for people. Claude Code's WorktreeCreate and
  claude-hook-remove
                  WorktreeRemove hooks, wired up in .claude/settings.json, so
                  the worktrees it makes for itself use isolated build output
                  and are cleaned up after.

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

# A worktree slug Claude Code sent, checked for what it will be used as: a path.
#
# Its alphabet is not this script's. Claude Code accepts `[a-zA-Z0-9._-]` per
# slash-separated segment, up to 64 characters — dots included — so names it
# creates happily, such as `v1.2` or `release/1.0`, are not names `check_name`
# would allow. Rejecting them here would refuse worktrees stock Claude Code
# makes without complaint.
#
# What actually matters is that the slug becomes a directory under
# `.claude/worktrees/`, so the checks are the ones a path needs: no empty or
# dot segments to climb out of it, no leading dash to be read as an option.
check_hook_name() {
  local name="$1" segment
  local -a segments=()
  [[ -n "$name" ]] || die "WorktreeCreate: empty name"
  [[ "$name" != -* ]] || die "WorktreeCreate: name may not start with a dash: $name"
  # A trailing slash leaves no empty field to catch below, so it is caught here.
  [[ "$name" != */ ]] || die "WorktreeCreate: name may not end with a slash: $name"
  # `read` below stops at the first newline, so anything after one would go
  # unexamined. Refused rather than half-checked. (A name whose only newline is
  # a trailing one never reaches here: the command substitution that read the
  # payload has already stripped it, leaving a slug this rule then judges
  # normally.)
  [[ "$name" != *$'\n'* ]] || die "WorktreeCreate: name may not contain a newline"
  # `read -ra`, not `for segment in $name`: an unquoted expansion is also a glob,
  # so a name of `*` would have been replaced by the contents of whatever
  # directory the hook happened to run in and then matched the alphabet below
  # one innocent-looking filename at a time. Splitting without expanding means
  # the rule reads the name it was given.
  IFS=/ read -ra segments <<< "$name"
  for segment in "${segments[@]}"; do
    [[ -n "$segment" ]] || die "WorktreeCreate: empty path segment in name: $name"
    [[ "$segment" != . && "$segment" != .. ]] \
      || die "WorktreeCreate: name may not contain a '.' or '..' segment: $name"
    [[ "$segment" =~ ^[A-Za-z0-9._-]{1,64}$ ]] \
      || die "WorktreeCreate: segment '$segment' is not a worktree name: $name"
  done
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
  # `symbolic-ref` happily reports a target that no longer exists — a remote
  # renamed master to main without `git remote set-head` leaves exactly that —
  # and `worktree add` on a dangling ref fails, which would have broken every
  # creation until someone repaired it by hand. Verified before it is used, so
  # a stale pointer falls through to the branches below instead.
  if [[ -n "$ref" ]] && git -C "$repo_root" rev-parse --verify --quiet "$ref" >/dev/null; then
    printf '%s\n' "$ref"
    return 0
  fi
  for ref in refs/remotes/origin/main refs/remotes/origin/master; do
    if git -C "$repo_root" rev-parse --verify --quiet "$ref" >/dev/null; then
      printf '%s\n' "$ref"
      return 0
    fi
  done
  printf 'HEAD\n'
}

# Each of the three readers below consumes git's whole output rather than
# stopping at the answer. `exit` in awk looks like the obvious economy and is a
# trap here: once the registry outgrows the pipe buffer, awk leaving early kills
# `git worktree list` with SIGPIPE, `pipefail` turns that into the pipeline's
# status, and `set -e` ends the script inside a `$(...)`. The failure arrives as
# a worktree that "git does not know" and a removal that stops before it
# removes, which is self-reinforcing: past that size the cleanup that keeps the
# registry small is the thing that breaks. Reading to the end costs nothing at
# any plausible number of worktrees.

# Where git has a branch checked out, if anywhere. Empty when it is not.
worktree_for_branch() {
  git -C "$repo_root" worktree list --porcelain | awk -v want="refs/heads/$1" '
    /^worktree /  { path = substr($0, 10) }
    /^branch /    { if (!found && substr($0, 8) == want) { answer = path; found = 1 } }
    END           { if (found) print answer }
  '
}

# Whether git knows a path as one of its worktrees, branch or not. A worktree
# left on a detached HEAD has no `branch` line, and reading only that made it
# indistinguishable from a stray directory.
is_registered_worktree() {
  git -C "$repo_root" worktree list --porcelain | awk -v want="$1" '
    /^worktree /  { if (substr($0, 10) == want) found = 1 }
    END           { exit(found ? 0 : 1) }
  '
}

# The branch checked out at a path, if git knows the path as a worktree.
branch_at_worktree() {
  git -C "$repo_root" worktree list --porcelain | awk -v want="$1" '
    /^worktree /  { path = substr($0, 10) }
    /^branch /    { if (!found && path == want) { sub("^refs/heads/", "", $2); answer = $2; found = 1 } }
    END           { if (found) print answer }
  '
}

# Require build output to belong to the checkout whose source Cargo reads.
# A directory is the ownership boundary: relative target/debug paths used by
# Cargo, Tauri, and the smoke scripts all resolve inside the same checkout.
ensure_local_target() {
  local path="$1" target="$1/target"
  if [[ -L "$target" ]]; then
    echo "→ $target is a symbolic link; refusing shared or foreign build output" >&2
    echo "  run 'just worktree isolate' from that checkout first" >&2
    return 1
  fi
  if [[ -e "$target" && ! -d "$target" ]]; then
    echo "→ $target exists and is not a directory; refusing to replace it" >&2
    return 1
  fi
  mkdir -p "$target"
}

# Existing worktrees opt in one at a time. Only the exact link created by the
# former recipe is replaced. Its target is left untouched, so migration cannot
# discard the original clone's artifacts. Any other link is refused because
# this script cannot establish who owns it.
cmd_isolate() {
  local target="$script_root/target" former_shared="$repo_root/target"
  is_registered_worktree "$script_root" \
    || die "$script_root is not a registered worktree"

  if [[ -L "$target" ]]; then
    [[ "$(readlink "$target")" == "$former_shared" ]] \
      || die "$target points somewhere other than the former shared target; leaving it unchanged"
    rm -f "$target"
    mkdir "$target"
    echo "isolated $target; kept $former_shared unchanged"
    return 0
  fi
  if [[ -d "$target" ]]; then
    echo "$target is already checkout-owned"
    return 0
  fi
  [[ ! -e "$target" ]] || die "$target exists and is not a directory; leaving it unchanged"
  mkdir "$target"
  echo "created checkout-owned $target"
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

  ensure_local_target "$path"

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
# plain `git worktree add`. This hook preserves Claude Code's placement,
# branch, dependency-installation, and cleanup behavior while ensuring build
# output stays with the source tree that produced it.
#
# This replaces that creation step, which means owing Claude Code the behaviour
# it would otherwise have had. The payload carries one field, `name`, and it is
# a worktree slug — not a branch, and not a path. Everything else is this
# script's job to match:
#
#   * The worktree goes under `.claude/worktrees/<name>`, which is where Claude
#     Code puts its own and is already gitignored. Deliberately NOT the sibling
#     directory `create` uses: those are people's worktrees, and the two naming
#     schemes are not the same function. A slug is allowed dots, `create` is
#     not, and mapping both into one namespace is how an agent would come to be
#     handed a person's checkout to commit to. Separate namespaces cannot.
#   * The branch is `worktree-<name>`, which is what Claude Code's own default
#     names it, so its expectations about the branch still hold.
#   * The base is the repository's default branch — `worktree.baseRef: "fresh"`,
#     the default. `git worktree add -b` with no start-point instead branches
#     from whatever HEAD the clone is sitting on; a clone parked on a feature
#     branch would have handed every agent that branch's commits. `--no-track`
#     because the base is a remote branch and inheriting it as upstream makes
#     the agent's `git push` and `git pull` act on main.
#
# The protocol is why this is a command of its own rather than a flag: Claude
# Code writes JSON on stdin and reads the path from stdout, so stdout carries
# the path and nothing else and every line meant for a person goes to stderr. A
# non-zero exit aborts creation and shows stderr to the user.
#
# Installing dependencies is an improvement to a worktree, not a condition of
# one: a failure is not allowed to cost a worktree git has already made. Build
# isolation is a correctness condition and a reopened legacy link is refused.
cmd_claude_hook() {
  local payload name dir branch existing occupant base
  payload="$(cat)"
  name="$(printf '%s' "$payload" | hook_field name)"
  [[ -n "$name" ]] || die "WorktreeCreate: no .name in: $payload"
  check_hook_name "$name"

  dir="$repo_root/.claude/worktrees/$name"
  branch="worktree-$name"

  # Claude Code reopens a worktree whose directory already exists, so this does
  # too — but only when git agrees it is a worktree. A bare directory there
  # would be handed back as a checkout that is not one, and Claude Code refuses
  # those with a message about git metadata rather than about this.
  if [[ -e "$dir" ]]; then
    is_registered_worktree "$dir" \
      || die "$dir exists but git does not know it as a worktree.
Remove it and let this recreate it, or pick another name."
    occupant="$(branch_at_worktree "$dir")"
    echo "→ reusing $dir (on ${occupant:-a detached HEAD})" >&2
    ensure_local_target "$dir"
    printf '%s\n' "$dir"
    return 0
  fi

  # A branch cannot be checked out twice, so `worktree add` would fail. Reachable
  # when a directory was deleted without git being told: the registry entry
  # outlives it.
  existing="$(worktree_for_branch "$branch")"
  if [[ -n "$existing" ]]; then
    if [[ -d "$existing" ]]; then
      echo "→ reusing $existing (branch $branch is checked out there)" >&2
      ensure_local_target "$existing"
      printf '%s\n' "$existing"
      return 0
    fi
    echo "→ pruning the registry entry for the deleted $existing" >&2
    git -C "$repo_root" worktree prune >&2
  fi

  mkdir -p "$(dirname "$dir")"
  if git -C "$repo_root" show-ref --quiet --verify "refs/heads/$branch"; then
    echo "→ worktree $dir (existing branch $branch)" >&2
    git -C "$repo_root" worktree add "$dir" "$branch" >&2
  else
    base="$(default_base)"
    echo "→ worktree $dir (new branch $branch from $base)" >&2
    git -C "$repo_root" worktree add --no-track -b "$branch" "$dir" "$base" >&2
  fi

  ensure_local_target "$dir"

  if ! (cd "$dir" && pnpm install --prefer-offline >&2); then
    echo "→ pnpm install failed; run it in $dir before building the frontend" >&2
  fi

  # The hook's answer. Nothing else may reach stdout.
  printf '%s\n' "$dir"
}

# Claude Code's WorktreeRemove hook.
#
# Required, not optional, once WorktreeCreate is set. Claude Code's periodic
# sweep only removes worktrees carrying a marker it writes itself, and one a
# hook created has none — so without this, every worktree made here would stay
# on disk for ever, which is the accumulation this change exists to stop. For
# the same reason Claude Code skips its own branch cleanup on this path, so the
# branch is this hook's to delete too.
#
# `worktree_path` is the only field this event carries; `name` is not sent.
#
# A legacy target symlink comes out before removal. Git then removes the
# checkout-owned directory normally; it never receives a path through which it
# could touch the original clone's target directory.
#
# The branch goes too, but only when it holds nothing. `git branch -d` is the
# obvious way to ask that and the wrong one: it means "merged into the branch
# this clone happens to have checked out", so a clone sitting on an unrelated
# feature branch refuses to delete even a worktree branch with no commits at
# all — every branch would accumulate, which is half the problem this hook is
# for. The question worth asking is whether the branch has any commit the base
# does not already have, and `merge-base --is-ancestor` asks exactly that.
cmd_claude_hook_remove() {
  local payload dir branch
  payload="$(cat)"
  dir="$(printf '%s' "$payload" | hook_field worktree_path)"
  [[ -n "$dir" ]] || die "WorktreeRemove: no .worktree_path in: $payload"
  # git records no trailing slash, and the branch lookup below is a string
  # compare against that record.
  while [[ "$dir" == */ && "$dir" != / ]]; do dir="${dir%/}"; done
  [[ -e "$dir" ]] || return 0
  # And resolved, for the same reason `repo_root` is: the lookups below compare
  # against git's registry, which records realpaths. A logical path reaching
  # here — a worktree made before this hook, under a symlinked parent — would
  # be removed and its branch quietly kept.
  dir="$(cd "$dir" && pwd -P)"

  branch="$(branch_at_worktree "$dir")"
  # A worktree left on a detached HEAD — an agent that ran `git bisect`, or
  # checked out a sha — has no branch line for git to answer with, and reading
  # only that would leave its branch behind for ever. Under
  # `.claude/worktrees/` the branch is known from the path: this hook chose
  # both. Nothing is deleted on the strength of that alone; it still has to
  # exist and still has to hold nothing the base does not.
  if [[ -z "$branch" && "$dir" == "$repo_root/.claude/worktrees/"* ]]; then
    branch="worktree-${dir#"$repo_root/.claude/worktrees/"}"
    git -C "$repo_root" show-ref --quiet --verify "refs/heads/$branch" || branch=""
  fi
  if [[ -L "$dir/target" ]]; then
    rm -f "$dir/target"
  fi
  git -C "$repo_root" worktree remove --force "$dir" >&2
  # A nested name (`a/b`) leaves `a/` behind. `rmdir` removes only empty
  # directories, but `-p` on an absolute path keeps walking up until something
  # is not empty and has no notion of a boundary — it would take `.claude/`
  # itself, or, for a worktree adopted from somewhere else entirely, an empty
  # parent outside the repository that this hook never created. So: only under
  # the directory this hook owns, and run from inside it on the part below it,
  # which cannot climb past where it started.
  local owned="$repo_root/.claude/worktrees" inner
  if [[ "$dir" == "$owned"/* ]]; then
    inner="$(dirname "${dir#"$owned/"}")"
    if [[ "$inner" != "." ]]; then
      (cd "$owned" && rmdir -p "$inner" 2>/dev/null) || true
    fi
  fi

  # Only branches this hook names, and only when the base already contains
  # everything on them. `-D` is safe here because that test, not git's default
  # one, is the question actually being asked.
  if [[ "$branch" == worktree-* ]]; then
    local base
    base="$(default_base)"
    if git -C "$repo_root" merge-base --is-ancestor "refs/heads/$branch" "$base" 2>/dev/null; then
      # Non-fatal: the worktree is already gone, and a branch git declines to
      # delete — one checked out in another worktree — is not a failed removal.
      git -C "$repo_root" branch -D "$branch" >&2 \
        || echo "→ could not delete branch $branch" >&2
    else
      echo "→ keeping branch $branch; it has commits $base does not" >&2
    fi
  fi
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

  # Legacy worktrees may still link to the original clone's target. Unlink the
  # path itself before asking git to remove the checkout; never traverse it.
  if [[ -L "$path/target" ]]; then
    rm -f "$path/target"
  fi
  git -C "$repo_root" worktree remove --force "$path"
  echo "removed $path (branch '$1' kept)"
}

# The package-scoped counterpart to `cargo clean`. Cargo's effective target is
# the authority here: CARGO_TARGET_DIR and .cargo/config.toml both override the
# ordinary checkout-local path. Checking only $script_root/target before running
# Cargo would authorize one directory and let Cargo delete another.
cmd_clean() {
  local effective_target
  effective_target="$({
    cd "$script_root"
    cargo metadata --no-deps --format-version 1
  } | node -e '
    let raw = ""
    process.stdin.on("data", (chunk) => (raw += chunk))
    process.stdin.on("end", () => {
      const target = JSON.parse(raw).target_directory
      if (typeof target !== "string" || target.length === 0) process.exit(1)
      process.stdout.write(target)
    })
  ')" || die "could not resolve Cargo's effective target directory; nothing was cleaned"

  [[ "$effective_target" == "$script_root/target" ]] || die \
    "refusing to clean $effective_target; this checkout owns $script_root/target
Unset CARGO_TARGET_DIR or remove the target-dir override before using this recipe.
Clean an intentional external target explicitly, from the process that owns it."
  ensure_local_target "$script_root"
  echo "→ cargo clean -p nessa-app -p nessa-server (dependencies kept)"
  (cd "$script_root" && cargo clean -p nessa-app -p nessa-server)
}

case "${1:-}" in
  create)      shift; cmd_create "$@" ;;
  isolate)     cmd_isolate ;;
  clean)       cmd_clean ;;
  remove)      shift; cmd_remove "$@" ;;
  list)        git -C "$repo_root" worktree list ;;
  path)        shift; cmd_path "$@" ;;
  check-hook-name) shift; check_hook_name "${1:-}" ;;
  claude-hook) cmd_claude_hook ;;
  claude-hook-remove) cmd_claude_hook_remove ;;
  *)           usage ;;
esac
