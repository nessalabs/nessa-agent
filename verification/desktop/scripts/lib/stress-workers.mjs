/**
 * Paired machines for the conversation stress check's `pool` step
 * (`conversation-stress.mjs`): real `nessa env dial` processes, the command a
 * worker container runs (docs/guides/local-auth.md, on a paired machine in a
 * container). Each has a data directory and `config.json` of its own that run
 * the scripted agent, enrolls with an environment invitation the gateway's
 * owner creates, is granted every agent, and is approved, so the gateway's
 * pool (`peer:any`) has it to place conversations on.
 *
 * A worker is named as the gateway names it, `peer:` and the device key that
 * claimed its invitation, so a binding that names a worker says which process
 * it is. Every worker started is stopped and its directory removed by
 * `close()`, whatever happened before.
 */
import { spawn, spawnSync } from "node:child_process"
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { setTimeout as sleep } from "node:timers/promises"

import {
  exited,
  repoRoot,
  signedOutEnvironment,
} from "../../../../scripts/mcp-test-server/local-gateway.mjs"

/** How long a drain may take: below the stop grace the step allows it. */
export const DRAIN_DEADLINE = "20s"
/** The instance a worker's data directory holds; its own, never the gateway's. */
const INSTANCE = "stress-worker"

const nessa = () => process.env.MCP_LIVE_NESSA ?? join(repoRoot, "target/debug/nessa")

const hex = (bytes) => bytes.map((byte) => byte.toString(16).padStart(2, "0")).join("")

/**
 * Starts one worker for the gateway whose native listener is `address`,
 * paired by `owner` (a client signed in as the gateway's owner), running
 * `agent` as `launch` (`scriptedLaunch`) says. `label` names it in its
 * messages; `connected(name)` resolves whether the gateway came to list the
 * worker named `name` as connected. Resolves once it is approved, granted,
 * and connected.
 */
export async function startWorker({ owner, address, agent, launch, label, connected }) {
  const directory = mkdtempSync(join(tmpdir(), `nessa-stress-worker-${label}-`))
  const output = []
  let child = null
  const worker = {
    label,
    directory,
    name: null,
    get process() {
      return child
    },
    log: () => output.join(""),
    /** Sends `signal` and waits up to `ms` for the exit; whether it exited. */
    async stop(signal = "SIGTERM", ms = 30_000) {
      if (!child || child.exitCode !== null || child.signalCode !== null) return true
      child.kill(signal)
      return exited(child, ms)
    },
    /** Stops it, at once if it does not drain, and removes its directory. */
    async close() {
      try {
        if (!(await worker.stop("SIGTERM", 30_000))) await worker.stop("SIGKILL", 5_000)
      } finally {
        rmSync(directory, { recursive: true, force: true })
      }
    },
  }
  try {
    mkdirSync(join(directory, "home"))
    const env = {
      ...signedOutEnvironment(directory),
      NESSA_STAGE: "ci",
      NESSA_DATA_DIR: directory,
      NESSA_INSTANCE: INSTANCE,
    }
    const workspace = join(directory, "workspace")
    mkdirSync(workspace)
    const instance = join(directory, "ci", "instances", INSTANCE)
    mkdirSync(instance, { recursive: true, mode: 0o700 })
    // The scripted agent by an absolute path, as `env serve` requires. No
    // tool servers: a session the gateway starts here is given none of them.
    writeFileSync(
      join(instance, "config.json"),
      JSON.stringify({
        agents: {
          catalog: join(repoRoot, "crates/nessa-sdk/data/models.json"),
          workspace,
          selected: agent,
          runtimes: {
            [agent]: {
              command: launch.command.argv[0],
              args: launch.command.argv.slice(1),
              model: launch.command.model,
              toolsEnabled: true,
            },
          },
        },
      }),
      { mode: 0o600 },
    )
    const { code, status } = await owner.pairing.create({ enrollee: "environment" })
    const codeFile = join(directory, "enroll-code")
    writeFileSync(codeFile, code, { mode: 0o600 })
    // The same command on every start, as a container runs it: the first
    // enrolls with the code, a later one finds the pairing and dials.
    const dial = () => {
      child = spawn(
        nessa(),
        [
          "env",
          "dial",
          address,
          "--enroll-code-file",
          codeFile,
          "--drain-deadline",
          DRAIN_DEADLINE,
        ],
        { env, stdio: ["ignore", "pipe", "pipe"] },
      )
      for (const stream of [child.stdout, child.stderr])
        stream.on("data", (chunk) => output.push(chunk.toString()))
    }
    dial()
    // Enrolling claims the invitation with the worker's key.
    let claimed
    for (let tries = 0; tries < 120 && !claimed; tries += 1) {
      if (child.exitCode !== null)
        throw new Error(`worker ${label} exited ${child.exitCode} before it enrolled`)
      claimed = (await owner.pairing.status(status.invitationId)).claimedDeviceKey
      if (!claimed) await sleep(250)
    }
    if (!claimed) throw new Error(`worker ${label} never claimed its invitation`)
    worker.name = `peer:${hex(claimed)}`
    const approved = await owner.pairing.approve(status.invitationId, claimed)
    if (approved.activationStopped)
      throw new Error(
        `worker ${label}'s approval stopped ${approved.activationStopped}: ${approved.status.phase}`,
      )
    // A dialed gateway is granted nothing until the machine's owner says so,
    // against its data directory, naming the gateway its pairing approved:
    // which `env grant` reads only while no `env dial` holds that pairing.
    // So the worker is stopped once it has learned it was approved, granted,
    // and started again, as a container is replaced on its volume.
    if (!(await connected(worker.name)))
      throw new Error(`worker ${label} never dialed in once approved`)
    if (!(await worker.stop("SIGTERM", 30_000)) || child.exitCode !== 0)
      throw new Error(
        `worker ${label} stopped ${child.exitCode ?? child.signalCode} before its grant`,
      )
    const grant = spawnSync(nessa(), ["env", "grant", "--all-agents"], {
      env,
      encoding: "utf8",
    })
    if (grant.status !== 0)
      throw new Error(`env grant on worker ${label} failed: ${grant.stderr.trim()}`)
    dial()
    if (!(await connected(worker.name)))
      throw new Error(`worker ${label} never dialed in again once granted`)
    return worker
  } catch (error) {
    error.message = `${error.message}\n${worker.log().slice(-2000)}`
    await worker.close()
    throw error
  }
}
