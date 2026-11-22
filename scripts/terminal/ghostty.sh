#!/usr/bin/env bash
# Builds the Ghostty terminal library that the pinned pty-runtime links, so
# the gateway can be built with terminals (docs/design/terminals.md):
#
#   eval "$(scripts/terminal/ghostty.sh)"
#   cargo build -p nessa-server --features nessa-server/terminals
#
# pty-runtime's own bootstrap fetches its pinned Zig and Ghostty and checks
# both by digest. Everything lands in .vendor/pty-runtime/<rev>, so a second
# run with the same pin builds nothing. Prints the one line to export.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
rev="$(sed -n 's/^pty-runtime = .*rev = "\([0-9a-f]\{40\}\)".*/\1/p' "$root/crates/nessa-gateway/Cargo.toml")"
if [[ -z "$rev" ]]; then
  echo "could not read the pty-runtime pin from crates/nessa-gateway/Cargo.toml" >&2
  exit 1
fi
checkout="$root/.vendor/pty-runtime/$rev"
if [[ ! -d "$checkout/.git" ]]; then
  rm -rf "$checkout"
  mkdir -p "$checkout"
  git -C "$checkout" init -q
  git -C "$checkout" remote add origin https://github.com/nessalabs/pty-runtime
  git -C "$checkout" fetch -q --depth 1 origin "$rev"
  git -C "$checkout" checkout -q FETCH_HEAD
fi
cache="$checkout/work/experiment-cache"
source="$( (find "$cache" -maxdepth 1 -type d -name 'ghostty-*' 2>/dev/null || true) | head -n 1)"
if [[ -z "$source" || ! -f "$source/zig-out/lib/libghostty-vt.a" ]]; then
  python3 "$checkout/scripts/native/bootstrap.py" --cache "$cache" \
    --jobs "${NESSA_GHOSTTY_JOBS:-4}" >&2
  source="$(find "$cache" -maxdepth 1 -type d -name 'ghostty-*' | head -n 1)"
fi
if [[ ! -f "$source/zig-out/lib/libghostty-vt.a" ]]; then
  echo "the Ghostty build left no library under $cache" >&2
  exit 1
fi
echo "export PTY_RUNTIME_GHOSTTY_SOURCE=$source"
