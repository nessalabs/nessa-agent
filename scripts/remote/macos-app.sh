#!/usr/bin/env bash
# Print the path of a macOS desktop host built by CI that this working tree
# would build identically, building one first if none has been downloaded
# (#358).
#
#   scripts/remote/macos-app.sh               # progress on stderr, the binary's path on stdout
#   scripts/remote/macos-app.sh --key <inputs>  # the key those inputs have in this tree
#
# The snapshot is the tree as it is on disk, untracked files included, made
# through a throwaway index so the branch and the real index are untouched. Each
# build arrives with inputs.txt, the files it was compiled from as Cargo tracks
# them (.github/workflows/macos-dev-app.yml), and is kept under a key hashed from
# those files' contents. A build is reused while its inputs hash the same in the
# current tree, so an edit only the dev server serves never asks CI for a host.
#
# A new build is pushed to ci/scratch/<login>, one branch per person, replaced
# every time. This repository is public: so is whatever that snapshot contains.
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
cache="${XDG_CACHE_HOME:-$HOME/.cache}/nessa/macos-app"
log() { echo "macos-app: $*" >&2; }

index="$(mktemp)"
listing="$(mktemp)"
trap 'rm -f "$index" "$listing"' EXIT
cp "$(git -C "$root" rev-parse --absolute-git-dir)/index" "$index"
GIT_INDEX_FILE="$index" git -C "$root" add -A
tree="$(GIT_INDEX_FILE="$index" git -C "$root" write-tree)"
# Directories too (-t): a build script may watch one, Tauri's capabilities/
# among them, and its tree id changes when anything inside it is added or edited.
git -C "$root" ls-tree -r -t --format='%(path) %(objectname)' "$tree" > "$listing"

# The key of a build's inputs in this tree: each path with its object id, or
# `absent`, hashed together.
key_in_tree() {
  awk 'NR == FNR { id[$1] = $2; next } { print $0, ($0 in id ? id[$0] : "absent") }' \
    "$listing" "$1" | shasum | cut -c1-40
}

if [[ "${1:-}" == --key ]]; then
  key_in_tree "$2"
  exit
fi

for inputs in "$cache"/*/inputs.txt; do
  [[ -f "$inputs" ]] || continue
  build="$(dirname "$inputs")"
  if [[ "$(key_in_tree "$inputs")" == "$(basename "$build")" ]]; then
    log "reusing $(basename "$build"): no host input changed"
    touch "$build"
    echo "$build/nessa-app"
    exit
  fi
done

commit="$(git -C "$root" commit-tree "$tree" -p HEAD -m "macOS dev app snapshot of $(git -C "$root" rev-parse --short HEAD)")"
branch="ci/scratch/$(gh api user -q .login)"
log "host inputs changed; pushing snapshot $commit to $branch"
git -C "$root" push -q -f origin "$commit:refs/heads/$branch"

run=""
for _ in $(seq 1 30); do
  # By branch and commit, not --workflow: that needs the file on main.
  run="$(gh run list --branch "$branch" --commit "$commit" \
    --json databaseId,workflowName -q '.[] | select(.workflowName == "macOS dev app") | .databaseId')"
  [[ -n "$run" ]] && break
  sleep 2
done
[[ -n "$run" ]] || { log "no workflow run appeared for $commit"; exit 1; }

log "building in CI: $(gh run view "$run" --json url -q .url)"
SECONDS=0
gh run watch "$run" --exit-status --interval 15 >/dev/null
log "built in $((SECONDS / 60))m$((SECONDS % 60))s"

download="$(mktemp -d)"
gh run download "$run" -n nessa-app -D "$download"
tar -xzf "$download/nessa-app.tar.gz" -C "$download"
build="$cache/$(key_in_tree "$download/inputs.txt")"
rm -rf "$build"
mkdir -p "$cache"
mv "$download" "$build"
rm "$build/nessa-app.tar.gz"

# Keep the five most recently used builds.
ls -1td "$cache"/*/ | tail -n +6 | xargs rm -rf
echo "$build/nessa-app"
