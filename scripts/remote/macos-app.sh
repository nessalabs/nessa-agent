#!/usr/bin/env bash
# Print the path of a macOS desktop host built by CI that this working tree
# would build identically, building one first if none has been downloaded
# (#358).
#
#   scripts/remote/macos-app.sh                 # progress on stderr, the binary's path on stdout
#   scripts/remote/macos-app.sh --key <inputs>  # the key those inputs have in this tree
#
# The snapshot is the tree as it is on disk, untracked files included, made
# through a throwaway index so the branch and the real index are untouched. Each
# build arrives with inputs.txt, the files Cargo compiled it from
# (.github/workflows/macos-dev-app.yml), and is kept under the key
# scripts/remote/host_inputs.py gives those files. A build is reused while its
# key is the same in the current tree, so an edit only the dev server serves
# never asks CI for a host.
#
# A new build is pushed to ci/scratch/<login>, one branch per person, replaced
# every time. The repository is public, and so is the snapshot pushed. It is a
# commit with no parent, so no unpushed history goes with it.
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cache="${XDG_CACHE_HOME:-$HOME/.cache}/nessa/macos-app"
log() { echo "macos-app: $*" >&2; }

scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
cp "$(git -C "$root" rev-parse --absolute-git-dir)/index" "$scratch/index"
GIT_INDEX_FILE="$scratch/index" git -C "$root" add -A
tree="$(GIT_INDEX_FILE="$scratch/index" git -C "$root" write-tree)"
git -C "$root" ls-tree -r -t -z --format='%(objectname) %(path)' "$tree" > "$scratch/listing"

key_in_tree() { python3 "$here/host_inputs.py" key "$scratch/listing" "$1"; }

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

commit="$(git -C "$root" commit-tree "$tree" -m "macOS dev app snapshot of $(git -C "$root" rev-parse --short HEAD)")"
branch="ci/scratch/$(gh api user -q .login)"
log "host inputs changed; pushing snapshot $commit to $branch"
git -C "$root" push -q -f origin "$commit:refs/heads/$branch"

run=""
for _ in $(seq 1 30); do
  # By branch and commit, not --workflow: that needs the file on main.
  run="$(gh run list --branch "$branch" --commit "$commit" --json databaseId,workflowName \
    -q '[.[] | select(.workflowName == "macOS dev app")][0].databaseId // empty')"
  [[ -n "$run" ]] && break
  sleep 2
done
[[ -n "$run" ]] || { log "no workflow run appeared for $commit"; exit 1; }

url="$(gh run view "$run" --json url -q .url)"
log "building in CI: $url"
SECONDS=0
if ! gh run watch "$run" --exit-status --interval 15 >/dev/null; then
  log "the CI build did not succeed ($(gh run view "$run" --json conclusion -q .conclusion)): $url"
  exit 1
fi
log "built in $((SECONDS / 60))m$((SECONDS % 60))s"

gh run download "$run" -n nessa-app -D "$scratch/download"
tar -xzf "$scratch/download/nessa-app.tar.gz" -C "$scratch/download"
rm "$scratch/download/nessa-app.tar.gz"
build="$cache/$(key_in_tree "$scratch/download/inputs.txt")"
mkdir -p "$cache"
rm -rf "$build"
mv "$scratch/download" "$build"

# Keep the five most recently used builds.
find "$cache" -mindepth 1 -maxdepth 1 -type d -print0 \
  | xargs -0 ls -1td \
  | tail -n +6 \
  | while IFS= read -r stale; do rm -rf "$stale"; done
echo "$build/nessa-app"
