#!/usr/bin/env node
/**
 * Give a dev gateway an agent, from the checkout that knows where its pieces are.
 *
 * `settings.agents` in the namespace's `config.json` is the only thing that makes
 * `nessa server` able to run a conversation. A packaged install gets one from
 * its bundle (`crates/nessa-server/src/composition/desktop.rs`), which a
 * checkout does not have — so a developer could connect and authenticate, then
 * be told "the gateway has no agent configured" on the first message.
 *
 * The server must not learn what a git checkout is: `crates/nessa-sdk/...` is
 * not a path a gateway may go looking for. So this runs on the outside, beside
 * `nessa server --provision-local`, and writes the same kind of file a person
 * would write by hand. It complements provisioning rather than duplicating it:
 * that command creates credentials, this one names the agent.
 *
 *   node scripts/dev-agent-config.mjs
 *
 * Guarantees, in the order they matter:
 *
 * - **An existing `agents` block is never touched.** Not merged, not repaired,
 *   not reordered. Someone who wrote one by hand owns it; all this does then is
 *   check that the files it names are still there and say so if they are not.
 * - **Never leaves a broken file.** `config.json` beside `auth/` fails the
 *   gateway at startup when it is malformed, so the merged document is parsed
 *   back before anything is renamed into place, and an existing file that does
 *   not parse is left exactly as it is.
 * - **Idempotent.** A second run finds the `agents` block and writes nothing.
 * - **Degrades honestly.** Anything missing is reported with the command that
 *   fixes it, and the exit status stays 0 — a gateway with no agent is still a
 *   gateway worth starting, and blocking the dev loop would help nobody.
 */
import { execFileSync, spawnSync } from "node:child_process"
import {
  chmodSync,
  closeSync,
  constants,
  existsSync,
  fstatSync,
  mkdirSync,
  openSync,
  readFileSync,
  realpathSync,
  renameSync,
  statSync,
  unlinkSync,
  writeFileSync,
} from "node:fs"
import { randomUUID } from "node:crypto"
import { homedir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..")

/** The lowest Node major the ACP harnesses are exercised on. The bundle ships 26. */
const MINIMUM_NODE_MAJOR = 20

/** Every agent this checkout can point a dev gateway at.
 *
 * Keyed by the name the gateway knows the agent by, which is the same key the
 * `runtimes` map uses. An agent whose harness is not installed is left out
 * rather than written as a path that is not there — the gateway refuses to
 * start on one of those, and a missing Codex should not cost a working Claude.
 */
export const AGENTS = {
  claude: {
    harness: "crates/nessa-sdk/harnesses/claude-acp",
    entry:
      "crates/nessa-sdk/harnesses/claude-acp/node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js",
    model: "claude-sonnet-5",
  },
  codex: {
    harness: "crates/nessa-sdk/harnesses/codex-acp",
    entry:
      "crates/nessa-sdk/harnesses/codex-acp/node_modules/@agentclientprotocol/codex-acp/dist/index.js",
    model: "gpt-5.6-terra",
  },
}

/** The agent a dev gateway runs on when the developer names none.
 *
 * Claude when it is installed, because that is what a checkout has always
 * started on and what every conversation already on disk belongs to. */
const PREFERRED = "claude"

/** The checked-in model catalog, named once: the block points at it and the
 * check below looks for it, and a move that updated only one of those would
 * write a config naming a file that is not there. */
const CATALOG = "crates/nessa-sdk/data/models.json"

/** The command that installs one agent's harness. */
export const installHarness = (name) =>
  `(cd ${AGENTS[name].harness} && npm ci --omit=dev)`

function say(message) {
  process.stdout.write(`${message}\n`)
}

/** A configuration the gateway will refuse to start on. */
class UnusableConfiguration extends Error {}

/**
 * Report a configuration the gateway will refuse, and fail.
 *
 * Distinct from [`skip`], which is for a machine that has no agent: there the
 * gateway starts and says so when somebody sends a message. Here it does not
 * start at all, so a zero exit would be this script reporting success for a
 * file it knows is broken.
 *
 * Throws rather than exiting, which `skip` can afford to do and this cannot.
 * `publish` is exported and the test suite calls it in its own process, so an
 * exit here would end the whole `node --test` run at the first test that
 * reached it — and a regression in the very thing this guard watches would
 * read as a suite that stopped early rather than one that failed. `main`
 * turns the throw into the nonzero exit a dev loop needs.
 */
function stop(reason, remedy) {
  say(`→ dev agent configuration is unusable: ${reason}`)
  const lines = Array.isArray(remedy) ? remedy : [remedy]
  for (const line of lines.filter(Boolean)) say(`  ${line}`)
  throw new UnusableConfiguration(reason)
}

/** A machine with no agent to configure. The gateway still starts. */
class StoodDown extends Error {}

/**
 * Report why there is no agent, and leave the gateway to start without one.
 *
 * Throws, for the same reason [`stop`] does and not only on the paths a test
 * happens to reach today: `publish` is exported and called in other processes,
 * and a stand-down that exited would end the caller's run at whichever line
 * reached it — silently, and with the success status a stand-down carries.
 * `main` turns this into that status. Throwing also lets the configuration
 * lock's `finally` run. The lock is an `flock` on `config.json.lock`, released
 * by closing that descriptor; an exit from under it would keep the flock for
 * as long as this process stayed up. The lock file itself is left in place.
 */
function skip(reason, remedy) {
  say(`→ dev agent not configured: ${reason}`)
  const lines = Array.isArray(remedy) ? remedy : [remedy]
  for (const line of lines.filter(Boolean)) say(`  ${line}`)
  say("  the gateway will start, but sending a message will report no agent")
  throw new StoodDown(reason)
}

/**
 * The stage/instance namespace, resolved the way the server resolves it in
 * `crates/nessa-server/src/env/paths.rs` — that file is the contract; this
 * mirrors it because a `.mjs` script cannot call into the Rust crate.
 *
 * @returns {string} absolute namespace root, the directory holding `auth/`
 */
export function namespaceRoot(env = process.env, home = homedir()) {
  const stage = env.NESSA_STAGE ?? "dev"
  const instance = env.NESSA_INSTANCE
  // An empty NESSA_INSTANCE is set, not absent: it reaches this check and is
  // refused as a segment, rather than quietly serving the stage's shared
  // namespace. That is this loop's doing, which is why it is said here.
  for (const segment of [stage, ...(instance === undefined ? [] : [instance])]) {
    if (!/^[A-Za-z0-9_-]+$/.test(segment))
      throw new Error(
        `stage and NESSA_INSTANCE must be nonempty alphanumeric, hyphen or underscore segments: ${segment}`,
      )
  }
  const dataDir = env.NESSA_DATA_DIR
  let base
  if (dataDir) {
    if (!dataDir.startsWith("/"))
      throw new Error("NESSA_DATA_DIR must be an absolute path")
    base = dataDir
  } else {
    if (!home || !home.startsWith("/")) throw new Error("HOME must be an absolute path")
    base = join(home, ".nessa")
  }
  const staged = stage === "prod" ? base : join(base, stage)
  return instance === undefined ? staged : join(staged, "instances", instance)
}

/**
 * Which agents this checkout can actually launch, by name.
 *
 * An agent is here only when its harness is on disk. A name written with a path
 * that is not there fails the whole gateway at startup, not just that agent, so
 * a Codex nobody installed would cost a developer their working Claude.
 *
 * @returns {string[]} installed agent names, in a stable order
 */
export function installedAgents(checkout, exists = existsSync) {
  return Object.keys(AGENTS).filter((name) => exists(join(checkout, AGENTS[name].entry)))
}

/**
 * The agents block this checkout would write.
 *
 * Every path is absolute and checked here rather than left to fail inside
 * provider construction, where the message is about a command and its arguments
 * and not about the thing a developer forgot to install.
 *
 * `selected` is stated whenever more than one agent is configured, because the
 * gateway refuses to guess between them — and a developer who never chose would
 * otherwise be told to answer a question they did not know they were asked.
 *
 * @returns {object} the `agents` value, ready to merge
 */
export function agentsBlock({ checkout, namespace, node, mcpBinary, agents }) {
  const workspace = join(namespace, "workspaces/default")
  const runtimes = {}
  for (const name of agents) {
    runtimes[name] = {
      command: node,
      args: [join(checkout, AGENTS[name].entry)],
      model: AGENTS[name].model,
      toolsEnabled: true,
      contextTokens: 100000,
      outputTokens: 4096,
    }
  }
  return {
    catalog: join(checkout, CATALOG),
    workspace,
    selected: agents.includes(PREFERRED) ? PREFERRED : agents[0],
    runtimes,
    ...(mcpBinary
      ? {
          mcpServers: [
            {
              name: "nessa",
              command: mcpBinary,
              args: [
                "--workspace",
                workspace,
                "--audit-directory",
                join(namespace, "process-audit"),
              ],
            },
          ],
        }
      : {}),
  }
}

/**
 * Which node to record.
 *
 * `process.execPath` — the interpreter running this script. It is the only node
 * on this machine we have actually executed, so it is the only one whose
 * existence and version we can state rather than assume. A name off `PATH`
 * would read as more stable and is not: `which node` under a version manager is
 * a per-shell directory that disappears when the shell does.
 *
 * It can still go stale when someone upgrades or uninstalls that version. That
 * is why a later run re-checks an already-written block and says so, instead of
 * letting the gateway fail at startup with a message about absolute files.
 */
function chooseNode() {
  const path = process.execPath
  const major = Number.parseInt(process.versions.node.split(".")[0], 10)
  if (!Number.isInteger(major) || major < MINIMUM_NODE_MAJOR)
    skip(
      `this script is running on Node ${process.versions.node}; the ACP harnesses need ${MINIMUM_NODE_MAJOR} or newer`,
      "install a supported Node and run the dev loop again",
    )
  if (!existsSync(path)) skip(`the running Node (${path}) is not a readable file`, "")
  return path
}

/** The built `nessa-mcp`, or undefined. A configured-but-absent MCP server is worse than none. */
function findMcpBinary() {
  let targetDirectory = join(root, "target")
  try {
    targetDirectory = JSON.parse(
      execFileSync("cargo", ["metadata", "--no-deps", "--format-version", "1"], {
        cwd: root,
        encoding: "utf8",
      }),
    ).target_directory
  } catch {
    // No cargo on PATH is not this script's problem to report; the debug tree
    // under the checkout is the only place a dev build could be anyway.
  }
  for (const profile of ["debug", "release"]) {
    const candidate = join(targetDirectory, profile, "nessa-mcp")
    if (existsSync(candidate) && statSync(candidate).isFile()) return candidate
  }
  return undefined
}

/** Parse an existing config, or report that it is not ours to repair. */
function readExisting(path) {
  if (!existsSync(path)) return {}
  let text
  try {
    text = readFileSync(path, "utf8")
  } catch (error) {
    skip(
      `${path} could not be read (${error.message})`,
      "fix its permissions, then run again",
    )
  }
  let parsed
  try {
    parsed = JSON.parse(text)
  } catch (error) {
    skip(
      `${path} is not valid JSON (${error.message})`,
      "it was left untouched; repair it by hand — the gateway also refuses to start on it",
    )
  }
  if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed))
    skip(`${path} is not a JSON object`, "it was left untouched; repair it by hand")
  return parsed
}

/**
 * Whether a configuration already answers the agent question.
 *
 * The server reads `agents` as an `Option<AgentsConfig>`, so an explicit `null`
 * is a valid configuration that means "no agents" — the gateway starts and says
 * so when a message is sent. Absent is the only state this script fills in;
 * `null` is somebody's answer and is left alone, the same as a block they
 * wrote themselves.
 */
export function agentIsSettled(existing) {
  return existing.agents !== undefined
}

/**
 * Report on a configuration this script is not going to write into.
 *
 * Both readers of an already-settled file come through here — the early answer
 * and the one taken under the lock — because they have the same thing to say
 * and only one of them used to say all of it.
 */
function checkExisting(existing, path, { held = false } = {}) {
  // First, because nothing else about the file matters if the gateway will not
  // read it. The `agents` answer beside it is somebody else's and stands, but
  // this key is this script's own and the gateway refuses to start while it is
  // there, so it is taken back out rather than reported as a chore.
  //
  // Only with the lock held. The early reader has not taken it, so it says what
  // it found and returns `false`, and the caller goes and takes the lock — the
  // same right every other write to this file is made under.
  if (existing.agent !== undefined) {
    if (!held) {
      say(`→ ${path} still has the retired "agent" block beside "agents"`)
      say("  taking the lock to remove it")
      return false
    }
    retireAgentKey(existing, path)
  }
  const agents = existing.agents
  if (agents === null) {
    say(`→ ${path} sets "agents": null, which is a gateway with no agents`)
    say('  it was left as it is; remove the "agents" line and rerun to have one written')
    return true
  }
  // Every absolute path the configuration would hand this machine. A relative
  // argument is the agent's own vocabulary — a subcommand or a flag — and is
  // nothing for this script to go looking for, the same rule the gateway uses.
  const runtimes = agents.runtimes ?? {}
  const missing = [agents.catalog]
    .concat(
      Object.values(runtimes).flatMap((runtime) => [
        runtime?.command,
        ...(Array.isArray(runtime?.args) ? runtime.args : []),
      ]),
    )
    .filter(
      (value) => typeof value === "string" && value.startsWith("/") && !existsSync(value),
    )
  if (missing.length === 0) {
    const named = Object.keys(runtimes)
    say(
      `→ dev agents already configured in ${path} (${named.join(", ") || "none named"}); left as it is`,
    )
    return true
  }
  say(`→ dev agents in ${path} point at files that are not there:`)
  for (const value of missing) say(`    ${value}`)
  say('  it was left untouched. Reinstall what moved, or remove the "agents" block')
  say("  and run the dev loop again to have one written, after installing a harness:")
  for (const name of Object.keys(AGENTS)) say(`    ${installHarness(name)}`)
  return true
}

/**
 * Take the lock solely to report on, and repair, a file already settled.
 *
 * The ordinary path reaches the lock through `publish`, which needs a generated
 * block to write. Here there is nothing to generate — the question is answered
 * — and the one thing that still has to happen is a write, so the lock is taken
 * for that alone rather than by computing a block nobody will use.
 */
function checkUnderLock(configPath) {
  const lock = lockFor(configPath)
  if ("reason" in lock) {
    say(`→ not repairing ${configPath}: ${lock.reason}`)
    return
  }
  try {
    checkExisting(readExisting(configPath), configPath, { held: true })
  } finally {
    lock.release()
  }
}

function main() {
  if (process.platform === "win32")
    skip(
      "ACP agents need Unix process supervision, so a Windows checkout has none to configure",
      "",
    )
  let namespace
  try {
    namespace = namespaceRoot()
  } catch (error) {
    skip(error.message, "")
  }
  const configPath = join(namespace, "config.json")
  const existing = readExisting(configPath)
  // An early answer, so the work below is skipped entirely. `publish` asks
  // again at the end, because this one goes stale while that work happens.
  if (agentIsSettled(existing)) {
    if (!checkExisting(existing, configPath)) checkUnderLock(configPath)
    return
  }

  const checkout = realpathSync(root)
  const agents = installedAgents(checkout)
  if (agents.length === 0)
    skip("no ACP harness is installed in this checkout", [
      ...Object.keys(AGENTS).map((name) => `run: ${installHarness(name)}`),
      "then start the dev loop again",
    ])
  const catalog = join(checkout, CATALOG)
  if (!existsSync(catalog))
    skip(
      `the model catalog is missing: ${catalog}`,
      "this file is checked in; restore it",
    )

  const node = chooseNode()
  const mcpBinary = findMcpBinary()

  // The namespace and the agent's workspace must exist, with the private
  // permissions the gateway insists on for everything under this root.
  mkdirSync(join(namespace, "workspaces/default"), { recursive: true, mode: 0o700 })
  const block = agentsBlock({ checkout, namespace, node, mcpBinary, agents })

  publish({ configPath, agents: block, node, mcpBinary })
}

/**
 * Writes the agents into whatever the file says at this moment.
 *
 * The configuration is read again here rather than reused from the start of
 * the run. Everything between the two reads takes real time — locating a node,
 * asking cargo about the MCP binary, making the workspace — and another writer
 * in that window is an ordinary thing: a second checkout's dev loop, an editor
 * saving settings, the gateway itself. Renaming a file built on the older read
 * over theirs is atomic and still loses what they wrote.
 *
 * `interrupt` exists for the test that proves it: it runs between the read and
 * the write, which is the window a concurrent writer lives in.
 */
/**
 * Hold the right to write this configuration, or stand down without writing.
 *
 * Re-reading before the rename narrows the window between deciding and writing;
 * it does not close it. Two runs can both read, both decide to write, and the
 * second rename silently replaces the first — atomic, and still a lost update.
 * So the read, the decision and the rename happen while holding this.
 *
 * The same protocol as the gateway's `OsConfigFiles::try_lock`
 * (`docs/design/mcp-connections.md`): open `<config>.lock` (create, 0600, no
 * follow, no block), refuse anything that is not a regular file, and take an
 * exclusive non-blocking `flock` on that descriptor. Node has no `flock`, so
 * Perl calls it on the descriptor this function already opened. The file stays.
 * Deleting it would put the next writer on a new inode, which does not exclude
 * a holder of the old one. A file left behind — empty, or still carrying a pid
 * line from the older exclusive-create lock — is not a holder.
 *
 * A helper that cannot run is a stand-down, not a write without the lock.
 * Closing the descriptor releases the flock. An editor saving `config.json`
 * still takes no lock: the race left is between the read inside the lock and
 * the rename.
 *
 * `reason` is the whole clause a caller prints. Only a busy flock says the
 * lock is held; an open or helper failure names that failure.
 *
 * @returns {{ release: () => void } | { reason: string }}
 */
function lockFor(configPath) {
  const lock = `${configPath}.lock`
  let fd
  try {
    fd = openSync(
      lock,
      constants.O_CREAT |
        constants.O_WRONLY |
        constants.O_NONBLOCK |
        constants.O_NOFOLLOW,
      0o600,
    )
  } catch (error) {
    return { reason: `could not lock ${lock}: ${error.message}` }
  }
  try {
    if (!fstatSync(fd).isFile()) {
      closeQuiet(fd)
      return { reason: `${lock} must be a regular file` }
    }
  } catch (error) {
    closeQuiet(fd)
    return { reason: `could not lock ${lock}: ${error.message}` }
  }
  // The helper's descriptor must be a different number from ours. dup2 of
  // an fd onto itself does not clear close-on-exec, so exec would close it
  // and the helper would lock nothing.
  const childFd = fd === 3 ? 4 : 3
  const stdio = ["ignore", "ignore", "pipe"]
  while (stdio.length <= childFd) stdio.push("ignore")
  stdio[childFd] = fd
  const taken = spawnSync("perl", ["-e", flockProgram(childFd)], {
    stdio,
    encoding: "utf8",
    timeout: 5_000,
  })
  if (taken.status === 1) {
    closeQuiet(fd)
    return {
      reason: "its lock is held by another writer (the gateway, or another dev loop)",
    }
  }
  if (taken.error || taken.status !== 0) {
    closeQuiet(fd)
    const detail =
      taken.error?.message ?? (taken.stderr?.trim() || `perl exited ${taken.status}`)
    return { reason: `could not lock ${lock}: ${detail}` }
  }
  return { release: () => closeQuiet(fd) }
}

/** `flock(2)` on the inherited descriptor: 0 held, 1 busy, anything else a failure. */
function flockProgram(childFd) {
  return `
use Fcntl qw(:flock);
open(my $fh, ">&=${childFd}") or die "fdopen: $!";
flock($fh, LOCK_EX|LOCK_NB) or do {
  if ($!{EWOULDBLOCK} || $!{EAGAIN}) { exit 1 }
  die "flock: $!";
};
exit 0;
`
}

function closeQuiet(fd) {
  try {
    closeSync(fd)
  } catch {
    // Already closed.
  }
}

/**
 * Replace the configuration's bytes, never leaving a half-written one in place.
 *
 * Throws rather than reporting, because the two callers owe different answers:
 * a dev loop that could not write the block it generated stands down, and one
 * that could not remove a key the gateway refuses to start on has failed.
 */
function writeConfig(configPath, text) {
  // Random, not the pid. A run killed between the write and the rename leaves
  // the temp file behind, and a later run that happens to get the same pid then
  // fails `wx` with EEXIST — reported as "check the namespace's permissions",
  // which names the wrong cause entirely.
  const temporary = `${configPath}.${randomUUID()}.tmp`
  try {
    writeFileSync(temporary, text, { mode: 0o600, flag: "wx" })
    chmodSync(temporary, 0o600)
    renameSync(temporary, configPath)
  } catch (error) {
    try {
      unlinkSync(temporary)
    } catch {
      // Nothing to clean up.
    }
    throw error
  }
}

/**
 * Take the retired `agent` key back out of a file this script is not otherwise
 * writing into.
 *
 * This script wrote that key, nothing reads it now, and the server refuses to
 * start on a file that still carries it — so bringing it to the current shape
 * is what the standard asks of the tool that wrote it, on this path as much as
 * on the one where the block is generated. Only that key is touched; the
 * `agents` answer beside it is somebody else's and is written back exactly as
 * it was read.
 *
 * Called only with the configuration lock held, because it is a write.
 */
function retireAgentKey(existing, path) {
  const { agent: _retired, ...rest } = existing
  try {
    writeConfig(path, `${JSON.stringify(rest, null, 2)}\n`)
  } catch (error) {
    stop(`${path} still has the retired "agent" block and could not be repaired`, [
      error.message,
      "the gateway refuses to start on it: `agent` is an unknown field now",
      'delete the "agent" key by hand and run the dev loop again',
    ])
  }
  say(`→ ${path} carried the retired "agent" block beside "agents"`)
  say("    retired    the block an earlier version of this script wrote")
  say("               the gateway refuses to start while it is there")
}

export function publish({ configPath, agents, node, mcpBinary, interrupt }) {
  const lock = lockFor(configPath)
  if ("reason" in lock) {
    say(`→ not configuring ${configPath}: ${lock.reason}`)
    return false
  }
  try {
    return underLock({ configPath, agents, node, mcpBinary, interrupt })
  } finally {
    lock.release()
  }
}

/** The read, the decision and the write, with the right to do them held. */
function underLock({ configPath, agents, node, mcpBinary, interrupt }) {
  interrupt?.()
  const existing = readExisting(configPath)
  // Somebody answered the question while this was working. Theirs stands —
  // the same courtesy an agent block already in the file gets.
  if (agentIsSettled(existing)) {
    checkExisting(existing, configPath, { held: true })
    return false
  }

  // The previous version of this script wrote an `agent` block, and the server
  // reads its configuration with `deny_unknown_fields`. Left beside the `agents`
  // written here, that old key is a gateway that refuses to start, reported as
  // an unknown field rather than as anything to do with the dev loop — and
  // produced by a run that has just said it succeeded.
  //
  // So it is retired here. This script owned that block, it is local
  // development data, and bringing it to the current shape is what the standard
  // asks of the tool that wrote it. Nothing reads it any more.
  const { agent: retired, ...rest } = existing
  const merged = { ...rest, agents }
  const text = `${JSON.stringify(merged, null, 2)}\n`
  // Parse it back before it can become the file the gateway reads. A config.json
  // that does not parse is not a missing agent, it is a server that will not start.
  const round = JSON.parse(text)
  // Not a stand-down: this script generated a document the gateway will refuse
  // to start on, which is a bug here and not a machine without an agent.
  if (round.agent !== undefined)
    stop(
      "the generated configuration still holds the retired agent block",
      "please report this",
    )
  const survived = Object.entries(agents.runtimes).every(
    ([name, runtime]) =>
      round.agents.runtimes[name]?.command === node &&
      round.agents.runtimes[name]?.args?.[0] === runtime.args[0],
  )
  if (!survived || round.agents.selected !== agents.selected)
    stop("the generated configuration did not survive a round trip", "please report this")

  try {
    writeConfig(configPath, text)
  } catch (error) {
    skip(
      `could not write ${configPath} (${error.message})`,
      "check the namespace's permissions",
    )
  }

  say(`→ dev agents configured in ${configPath}`)
  if (retired !== undefined)
    say('    retired    the "agent" block an earlier version of this script wrote')
  say(`    node       ${node}`)
  for (const [name, runtime] of Object.entries(agents.runtimes))
    say(`    ${name.padEnd(10)} ${runtime.args[0]}`)
  say(`    selected   ${agents.selected}`)
  say(`    workspace  ${agents.workspace}`)
  say(
    mcpBinary
      ? `    nessa MCP  ${mcpBinary}`
      : '    nessa MCP  not built; omitted (cargo build -p nessa-mcp, then delete the "agents" block and rerun)',
  )
  return true
}

// Both sides are resolved before comparison: Node resolves `import.meta.url`
// through symlinks and `/var` → `/private/var`, but leaves `argv[1]` as typed,
// and a script that silently did nothing would be the worst failure here.
const invoked = process.argv[1] ? realpathSync(process.argv[1]) : ""
if (invoked === realpathSync(fileURLToPath(import.meta.url))) {
  try {
    main()
  } catch (error) {
    // The two answers this script gives about itself. Anything else is a bug
    // here and keeps its stack, because a dev loop that hides one is worse
    // than one that prints it.
    if (error instanceof StoodDown) process.exit(0)
    if (!(error instanceof UnusableConfiguration)) throw error
    process.exit(1)
  }
}
