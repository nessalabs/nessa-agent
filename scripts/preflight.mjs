#!/usr/bin/env node
/**
 * What has to be true before a window is worth opening.
 *
 * Six ways to start this wrong turned up in one evening, and every one of them
 * looked the same from the outside: a blank window, or an agent that would not
 * run, with nothing on screen saying which of the six it was. The cost was not
 * the faults themselves — most are one command to fix — but that finding out
 * which one you had meant reading logs, source, or both.
 *
 * So the checks run first, in the order a start actually depends on, and each
 * one says what it is doing. The rule about what this may do to fix what it
 * finds:
 *
 *   * What this run owns, it repairs — an agent config it would have written
 *     anyway, a binary it is about to run. Repairing those is doing the job,
 *     not taking a liberty.
 *   * What belongs to something else, it refuses and explains — a port held by
 *     a launchd service the installed app depends on is not this script's to
 *     stop. It names the holder and the exact command, and leaves the choice.
 *
 * Every refusal ends with what to run. An agent reading this needs the next
 * command, not a diagnosis it has to translate into one.
 *
 *   stage ─▶ agent config ─▶ server binary ─▶ (caller starts the gateway)
 *             repair            repair
 *
 * Ports are not checked here. `free-gateway-port.mjs` already does it, and does
 * it well — it names the service, the pid, the runtime path, what stopping it
 * costs, and both ways out. This runs before it and does not duplicate it.
 */
import { execFileSync, spawnSync } from "node:child_process"
import { existsSync, readFileSync } from "node:fs"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

import { ACP_ENTRY, HARNESS, agentIsSettled, namespaceRoot } from "./dev-agent-config.mjs"
import { gatewayPort, selectedPort } from "./gateway-port.mjs"

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..")

/** Stages the port table knows. Anything else is a typo, caught before it spreads. */
function checkStage(stage) {
  try {
    gatewayPort(stage)
  } catch {
    fail(
      `"${stage}" is not a stage.`,
      "Stages are named in protocol/defaults/gateway-ports.json.",
      "  just start        # dev",
      "  just start prod",
    )
  }
  return stage
}

function say(line) {
  process.stdout.write(`→ ${line}\n`)
}

/** A refusal is a thing to run, not a thing to interpret. */
function fail(...lines) {
  process.stderr.write(`\n${lines.join("\n")}\n\n`)
  process.exit(1)
}

/**
 * The agent's own runtime, which a fresh checkout does not have.
 *
 * It is installed by `npm ci` inside the harness rather than by this
 * repository's `pnpm install`, so every new worktree starts without it. The
 * config step then refuses — correctly — to write an agent block pointing at
 * files that are not there, says so, and exits 0. That warning scrolls past,
 * the gateway starts with no agent, and the panel reports the agent as not
 * installed. Which it is. Nothing in that chain is wrong except that a warning
 * carried the weight of a failure.
 *
 * Installing it is this checkout's own business, so it is done rather than
 * reported.
 */
function checkHarness() {
  if (existsSync(join(root, ACP_ENTRY))) {
    say("agent runtime installed")
    return
  }
  say(`agent runtime missing; installing it into ${HARNESS}`)
  try {
    execFileSync("npm", ["ci", "--omit=dev"], {
      stdio: "inherit",
      cwd: join(root, HARNESS),
    })
  } catch {
    // Left to the caller: this needs the network, and a machine without one
    // has a different problem than a missing install.
  }
  if (!existsSync(join(root, ACP_ENTRY))) {
    fail(
      "The agent's runtime is not installed, so the gateway would have no agent to talk to.",
      `It belongs at ${ACP_ENTRY}`,
      "",
      "Install it, then start again:",
      `  (cd ${HARNESS} && npm ci --omit=dev)`,
    )
  }
  say("agent runtime installed")
}

/**
 * The agent config the gateway will read — the instance's, not the stage's.
 *
 * These parted company once already: the config step looked at the stage's
 * `config.json`, found an agent, and reported the work done, while the gateway
 * under an instance read the instance's own file and found none. The panel then
 * said the agent was not installed, which was true of the file it read and of
 * no other.
 *
 * So this asks the same question of the same file the gateway will open, after
 * the config step has had its turn, and repairs it by running that step for
 * this namespace rather than by writing a config of its own.
 */
function checkAgent(stage) {
  const namespace = namespaceRoot({ ...process.env, NESSA_STAGE: stage })
  const configPath = join(namespace, "config.json")
  const settled = () => {
    if (!existsSync(configPath)) return false
    try {
      // The whole config, which is what `agentIsSettled` asks about — the same
      // question the config step asks, so the two cannot disagree about it.
      return agentIsSettled(JSON.parse(readFileSync(configPath, "utf8")))
    } catch {
      return false
    }
  }
  if (settled()) {
    say(`agent configured in ${configPath}`)
    return
  }
  say(`no agent in ${configPath}; writing one`)
  const wrote = spawnSync(
    process.execPath,
    [join(root, "scripts/dev-agent-config.mjs")],
    {
      stdio: "inherit",
      env: { ...process.env, NESSA_STAGE: stage },
    },
  )
  if (wrote.status !== 0 || !settled()) {
    fail(
      `The gateway would start without an agent, and nothing could be sent to it.`,
      `It reads ${configPath}, which has no usable "agent" section.`,
      "",
      "Write one, then start again:",
      `  NESSA_STAGE=${stage}${process.env.NESSA_INSTANCE ? ` NESSA_INSTANCE=${process.env.NESSA_INSTANCE}` : ""} node scripts/dev-agent-config.mjs`,
    )
  }
  say(`agent written to ${configPath}`)
}

/**
 * The server binary, built before anything waits on it.
 *
 * `just start` waits for the gateway to answer and gives up after a timeout.
 * On a cold branch cargo is still compiling when that timeout expires, so the
 * recipe stopped and took the app with it — a build in progress reported as a
 * server that would not start. Building first makes the wait mean what it says.
 */
function checkServer() {
  const binary = join(root, "target/debug/nessa")
  if (existsSync(binary)) {
    say("server binary present")
    return
  }
  say("server not built yet; building it before anything waits on it")
  try {
    execFileSync("cargo", ["build", "-p", "nessa-server"], {
      stdio: "inherit",
      cwd: root,
    })
  } catch {
    fail(
      "The gateway could not be built, so there is nothing to start.",
      "The compiler's own output is above.",
      "",
      "  cargo build -p nessa-server",
    )
  }
}

function main() {
  const stage = checkStage((process.argv[2] ?? "dev").trim())
  const instance = process.env.NESSA_INSTANCE
  say(
    `stage ${stage}${instance ? `, instance ${instance}` : ""}, gateway :${gatewayPort(stage)}`,
  )
  checkHarness()
  checkAgent(stage)
  checkServer()
}

main()
