#!/usr/bin/env bash
# Runs on the build sandbox, in a synced checkout, before every remote command
# (#358): what a command needs that the sync deliberately does not carry.
# Each step is skipped while its lockfile is unchanged since it last ran.
set -euo pipefail

# Install with `command` in `directory` unless `lockfile` matches the one the
# last successful install recorded.
once_per_lockfile() {
  local directory="$1" lockfile="$2" command="$3" sum
  sum="$(sha1sum "${directory}/${lockfile}")"
  [[ "$(cat "${directory}/node_modules/.lock-sum" 2>/dev/null)" == "${sum}" ]] && return
  (cd "${directory}" && sh -c "${command}")
  echo "${sum}" > "${directory}/node_modules/.lock-sum"
}

# Nessa UI from its pin, filled by the script that owns it — not this machine's
# .vendor, which may be a link to a checkout in progress.
[[ -L .vendor/nessa_ui ]] && rm .vendor/nessa_ui
node scripts/ensure-nessa-ui.mjs --check >/dev/null 2>&1 || node scripts/ensure-nessa-ui.mjs

# Linux packages; this machine's node_modules never sync.
once_per_lockfile . pnpm-lock.yaml "pnpm install --frozen-lockfile --ignore-scripts --silent"

# The ACP harnesses the dev gateway runs agents through, named and installed the
# way scripts/dev-agent-config.mjs says.
node --input-type=module -e '
  import { AGENTS, installHarness } from "./scripts/dev-agent-config.mjs"
  for (const name of Object.keys(AGENTS)) console.log(`${AGENTS[name].harness}\t${installHarness(name)}`)
' | while IFS=$'\t' read -r directory command; do
  once_per_lockfile "${directory}" package-lock.json "cd \"$PWD\" && ${command}"
done
