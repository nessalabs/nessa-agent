import { chmodSync, mkdirSync, writeFileSync } from "node:fs"
import { join } from "node:path"

/** The host the `ssh` step connects: answered by the stand-in `ssh`. */
export const REACHABLE = "build-box"
/** A machine the configuration names that no `ssh` reaches. */
export const UNREACHABLE = "spare-box"

/**
 * The hub's `~/.ssh/config` for the `ssh` step: two Linux machines it
 * suggests, and what it leaves out: a git forge by its name, one by its
 * `HostName`, one signed in as `git`, a pattern, and a `Match` block.
 */
export const SSH_CONFIG = `# Written by the stress check's ssh step.
Host github.com
  User git

Host work-gitlab
  HostName gitlab.example.com

Host forge
  HostName code.example.com
  User git

Host ${REACHABLE}
  HostName 10.0.0.7
  User builder
  Port 2222

Host ${UNREACHABLE}
  HostName 10.0.0.8

Host *.internal
  User ops

Match host *
  ServerAliveInterval 30
`

/**
 * Writes `SSH_CONFIG` as the gateway's own: `home` is the `HOME` it runs
 * under (`signedOutEnvironment`).
 */
export function writeSshConfig(home) {
  mkdirSync(join(home, ".ssh"), { recursive: true, mode: 0o700 })
  writeFileSync(join(home, ".ssh", "config"), SSH_CONFIG, { mode: 0o600 })
}

/**
 * A stand-in `ssh`, written into `directory` (put first on the gateway's
 * PATH): `REACHABLE` is this machine, with a home of its own under
 * `directory/remote`, where each command runs as `sh -c` would on a real
 * host and the sftp subsystem is this machine's `sftp-server` when it has
 * one; any other host is unreachable, as `ssh` exits 255. Options are
 * skipped up to the `--` the gateway always gives before the host.
 */
export function writeFakeSsh(directory) {
  const remote = join(directory, "remote", REACHABLE)
  mkdirSync(remote, { recursive: true })
  const bin = join(directory, "bin")
  mkdirSync(bin, { recursive: true })
  const program = join(bin, "ssh")
  writeFileSync(
    program,
    `#!/bin/sh
while [ $# -gt 0 ]; do
  case "$1" in
    --) shift; break ;;
    -[oSpliF]) shift 2 ;;
    *) shift ;;
  esac
done
host=$1
shift
if [ "$host" != ${JSON.stringify(REACHABLE)} ]; then
  echo "ssh: Could not resolve hostname $host" >&2
  exit 255
fi
cd ${JSON.stringify(remote)} || exit 255
HOME=${JSON.stringify(remote)}
export HOME
if [ "$*" = sftp ]; then
  for server in /usr/libexec/sftp-server /usr/lib/openssh/sftp-server /usr/lib/ssh/sftp-server; do
    [ -x "$server" ] && exec "$server"
  done
  exit 255
fi
exec sh -c "$*"
`,
  )
  chmodSync(program, 0o755)
  return { bin, remote }
}
