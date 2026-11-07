import { chmodSync, mkdirSync, writeFileSync } from "node:fs"
import { join } from "node:path"

/** The host the `ssh` step connects: answered by the stand-in `ssh`. */
const REACHABLE = "build-box"
/** A machine the configuration names that no `ssh` reaches. */
const UNREACHABLE = "spare-box"

let rounds = 0

/**
 * The two machines one run of the `ssh` step suggests. Connecting saves the
 * reachable one in config.json, and the gateway suggests no machine it
 * already has, so each later run names machines of its own: `build-box`,
 * then `build-box-2`, and so on.
 */
export function nextSshHosts() {
  rounds += 1
  const suffix = rounds === 1 ? "" : `-${rounds}`
  return { reachable: REACHABLE + suffix, unreachable: UNREACHABLE + suffix }
}

/**
 * The hub's `~/.ssh/config` for the `ssh` step: two Linux machines it
 * suggests, and what it leaves out: a git forge by its name, one by its
 * `HostName`, one signed in as `git`, a pattern, and a `Match` block.
 */
const sshConfig = ({
  reachable,
  unreachable,
}) => `# Written by the stress check's ssh step.
Host github.com
  User git

Host work-gitlab
  HostName gitlab.example.com

Host forge
  HostName code.example.com
  User git

Host ${reachable}
  HostName 10.0.0.7
  User builder
  Port 2222

Host ${unreachable}
  HostName 10.0.0.8

Host *.internal
  User ops

Match host *
  ServerAliveInterval 30
`

/**
 * Writes the configuration naming `hosts` (from `nextSshHosts`) as the
 * gateway's own: `home` is the `HOME` it runs under (`signedOutEnvironment`).
 */
export function writeSshConfig(home, hosts) {
  mkdirSync(join(home, ".ssh"), { recursive: true, mode: 0o700 })
  writeFileSync(join(home, ".ssh", "config"), sshConfig(hosts), { mode: 0o600 })
}

/**
 * A stand-in `ssh`, written into `directory` (put first on the gateway's
 * PATH): every `build-box` machine is this one, each with a home of its
 * own under `directory/remote/<host>` and a Nessa instance of its own (the
 * gateway's configuration, copied once), so two of them serve at once as
 * two machines would. Each command runs as `sh -c` would on a real host and
 * the sftp subsystem is this machine's `sftp-server` when it has one; any
 * other host is unreachable, as `ssh` exits 255. Options are skipped up to
 * the `--` the gateway always gives before the host.
 */
export function writeFakeSsh(directory) {
  const remote = join(directory, "remote")
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
case "$host" in
  ${REACHABLE}|${REACHABLE}-*) ;;
  *)
    echo "ssh: Could not resolve hostname $host" >&2
    exit 255
    ;;
esac
mkdir -p ${JSON.stringify(remote)}/"$host" || exit 255
cd ${JSON.stringify(remote)}/"$host" || exit 255
HOME=${JSON.stringify(remote)}/"$host"
export HOME
if [ -n "$NESSA_DATA_DIR" ] && [ -n "$NESSA_INSTANCE" ]; then
  instances="$NESSA_DATA_DIR/\${NESSA_STAGE:-ci}/instances"
  own="$NESSA_INSTANCE-$host"
  if [ ! -f "$instances/$own/config.json" ] && [ -f "$instances/$NESSA_INSTANCE/config.json" ]; then
    mkdir -p "$instances/$own" &&
      cp "$instances/$NESSA_INSTANCE/config.json" "$instances/$own/config.json.$$" &&
      mv "$instances/$own/config.json.$$" "$instances/$own/config.json"
  fi
  NESSA_INSTANCE=$own
  export NESSA_INSTANCE
fi
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
