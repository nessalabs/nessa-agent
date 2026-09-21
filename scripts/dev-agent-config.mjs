#!/usr/bin/env node
/**
 * Give a dev gateway an agent, from the checkout that knows where its pieces are.
 *
 * `settings.agent` in the namespace's `config.json` is the only thing that makes
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
 * - **An existing `agent` block is never touched.** Not merged, not repaired,
 *   not reordered. Someone who wrote one by hand owns it; all this does then is
 *   check that its two executables are still there and say so if they are not.
 * - **Never leaves a broken file.** `config.json` beside `auth/` fails the
 *   gateway at startup when it is malformed, so the merged document is parsed
 *   back before anything is renamed into place, and an existing file that does
 *   not parse is left exactly as it is.
 * - **Idempotent.** A second run finds the `agent` block and writes nothing.
 * - **Degrades honestly.** Anything missing is reported with the command that
 *   fixes it, and the exit status stays 0 — a gateway with no agent is still a
 *   gateway worth starting, and blocking the dev loop would help nobody.
 */
import { execFileSync } from "node:child_process"
import {
  chmodSync,
  existsSync,
  mkdirSync,
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

/** The lowest Node major the Claude ACP harness is exercised on. The bundle ships 26. */
const MINIMUM_NODE_MAJOR = 20

export const HARNESS = "crates/nessa-sdk/harnesses/claude-acp"
export const ACP_ENTRY = `${HARNESS}/node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js`
/** The checked-in model catalog, named once: the block points at it and the
 * check below looks for it, and a move that updated only one of those would
 * write a config naming a file that is not there. */
const CATALOG = "crates/nessa-sdk/data/models.json"

const INSTALL_HARNESS = `(cd ${HARNESS} && npm ci --omit=dev)`

function say(message) {
  process.stdout.write(`${message}\n`)
}

/** Report why there is no agent, and leave the gateway to start without one. */
function skip(reason, remedy) {
  say(`→ dev agent not configured: ${reason}`)
  const lines = Array.isArray(remedy) ? remedy : [remedy]
  for (const line of lines.filter(Boolean)) say(`  ${line}`)
  say("  the gateway will start, but sending a message will report no agent")
  process.exit(0)
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
 * The agent block this checkout would write.
 *
 * Every path is absolute and checked here rather than left to fail inside
 * provider construction, where the message is about "node and acpEntry" and not
 * about the thing a developer forgot to install.
 *
 * @returns {object} the `agent` value, ready to merge
 */
export function agentBlock({ checkout, namespace, node, mcpBinary }) {
  return {
    catalog: join(checkout, CATALOG),
    node,
    acpEntry: join(checkout, ACP_ENTRY),
    workspace: join(namespace, "workspaces/default"),
    model: "claude-sonnet-5",
    toolsEnabled: true,
    contextTokens: 100000,
    outputTokens: 4096,
    ...(mcpBinary
      ? {
          mcpServers: [
            {
              name: "nessa",
              command: mcpBinary,
              args: [
                "--workspace",
                join(namespace, "workspaces/default"),
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
      `this script is running on Node ${process.versions.node}; the Claude ACP harness needs ${MINIMUM_NODE_MAJOR} or newer`,
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
 * The server reads `agent` as an `Option<AgentConfig>`, so an explicit `null`
 * is a valid configuration that means "no agent" — the gateway starts and says
 * so when a message is sent. Absent is the only state this script fills in;
 * `null` is somebody's answer and is left alone, the same as a block they
 * wrote themselves.
 */
export function agentIsSettled(existing) {
  return existing.agent !== undefined
}

/** Warn about an agent someone else owns whose executables have gone missing. */
function checkExisting(agent, path) {
  if (agent === null) {
    say(`→ ${path} sets "agent": null, which is a gateway with no agent`)
    say('  it was left as it is; remove the "agent" line and rerun to have one written')
    return
  }
  const missing = [agent.node, agent.acpEntry, agent.catalog].filter(
    (value) => typeof value === "string" && !existsSync(value),
  )
  if (missing.length === 0) {
    say(`→ dev agent already configured in ${path}; left as it is`)
    return
  }
  say(`→ dev agent in ${path} points at files that are not there:`)
  for (const value of missing) say(`    ${value}`)
  say('  it was left untouched. Reinstall what moved, or remove the "agent" block')
  say(
    `  and run the dev loop again to have one written (${INSTALL_HARNESS} installs the harness)`,
  )
}

function main() {
  if (process.platform === "win32")
    skip(
      "Claude ACP needs Unix process supervision, so a Windows checkout has no agent to configure",
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
    checkExisting(existing.agent, configPath)
    return
  }

  const checkout = realpathSync(root)
  const acpEntry = join(checkout, ACP_ENTRY)
  if (!existsSync(acpEntry))
    skip("the Claude ACP harness is not installed in this checkout", [
      `run: ${INSTALL_HARNESS}`,
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
  const agent = agentBlock({ checkout, namespace, node, mcpBinary })

  publish({ configPath, agent, acpEntry, node, mcpBinary })
}

/**
 * Writes the agent into whatever the file says at this moment.
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
 * Hold the right to write this configuration, or find out who has it.
 *
 * Re-reading before the rename narrows the window between deciding and writing;
 * it does not close it. Two runs can both read, both decide to write, and the
 * second rename silently replaces the first — atomic, and still a lost update.
 * So the read, the decision and the rename happen while holding this.
 *
 * `wx` is the whole mechanism: creating the file is the acquisition, and it
 * either succeeds or it does not. What is written into it is a token — the pid
 * for a person reading it, and a uuid so the file can be recognised as *this*
 * run's rather than merely as one written by some run with this pid.
 *
 * A lock left behind by a killed run is reported, never taken. Taking it cannot
 * be done safely with the operations available here: between reading a pid and
 * unlinking the file, that file can become a live run's lock, and deleting it
 * would hand the same configuration to two writers at once — the exact thing
 * the lock exists to prevent. `rm` is the wrong tool for "delete this only if
 * it still says 2147483647". So a stale lock is a sentence with the command
 * that clears it, said to the person who can tell that nothing is running.
 *
 * Honest about the other limit: this coordinates writers that take the lock. An
 * editor saving `config.json` underneath us takes no lock and is still a race —
 * a narrower one, between the read inside the lock and the rename, and not one
 * a file rename can settle.
 *
 * @returns {{ release: () => void } | { held: string }}
 */
function lockFor(configPath) {
  const lock = `${configPath}.lock`
  const token = `${process.pid} ${randomUUID()} ${new Date().toISOString()}\n`
  try {
    writeFileSync(lock, token, { mode: 0o600, flag: "wx" })
  } catch (error) {
    if (error.code !== "EEXIST")
      return { held: `could not lock ${lock}: ${error.message}` }
    return { held: readHolder(lock).description }
  }
  return {
    // Released only while it is still this run's own lock. If somebody cleared
    // it by hand and another run took it, the file at this path is theirs, and
    // unlinking it would leave them holding nothing.
    release: () => {
      try {
        if (readFileSync(lock, "utf8") === token) unlinkSync(lock)
      } catch {
        // Already gone, or unreadable: nothing this run may remove.
      }
    },
  }
}

/**
 * Who holds a lock, and what to do about it.
 *
 * A live holder is somebody to leave alone. A holder that is gone is a wedged
 * lock, and the description carries the command that clears it, because this
 * run will not clear it itself — see [`lockFor`].
 */
function readHolder(lock) {
  const clear = `nothing is writing it, run: rm ${lock}`
  let text = ""
  try {
    text = readFileSync(lock, "utf8")
  } catch {
    return { description: "a lock that vanished as it was read; try again" }
  }
  const pid = Number(text.trim().split(/\s+/)[0])
  if (!Number.isInteger(pid) || pid <= 0)
    return { description: `a lock naming no process (${text.trim()}); if ${clear}` }
  try {
    process.kill(pid, 0)
    return { description: `pid ${pid}, which is still running` }
  } catch (error) {
    // EPERM means it exists and is somebody else's, which is not stale.
    return error.code === "EPERM"
      ? { description: `pid ${pid}` }
      : { description: `pid ${pid}, which is gone — if ${clear}` }
  }
}

export function publish({ configPath, agent, acpEntry, node, mcpBinary, interrupt }) {
  const lock = lockFor(configPath)
  if ("held" in lock) {
    say(`→ not configuring ${configPath}: its lock is held by ${lock.held}`)
    return false
  }
  try {
    return underLock({ configPath, agent, acpEntry, node, mcpBinary, interrupt })
  } finally {
    lock.release()
  }
}

/** The read, the decision and the write, with the right to do them held. */
function underLock({ configPath, agent, acpEntry, node, mcpBinary, interrupt }) {
  interrupt?.()
  const existing = readExisting(configPath)
  // Somebody answered the question while this was working. Theirs stands —
  // the same courtesy an agent block already in the file gets.
  if (agentIsSettled(existing)) {
    checkExisting(existing.agent, configPath)
    return false
  }

  const merged = { ...existing, agent }
  const text = `${JSON.stringify(merged, null, 2)}\n`
  // Parse it back before it can become the file the gateway reads. A config.json
  // that does not parse is not a missing agent, it is a server that will not start.
  const round = JSON.parse(text)
  if (round.agent.acpEntry !== acpEntry || round.agent.node !== node)
    skip("the generated configuration did not survive a round trip", "please report this")

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
    skip(
      `could not write ${configPath} (${error.message})`,
      "check the namespace's permissions",
    )
  }

  say(`→ dev agent configured in ${configPath}`)
  say(`    node       ${node}`)
  say(`    acpEntry   ${acpEntry}`)
  say(`    workspace  ${agent.workspace}`)
  say(
    mcpBinary
      ? `    nessa MCP  ${mcpBinary}`
      : '    nessa MCP  not built; omitted (cargo build -p nessa-mcp, then delete the "agent" block and rerun)',
  )
  return true
}

// Both sides are resolved before comparison: Node resolves `import.meta.url`
// through symlinks and `/var` → `/private/var`, but leaves `argv[1]` as typed,
// and a script that silently did nothing would be the worst failure here.
const invoked = process.argv[1] ? realpathSync(process.argv[1]) : ""
if (invoked === realpathSync(fileURLToPath(import.meta.url))) main()
