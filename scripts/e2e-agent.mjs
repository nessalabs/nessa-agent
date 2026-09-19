#!/usr/bin/env node
/**
 * End-to-end: real gateway, real Codex ACP harness subprocess, real message.
 *
 * Drives the whole path a panel takes — provision, connect, create a
 * conversation on Codex, send text, poll the view — against the vendor binary
 * rather than a fixture. Reports exactly how far it got.
 */
import { spawn, spawnSync } from "node:child_process"
import { setTimeout as sleep } from "node:timers/promises"
import {
  mkdtempSync,
  readFileSync,
  writeFileSync,
  mkdirSync,
  chmodSync,
  existsSync,
} from "node:fs"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import { randomUUID } from "node:crypto"
import { PROMPT, verdict } from "./e2e-verdict.mjs"

const root = join(dirname(fileURLToPath(import.meta.url)), "..")
const port = Number(process.env.E2E_PORT ?? 7430)
const harness = join(root, "crates/nessa-sdk/harnesses/codex-acp/node_modules")
const directory = mkdtempSync("/tmp/nessa-e2e-codex-")
const workspace = join(directory, "workspace")
mkdirSync(workspace)
const codexHome = join(directory, "codex-home")
mkdirSync(codexHome)

const agent = process.env.E2E_AGENT ?? "codex"
const model = process.env.E2E_MODEL ?? "gpt-5.6-luna"
// The ACP adapter is its own entry script under Node, not a `codex acp`
// subcommand — codex-cli 0.154.0 has no such subcommand, and pointing at it
// gets a process that exits immediately and reads as "provider stdout closed".
const command = process.execPath
const args = [join(harness, "@agentclientprotocol/codex-acp/dist/index.js")]

// The harness is installed, not committed, so a fresh checkout has none. Said
// here rather than left to the gateway, which can only report that a file it
// was pointed at does not exist — true, and no help at all in finding out why.
if (!existsSync(args[0])) {
  console.error(
    `no ACP harness at ${args[0]}\n` +
      `install it first: (cd crates/nessa-sdk/harnesses/codex-acp && npm install)`,
  )
  process.exit(1)
}

// Beside auth/, not at the top of the data directory, and readable by this user
// alone: the gateway refuses local storage any wider than that.
const configPath = join(directory, "ci", "instances", "e2e-codex", "config.json")
const config = {
  agents: {
    catalog: join(root, "crates/nessa-sdk/data/models.json"),
    workspace,
    selected: agent,
    runtimes: { [agent]: { command, args, model, toolsEnabled: true } },
  },
}

const env = {
  ...process.env,
  NESSA_STAGE: "ci",
  NESSA_PORT: String(port),
  NESSA_DATA_DIR: directory,
  NESSA_INSTANCE: "e2e-codex",
  CODEX_HOME: codexHome,
  // The gateway clears the agent's environment and forwards only a named set,
  // PATH among them. The ACP adapter shells out to the `codex` binary, which
  // lives in the harness, so it has to be findable on that PATH.
  PATH: `${join(harness, ".bin")}:${process.env.PATH}`,
}

const step = (name, detail) => console.log(`[${name}] ${detail}`)

/**
 * How many one-second reads the answer gets before the run is a failure.
 *
 * Running out is a failure and not a quiet ending: a gateway that admits the
 * request and never produces an answer is exactly the outage this script exists
 * to catch, and an exit status of zero for it would make every future run
 * meaningless.
 */
const POLLS = Number(process.env.E2E_POLLS ?? 60)

const init = spawnSync(
  "cargo",
  [
    "run",
    "-q",
    "-p",
    "nessa-server",
    "--",
    "auth",
    "init",
    "--local",
    "--owner-token-file",
    join(directory, "owner.token"),
  ],
  { cwd: root, env, encoding: "utf8" },
)
if (init.status !== 0) throw new Error(`provisioning failed: ${init.stderr}`)
writeFileSync(configPath, JSON.stringify(config, null, 2))
// Not writeFileSync's mode, which umask still widens.
chmodSync(configPath, 0o600)
step("provision", `owner credential issued; config at ${configPath}`)

const server = spawn("cargo", ["run", "-q", "-p", "nessa-server", "--", "server"], {
  cwd: root,
  env,
  stdio: ["ignore", "pipe", "pipe"],
})
const serverLog = []
for (const stream of [server.stdout, server.stderr])
  stream.on("data", (c) => serverLog.push(c.toString()))

let failed = false
try {
  for (let i = 0; ; i += 1) {
    try {
      if ((await fetch(`http://127.0.0.1:${port}/health`)).ok) break
    } catch {}
    if (i > 120) throw new Error(`server never became healthy:\n${serverLog.join("")}`)
    await sleep(250)
  }
  step("gateway", `healthy on 127.0.0.1:${port}`)

  const { NessaClient } = await import("@nessa/client")
  const client = await NessaClient.connect({
    stage: "ci",
    url: `ws://127.0.0.1:${port}`,
    role: "surface",
    surface: { kind: "panel", instance: "e2e" },
    client: { id: "e2e", version: "0.1.0", platform: "node" },
    profile: "product",
    auth: { credential: readFileSync(join(directory, "owner.token"), "utf8").trim() },
  })
  step("connect", "panel authenticated over /session")

  const id = randomUUID()
  const created = await client.conversation.create({ conversationId: id, agent })
  step("create", `conversation on "${agent}": ${JSON.stringify(created)}`)

  const view = await client.conversation.read(id)
  step("runtime", JSON.stringify(view.runtime ?? null))

  const sent = await client.conversation.send(id, PROMPT)
  step("send", `admitted: ${JSON.stringify(sent)}`)

  let last = null
  let answered = false
  for (let i = 0; i < POLLS; i += 1) {
    await sleep(1000)
    last = await client.conversation.read(id)
    const answer = verdict(last)
    // A turn that ended badly ends the run here, rather than being polled past
    // until the deadline reports the wrong reason for the wrong thing.
    if (answer.state === "failed") throw new Error(answer.detail)
    if (answer.state === "answered") {
      step("RESPONSE", JSON.stringify(answer.reply))
      answered = true
      break
    }
    if (i % 10 === 9)
      step("poll", `${i + 1}s: ${answer.detail}, ${last.pending.length} pending`)
  }
  if (!answered)
    throw new Error(
      `no completed answer within ${POLLS}s; last turn: ` +
        JSON.stringify(last?.messages.map((message) => message.status) ?? null),
    )
  step("final-view", JSON.stringify(last, null, 2).slice(0, 4000))
  client.close()
} catch (error) {
  failed = true
  console.error("[FAILED]", error?.stack ?? error)
} finally {
  console.log("\n===== SERVER LOG =====\n" + serverLog.join("").slice(-8000))
  server.kill("SIGTERM")
  await sleep(1500)
  if (server.exitCode === null) server.kill("SIGKILL")
  console.log(`\n[artifacts] ${directory}`)
}
process.exit(failed ? 1 : 0)
