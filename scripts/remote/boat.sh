#!/usr/bin/env bash
# Run this checkout's commands on a Linux Boat sandbox while the files and the
# agents editing them stay on this machine. Cargo's build output, Linux
# node_modules, and the dev servers live there; only macOS app builds run here.
# `just remote <recipe>` is the usual entry; this script is its plumbing (#358).
#
#   scripts/remote/boat.sh exec [--port N | --try-port N]... <cmd...>
#                                          # run there; --port must forward, --try-port may not
#   scripts/remote/boat.sh sync [--watch]  # push this checkout
#   scripts/remote/boat.sh shell           # interactive shell in the remote copy
#   scripts/remote/boat.sh put <here> <there>  # copy to the sandbox (there: relative to its home)
#   scripts/remote/boat.sh get <there> <here>  # copy from the sandbox
#   scripts/remote/boat.sh proxy           # SOCKS on 127.0.0.1:1080 to reach any sandbox port
#   scripts/remote/boat.sh stop            # snapshot and stop the builder (keeps its cache)
#   scripts/remote/boat.sh forget [--yes]  # delete the builder and its cache
#
# Each checkout has a builder of its own, so worktrees neither queue on one
# Cargo lock nor collide on one port. The main checkout's is created fresh; a
# worktree's is forked from the main one's latest snapshot, so it starts with
# a warm Cargo cache and whatever logins were made there. Stopping snapshots the
# disk, so the next run resumes warm.
set -euo pipefail

state_dir="${XDG_CONFIG_HOME:-$HOME/.config}/nessa"
root="$(git rev-parse --show-toplevel)"
main_root="$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")"
# Every builder holds its copy at the same path, so a fork's Cargo fingerprints
# and the dev agent config's absolute paths stay valid.
remote_checkout="src/nessa-agent"

log() { echo "boat: $*" >&2; }

# Where a checkout's builder id is recorded, keyed by the checkout's path.
builder_file() { echo "$state_dir/boat-builders/$(printf '%s' "$1" | shasum | cut -c1-16)"; }
state_file="$(builder_file "$root")"

builder() { [[ -f "$1" ]] && cat "$1"; }

# `field` of the sandbox in `boat info --json`, or of the first `"event":"ready"`
# line of `boat new` / `boat fork`.
sandbox_field() {
  python3 -c '
import json, sys
field = sys.argv[1]
for line in sys.stdin:
    try:
        value = json.loads(line)
    except ValueError:
        continue
    if "sandbox" in value:
        print(value["sandbox"].get(field) or ""); break
    if value.get("event") == "ready":
        print(value.get(field) or ""); break
' "$1"
}

state_of() { boat info "$1" --json 2>/dev/null | sandbox_field state; }

# The builder's states, and what each asks of a command that needs it:
#   ready, idle, running ──────────────► use it
#   stopping ──── wait for stopped ────► resume ──► use it
#   stopped ──────────────────────────► resume ──► use it
#   anything else, or Boat not answering ► refuse, naming the state; a builder
#     is a paid machine, so one is never replaced on a guess
#   none recorded for this checkout ──► fork the main checkout's, or create one
ensure_builder() {
  local id state base
  if boat status 2>/dev/null | grep -q '"loginState":"signed out"'; then
    log "Boat is signed out; run \`boat login\` with the method this account uses"
    exit 1
  fi
  if id="$(builder "$state_file")"; then
    state="$(state_of "$id")"
    for _ in $(seq 1 60); do
      [[ "$state" == stopping ]] || break
      sleep 2
      state="$(state_of "$id")"
    done
    case "$state" in
      ready | idle | running) ;;
      stopped)
        log "resuming builder $id"
        boat resume "$id" >/dev/null ;;
      *)
        log "builder $id is '${state:-unreachable}'; try again, or \`bash scripts/remote/boat.sh forget\` if it was deleted"
        exit 1 ;;
    esac
    echo "$id"
    return
  fi
  mkdir -p "$(dirname "$state_file")"
  base="$(builder "$(builder_file "$main_root")" || true)"
  # A fork copies the source's latest snapshot. Only the one taken when it
  # stopped is whole: Boat also snapshots a running sandbox in the background,
  # and a fork of that has caught Cargo mid-write and could not build.
  local base_state=""
  if [[ "$root" != "$main_root" && -n "$base" ]]; then
    base_state="$(state_of "$base")"
    for _ in $(seq 1 60); do
      [[ "$base_state" == stopping ]] || break
      sleep 2
      base_state="$(state_of "$base")"
    done
  fi
  if [[ "$base_state" == stopped ]]; then
    log "forking this worktree's builder from $base"
    id="$(boat fork "$base" --type large --json | sandbox_field id)" || true
  else
    [[ -n "$base_state" ]] && log "the main checkout's builder is ${base_state}, not stopped; starting this one fresh"
    log "creating builder sandbox"
    # With the account's environment, so whatever it stores (GitHub token,
    # secrets, agent logins) is there for the agents the dev gateway runs.
    id="$(boat new --type large --json | sandbox_field id)" || true
  fi
  [[ -n "$id" ]] || { log "Boat did not create a builder"; exit 1; }
  # Recorded before anything else can fail, so a builder is never left billing
  # with nothing pointing at it. Tools are installed by prepare.sh, every time
  # until it has succeeded once.
  echo "$id" > "$state_file"
  echo "$id"
}

# `boat ssh` costs ~4.5 s per call through the Boat API, too slow to sync on
# save. Open one plain SSH master with Boat's key and pinned host key, and send
# every later command through it. One master per checkout, so worktrees reach
# their own builders.
control="$state_dir/boat-ssh-$(basename "$state_file").sock"
ssh_opts=(-S "$control")

connect() {
  local id="$1" endpoint known
  ssh "${ssh_opts[@]}" -O check boat 2>/dev/null && return
  endpoint="$(boat info "$id" --json | sandbox_field sshEndpoint)"
  [[ -n "$endpoint" ]] || { log "no SSH endpoint for $id"; exit 1; }
  for attempt in 1 2; do
    # Boat pins each sandbox's host key in a file of its own, named by the
    # alias. Which file is this sandbox's is not published, so any pinned Boat
    # key is accepted: a host outside Boat still cannot answer.
    for known in "$HOME"/.ssh/ascii_sandbox_known_hosts/*; do
      ssh -fN -M "${ssh_opts[@]}" -o ControlPersist=2h -o BatchMode=yes \
        -o ServerAliveInterval=15 -o ServerAliveCountMax=3 \
        -p "${endpoint##*:}" -i "$HOME/.ssh/ascii_box_ed25519" \
        -o StrictHostKeyChecking=yes -o UserKnownHostsFile="$known" \
        -o GlobalKnownHostsFile=/dev/null \
        -o HostKeyAlias="ascii-sandbox-$(basename "$known")" \
        "user@${endpoint%:*}" 2>/dev/null && return
    done
    # A host key Boat has not pinned yet: one `boat ssh` records it.
    [[ "$attempt" == 1 ]] && boat ssh "$id" true >/dev/null
  done
  log "could not open SSH to $id"
  exit 1
}

run() { ssh "${ssh_opts[@]}" boat "$@"; }

port_busy() { nc -z 127.0.0.1 "$1" 2>/dev/null; }

forward() { ssh "${ssh_opts[@]}" -O "$1" -L "127.0.0.1:$2:127.0.0.1:$2" boat 2>/dev/null; }

# One rsync of the checkout: the whole of a watch-loop iteration.
push_checkout() {
  # Git decides what is ignored, not rsync's reading of .gitignore. Paths are
  # anchored at the checkout root. The explicit excludes, unlike ignore rules,
  # also protect what exists only there: Linux node_modules, the Nessa UI pin,
  # Cargo's output, and the .tmp* directories tests create inside crates
  # (tempfile::tempdir_in), which the sync would otherwise delete under them.
  rsync -az --delete \
    --exclude-from=<(git -C "$root" ls-files -oi --exclude-standard --directory | sed 's|^|/|') \
    --exclude /.git --exclude /.claude --exclude node_modules --exclude /.vendor \
    --exclude /dist --exclude /target --filter='P .tmp*' \
    -e "ssh ${ssh_opts[*]}" "$root/" "boat:$remote_checkout/"
}

# Sync on save. A failed sync is reported and retried rather than ending the
# loop: a file can vanish under rsync mid-save, and the next second is fine.
watch() {
  set +e
  while sleep 1; do
    push_checkout 2>/dev/null || log "sync failed; retrying"
  done
}

# Boat ends a sandbox at the end of its lifetime, and `extend` sets that
# lifetime rather than adding to it. Renewing while a session runs keeps a long
# session alive; the lease below stops it when the session ends.
keepalive() {
  set +e
  while sleep 1800; do boat extend "$1" --hours 2 >/dev/null || log "could not extend $1"; done
}

# Everything a command there needs: the checkout, then scripts/remote/prepare.sh.
prepare() {
  # Private-path tests create directories inside the checkout and refuse a
  # group-writable ancestor.
  run "umask 022 && mkdir -p $remote_checkout target && chmod go-w src $remote_checkout target"
  push_checkout
  run "cd $remote_checkout && bash scripts/remote/prepare.sh"
}

remote_env() {
  # The sandbox defaults to umask 002 and a low descriptor limit; the
  # private-path and runtime-capacity tests correctly fail under both.
  printf '%s' "umask 022 && ulimit -n \$(ulimit -Hn) && cd $remote_checkout \
    && export PATH=\$HOME/.cargo/bin:\$PATH CARGO_TARGET_DIR=\$HOME/target NESSA_STAGE=${NESSA_STAGE:-dev}"
}

# A command that keeps the builder busy holds a lease, a file named by its pid,
# for as long as it runs. When the last lease is released the builder stops,
# so nothing bills between sessions; a stopped builder resumes with its disk,
# and Cargo's cache, intact. NESSA_BOAT_KEEP=1 leaves it running.
leases="$state_dir/boat-leases/$(basename "$state_file")"

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
    id="$(builder "$state_file")" || { log "this checkout has no builder"; exit 0; }
    ssh "${ssh_opts[@]}" -O exit boat 2>/dev/null || true
    if [[ "$command" == stop ]]; then
      boat stop "$id" >/dev/null && log "stopped $id"
    else
      [[ "${1:-}" == --yes ]] && yes=(--yes) || yes=()
      boat delete "$id" ${yes[@]+"${yes[@]}"} >/dev/null && rm -f "$state_file" && log "deleted $id"
    fi
    exit ;;
  exec | sync | shell | proxy | put | get) ;;
  *) sed -n '2,21p' "$0" | sed 's/^# \{0,1\}//' >&2; exit 2 ;;
esac

leased=""
case "$command" in
  exec | shell) leased=1 ;;
  sync) [[ "${1:-}" == --watch ]] && leased=1 ;;
esac
# Taken before the builder is resumed, so a session ending meanwhile sees it.
[[ -n "$leased" ]] && take_lease

id=""
background=()
forwarded=()
finish() {
  for pid in ${background[@]+"${background[@]}"}; do kill "$pid" 2>/dev/null; done
  # Only what this command forwarded: another command may own the rest.
  for port in ${forwarded[@]+"${forwarded[@]}"}; do forward cancel "$port" || true; done
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
    required=()
    optional=()
    while true; do
      case "${1:-}" in
        --port) required+=("$2"); shift 2 ;;
        --try-port) optional+=("$2"); shift 2 ;;
        *) break ;;
      esac
    done
    for port in ${required[@]+"${required[@]}"}; do
      if port_busy "$port"; then
        log "127.0.0.1:$port is already in use here; free it and try again"
        exit 1
      fi
    done
    prepare
    for port in ${required[@]+"${required[@]}"} ${optional[@]+"${optional[@]}"}; do
      if ! port_busy "$port" && forward forward "$port"; then
        forwarded+=("$port")
        log "127.0.0.1:$port here is 127.0.0.1:$port there"
      else
        log "127.0.0.1:$port is in use here; not forwarded"
      fi
    done
    watch &
    background+=($!)
    keepalive "$id" &
    background+=($!)
    run -tt "$(remote_env) && bash scripts/remote/run.sh $(printf '%q ' "$@")" ;;
  sync)
    prepare
    [[ "${1:-}" == --watch ]] || exit 0
    keepalive "$id" &
    background+=($!)
    log "watching $root (Ctrl-C to stop)"
    watch ;;
  shell)
    prepare
    keepalive "$id" &
    background+=($!)
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
