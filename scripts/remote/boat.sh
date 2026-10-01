#!/usr/bin/env bash
# Run this checkout's commands on a Linux Boat sandbox while the files and the
# agents editing them stay on this machine. Cargo's build output, Linux
# node_modules, and the dev servers live there; only macOS app builds run here.
# `just remote <recipe>` is the usual entry; this script is its plumbing.
#
#   scripts/remote/boat.sh exec [--port N]... <cmd...>  # run there; each port reachable here
#   scripts/remote/boat.sh sync [--watch]  # push this checkout
#   scripts/remote/boat.sh shell           # interactive shell in the remote copy
#   scripts/remote/boat.sh put <here> <there>  # copy to the sandbox (there: relative to its home)
#   scripts/remote/boat.sh get <there> <here>  # copy from the sandbox
#   scripts/remote/boat.sh proxy           # SOCKS on 127.0.0.1:1080 to reach any sandbox port
#   scripts/remote/boat.sh stop            # snapshot and stop the builder (keeps its cache)
#   scripts/remote/boat.sh forget          # delete the builder and its cache
#
# One builder sandbox serves every checkout and worktree. Each checkout syncs to
# its own directory; all of them share one remote CARGO_TARGET_DIR, so
# dependencies compile once. Stopping snapshots the disk, so the next run
# resumes warm.
set -euo pipefail

state_dir="${XDG_CONFIG_HOME:-$HOME/.config}/nessa"
state_file="$state_dir/boat-builder"
root="$(git rev-parse --show-toplevel)"
checkout="$(basename "$root")"

log() { echo "boat: $*" >&2; }

builder() { [[ -f "$state_file" ]] && cat "$state_file"; }

info_field() { sed -n "s/.*\"$1\":\"\\([^\"]*\\)\".*/\\1/p"; }

ensure_builder() {
  local id state
  if boat status 2>/dev/null | grep -q '"loginState":"signed out"'; then
    log "Boat is signed out; run \`boat login\` with the method this account uses"
    exit 1
  fi
  id="$(builder || true)"
  if [[ -n "$id" ]]; then
    state="$(boat info "$id" --json 2>/dev/null | info_field state)"
    case "$state" in
      ready | idle | running) echo "$id"; return ;;
      stopped | stopping)
        log "resuming builder $id"
        boat resume "$id" >/dev/null
        echo "$id"; return ;;
      *) log "builder $id is '${state:-gone}', creating a new one" ;;
    esac
  fi
  log "creating builder sandbox"
  # With the account's environment, so the agents the dev gateway runs there
  # (claude, codex) are signed in. That also brings its GitHub token and secrets.
  id="$(boat new --type large --json | grep '"event":"ready"' | info_field id)"
  [[ -n "$id" ]] || { log "sandbox creation failed"; exit 1; }
  mkdir -p "$state_dir"
  echo "$id" > "$state_file"
  # mold links in parallel; nextest runs test binaries in parallel.
  boat ssh "$id" 'sudo -n apt-get install -y -qq mold microsocks >/dev/null \
    && mkdir -p ~/.cargo/bin \
    && curl -fsSL https://just.systems/install.sh | bash -s -- --to ~/.cargo/bin \
    && curl -LsSf https://get.nexte.st/latest/linux | tar zxf - -C ~/.cargo/bin \
    && printf "[target.x86_64-unknown-linux-gnu]\nlinker = \"clang\"\nrustflags = [\"-C\", \"link-arg=-fuse-ld=mold\"]\n" > ~/.cargo/config.toml \
    && sudo -n corepack enable'
  echo "$id"
}

# `boat ssh` costs ~4.5 s per call through the Boat API, too slow to sync on
# save. Open one plain SSH master with Boat's key and pinned host key, and send
# every later command through it.
control="$state_dir/boat-ssh.sock"
ssh_opts=(-S "$control")

connect() {
  local id="$1" endpoint known
  ssh "${ssh_opts[@]}" -O check boat 2>/dev/null && return
  endpoint="$(boat info "$id" --json | info_field sshEndpoint)"
  [[ -n "$endpoint" ]] || { log "no SSH endpoint for $id"; exit 1; }
  # Boat pins each sandbox's host key in its own file; the alias is the name.
  for known in "$HOME"/.ssh/ascii_sandbox_known_hosts/*; do
    ssh -fN -M "${ssh_opts[@]}" -o ControlPersist=2h -o BatchMode=yes \
      -p "${endpoint##*:}" -i "$HOME/.ssh/ascii_box_ed25519" \
      -o StrictHostKeyChecking=yes -o UserKnownHostsFile="$known" \
      -o GlobalKnownHostsFile=/dev/null \
      -o HostKeyAlias="ascii-sandbox-$(basename "$known")" \
      "user@${endpoint%:*}" 2>/dev/null && return
  done
  # A host key Boat has not pinned yet: one `boat ssh` records it.
  boat ssh "$id" true
  connect "$id"
}

run() { ssh "${ssh_opts[@]}" boat "$@"; }

forward() { ssh "${ssh_opts[@]}" -O "$1" -L "127.0.0.1:$2:127.0.0.1:$2" boat 2>/dev/null; }

# One rsync of the checkout: the whole of a watch-loop iteration.
push_checkout() {
  # Explicit excludes, unlike .gitignore rules, also protect what exists only
  # there: Linux node_modules, the vendor link, the dist stub.
  rsync -az --delete --exclude .git --exclude .claude --exclude dist \
    --exclude node_modules --exclude .vendor --exclude target \
    --filter=':- .gitignore' -e "ssh ${ssh_opts[*]}" "$root/" "boat:src/$checkout/"
}

watch() { while sleep 1; do push_checkout; done; }

# Everything a command there needs: the checkout, then scripts/remote/prepare.sh.
prepare() {
  # Private-path tests create directories inside the checkout and refuse a
  # group-writable ancestor.
  run "umask 022 && mkdir -p src/$checkout target && chmod go-w src src/$checkout target"
  push_checkout
  run "cd src/$checkout && bash scripts/remote/prepare.sh"
}

remote_env() {
  # The sandbox defaults to umask 002 and a low descriptor limit; the
  # private-path and runtime-capacity tests correctly fail under both.
  # nessa-app's build.rs needs a dist/ whose stage matches the host stage.
  printf '%s' "umask 022 && ulimit -n \$(ulimit -Hn) && cd src/$checkout \
    && export PATH=\$HOME/.cargo/bin:\$PATH CARGO_TARGET_DIR=\$HOME/target NESSA_STAGE=${NESSA_STAGE:-dev} \
    && mkdir -p dist && { [ -f dist/index.html ] || echo '<html></html>' > dist/index.html; } \
    && { [ -f dist/nessa-stage.json ] || echo \"{\\\"stage\\\":\\\"\$NESSA_STAGE\\\"}\" > dist/nessa-stage.json; }"
}

# A command that keeps the builder busy holds a lease, a file named by its pid,
# for as long as it runs. When the last lease is released the builder stops,
# so nothing bills between sessions; a stopped builder resumes with its disk,
# and Cargo's cache, intact. NESSA_BOAT_KEEP=1 leaves it running.
leases="$state_dir/boat-leases"

take_lease() { mkdir -p "$leases" && : > "$leases/$$"; }

release_lease() {
  local lease
  rm -f "$leases/$$"
  for lease in "$leases"/*; do
    [[ -e "$lease" ]] || continue
    kill -0 "$(basename "$lease")" 2>/dev/null && return
    rm -f "$lease" # its command ended without releasing it
  done
  [[ -n "${NESSA_BOAT_KEEP:-}" ]] && return
  log "last session ended; stopping builder $1"
  ssh "${ssh_opts[@]}" -O exit boat 2>/dev/null || true
  boat stop "$1" >/dev/null || log "could not stop $1; run: bash scripts/remote/boat.sh stop"
}

command="${1:-}"
shift || true
case "$command" in
  stop | forget)
    id="$(builder)"
    ssh "${ssh_opts[@]}" -O exit boat 2>/dev/null || true
    if [[ "$command" == stop ]]; then
      boat stop "$id" >/dev/null && log "stopped $id"
    else
      boat delete "$id" && rm -f "$state_file"
    fi
    exit ;;
  exec | sync | shell | proxy | put | get) ;;
  *) sed -n '2,19p' "$0" | sed 's/^# \{0,1\}//' >&2; exit 2 ;;
esac

leased=""
case "$command" in
  exec | shell) leased=1 ;;
  sync) [[ "${1:-}" == --watch ]] && leased=1 ;;
esac
# Taken before the builder is resumed, so a session ending meanwhile sees it.
[[ -n "$leased" ]] && take_lease

id=""
syncer=""
ports=()
finish() {
  [[ -n "$syncer" ]] && kill "$syncer" 2>/dev/null
  for port in ${ports[@]+"${ports[@]}"}; do forward cancel "$port" || true; done
  [[ -n "$leased" && -n "$id" ]] && release_lease "$id"
  return 0
}
trap finish EXIT
trap 'exit 130' INT TERM HUP

id="$(ensure_builder)"
boat extend "$id" --hours 2 >/dev/null
connect "$id"

case "$command" in
  exec)
    while [[ "${1:-}" == --port ]]; do ports+=("$2"); shift 2; done
    prepare
    for port in ${ports[@]+"${ports[@]}"}; do
      if forward forward "$port"; then
        log "127.0.0.1:$port here is 127.0.0.1:$port there"
      else
        log "127.0.0.1:$port is busy here; not forwarded"
      fi
    done
    # Sync on save for as long as the command runs.
    watch &
    syncer=$!
    run -tt "$(remote_env) && bash scripts/remote/run.sh $(printf '%q ' "$@")" ;;
  sync)
    prepare
    [[ "${1:-}" == --watch ]] || exit 0
    log "watching $root (Ctrl-C to stop)"
    watch ;;
  shell)
    prepare
    run -tt "$(remote_env) && exec bash -l" ;;
  put)
    # Private by default: credentials go through here, and the gateway refuses a
    # data directory anyone else can write to.
    run "umask 077 && mkdir -p \"\$(dirname '$2')\""
    rsync -az -e "ssh ${ssh_opts[*]}" "$1" "boat:$2" ;;
  get)
    mkdir -p "$(dirname "$2")"
    rsync -az -e "ssh ${ssh_opts[*]}" "boat:$1" "$2" ;;
  proxy)
    run 'pgrep -x microsocks >/dev/null || (setsid nohup microsocks -i 127.0.0.1 -p 1080 >/dev/null 2>&1 &)'
    forward forward 1080
    log "SOCKS5 on 127.0.0.1:1080. Browse any sandbox port with:"
    echo "open -na 'Google Chrome' --args --user-data-dir=\"\$HOME/.boat-chrome\" --proxy-server=socks5://127.0.0.1:1080 --proxy-bypass-list='<-loopback>'" ;;
esac
