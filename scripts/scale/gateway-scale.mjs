#!/usr/bin/env node
/**
 * How the gateway holds up under many conversations and many sockets at
 * once, with no desktop window and no model:
 *
 *   node scripts/scale/gateway-scale.mjs --conversations 10000 --sockets 2000
 *
 * It starts a release gateway (`target/release/nessa`, or `--binary`) in a
 * temporary `ci` namespace, with Claude's runtime pointed at the SDK's
 * `scale_agent` example (`cargo build --release -p nessa-sdk --example
 * scale_agent`): a stdio ACP agent of a few hundred kilobytes, so the agents'
 * own memory does not drown the gateway's. Clients are `@nessa/client`
 * sockets signed in as the owner, spread over `--workers` Node processes
 * (`client-worker.mjs`). The run:
 *
 *   1. opens `--sockets` sockets
 *   2. creates `--conversations` conversations, spread evenly over them
 *   3. sends every conversation one message and waits for its turn to end,
 *      `--rounds` times (the first round's agents are just open; the later
 *      rounds' have history)
 *   4. holds every conversation idle for `--hold-seconds`, to see what an
 *      idle conversation costs
 *   5. lists conversations once per socket
 *
 * while it samples the gateway once a second: resident memory, threads, open
 * descriptors, CPU time, child processes and their memory, and the size of
 * its data directory. stdout is one JSON verdict line; `--evidence <dir>`
 * also keeps the samples and the gateway's log. The verdict fails on any
 * failed operation, and on a p99 over `--p99-ms` in a phase that has one.
 *
 * Every knob is also an environment variable, `NESSA_SCALE_<NAME>`
 * (`NESSA_SCALE_CONVERSATIONS=10000`).
 */
import { spawn, spawnSync, fork } from "node:child_process"
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs"
import { cpus, tmpdir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { setTimeout as sleep } from "node:timers/promises"
import { fileURLToPath } from "node:url"
import { parseArgs } from "node:util"

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..")

const knobs = {
  conversations: { default: "100", help: "conversations created" },
  sockets: { default: "", help: "client sockets (default: conversations / 4)" },
  workers: {
    default: String(Math.max(1, Math.min(8, cpus().length))),
    help: "client processes",
  },
  parallel: {
    default: "64",
    help: "operations in flight per worker, at most four per socket",
  },
  rounds: {
    default: "2",
    help: "messages sent to every conversation, one round at a time",
  },
  "hold-seconds": {
    default: "5",
    help: "how long every conversation sits idle, sampled, before the list",
  },
  "agent-delay-ms": { default: "0", help: "how long the agent takes per prompt" },
  "request-timeout-ms": { default: "60000", help: "client request timeout" },
  "turn-timeout-ms": { default: "120000", help: "how long a turn may take to end" },
  "p99-ms": { default: "0", help: "fail a phase whose p99 is over this (0: no bound)" },
  limits: { default: "", help: "config.json limits, as JSON" },
  binary: { default: "", help: "the gateway binary" },
  agent: { default: "", help: "the scale agent binary" },
  evidence: { default: "", help: "a directory to keep samples and the gateway log in" },
}

const { values } = parseArgs({
  options: {
    ...Object.fromEntries(Object.keys(knobs).map((name) => [name, { type: "string" }])),
    help: { type: "boolean" },
  },
})
if (values.help) {
  process.stdout.write(
    `gateway-scale — many conversations and sockets on one gateway\n\n${Object.entries(
      knobs,
    )
      .map(
        ([name, k]) =>
          `  --${name.padEnd(20)} ${k.help} (default ${k.default || "derived"})`,
      )
      .join("\n")}\n`,
  )
  process.exit(0)
}
const knob = (name) =>
  values[name] ??
  process.env[`NESSA_SCALE_${name.toUpperCase().replaceAll("-", "_")}`] ??
  knobs[name].default
const number = (name) => {
  const value = Number(knob(name))
  if (!Number.isInteger(value) || value < 0)
    throw new Error(`--${name} must be a whole number`)
  return value
}

const conversations = number("conversations")
const sockets =
  knob("sockets") === "" ? Math.max(1, Math.ceil(conversations / 4)) : number("sockets")
const workers = Math.min(number("workers"), sockets)
// A socket admits four controls at once (`limits.controlSlots`), and a client
// past that is the client's fault, not the gateway's: each worker keeps its
// sockets within it.
const CONTROLS_PER_SOCKET = 4
const parallel = Math.min(
  number("parallel"),
  Math.max(1, Math.floor(sockets / workers)) * CONTROLS_PER_SOCKET,
)
const p99Bound = number("p99-ms")
const binary = resolve(knob("binary") || join(root, "target/release/nessa"))
const agentBinary = resolve(
  knob("agent") || join(root, "target/release/examples/scale_agent"),
)
const evidence = knob("evidence") ? resolve(knob("evidence")) : null
for (const [what, path, build] of [
  ["gateway", binary, "cargo build --release -p nessa-server"],
  [
    "scale agent",
    agentBinary,
    "cargo build --release -p nessa-sdk --example scale_agent",
  ],
])
  if (!existsSync(path)) {
    process.stderr.write(`gateway-scale: no ${what} at ${path}; build it: ${build}\n`)
    process.exit(2)
  }

const harness = JSON.parse(
  readFileSync(join(root, "crates/nessa-sdk/harnesses/claude-acp/package.json"), "utf8"),
)
const [[harnessName, harnessVersion]] = Object.entries(harness.dependencies)

const log = (line) => process.stderr.write(`gateway-scale: ${line}\n`)

/** A free port on 127.0.0.1. */
async function freePort() {
  const { createServer } = await import("node:net")
  const server = createServer().listen(0, "127.0.0.1")
  await new Promise((done) => server.once("listening", done))
  const { port } = server.address()
  await new Promise((done) => server.close(done))
  return port
}

/** The gateway's resources now, read from /proc (Linux only; elsewhere, nothing). */
function sample(pid, directory) {
  if (process.platform !== "linux") return null
  const status = Object.fromEntries(
    readFileSync(`/proc/${pid}/status`, "utf8")
      .split("\n")
      .map((line) => line.split(/:\s+/))
      .filter((pair) => pair.length === 2),
  )
  const kib = (value) => Number.parseInt(value ?? "0", 10)
  const stat = readFileSync(`/proc/${pid}/stat`, "utf8").split(") ")[1].split(" ")
  const ticks = 100
  let children = 0
  let childRssKib = 0
  for (const entry of readdirSync("/proc")) {
    if (!/^\d+$/.test(entry)) continue
    try {
      const fields = readFileSync(`/proc/${entry}/stat`, "utf8").split(") ")[1].split(" ")
      if (Number(fields[1]) !== pid) continue
      children += 1
      childRssKib +=
        Number(readFileSync(`/proc/${entry}/statm`, "utf8").split(" ")[1]) * 4
    } catch {}
  }
  let fds = 0
  try {
    fds = readdirSync(`/proc/${pid}/fd`).length
  } catch {}
  return {
    rssMib: Math.round(kib(status.VmRSS) / 1024),
    peakRssMib: Math.round(kib(status.VmHWM) / 1024),
    threads: Number(status.Threads),
    fds,
    cpuSeconds: (Number(stat[11]) + Number(stat[12])) / ticks,
    children,
    childRssMib: Math.round(childRssKib / 1024),
    dataMib: Math.round(directorySize(directory) / 1024 / 1024),
  }
}

function directorySize(path) {
  let total = 0
  for (const entry of readdirSync(path, { withFileTypes: true, recursive: true })) {
    if (!entry.isFile()) continue
    try {
      total += statSync(join(entry.parentPath ?? entry.path, entry.name)).size
    } catch {}
  }
  return total
}

const quantile = (sorted, q) =>
  sorted.length === 0
    ? null
    : Math.round(sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))])

function summarize(results) {
  const latencies = results.flatMap((r) => r.latencies).sort((a, b) => a - b)
  const failures = {}
  const samples = {}
  for (const r of results) {
    for (const [code, count] of Object.entries(r.failures))
      failures[code] = (failures[code] ?? 0) + count
    Object.assign(samples, r.samples)
  }
  return {
    ok: latencies.length,
    failed: Object.values(failures).reduce((a, b) => a + b, 0),
    failures,
    failureSamples: samples,
    p50: quantile(latencies, 0.5),
    p95: quantile(latencies, 0.95),
    p99: quantile(latencies, 0.99),
    max: latencies.length ? Math.round(latencies.at(-1)) : null,
  }
}

const directory = mkdtempSync(join(tmpdir(), "nessa-scale-"))
const instance = "scale"
let gateway
let children = []
let timer
const samples = []
const output = []
const verdict = {
  check: "gateway-scale",
  conversations,
  sockets,
  workers,
  parallel,
  phases: {},
}

// How long the gateway may take to stop every conversation's agent and
// confirm it, after SIGTERM.
const SHUTDOWN_MS = 120_000

const cleanup = async () => {
  clearInterval(timer)
  for (const child of children) child.kill("SIGKILL")
  if (gateway && gateway.exitCode === null) {
    gateway.kill("SIGTERM")
    const started = performance.now()
    const exited = await Promise.race([
      new Promise((done) => gateway.once("exit", () => done(true))),
      sleep(SHUTDOWN_MS, false),
    ])
    verdict.shutdown = {
      seconds: Number(((performance.now() - started) / 1000).toFixed(3)),
      exitCode: gateway.exitCode,
    }
    if (!exited) gateway.kill("SIGKILL")
  }
  if (evidence) {
    mkdirSync(evidence, { recursive: true })
    writeFileSync(join(evidence, "samples.json"), JSON.stringify(samples, null, 1))
    writeFileSync(join(evidence, "gateway.log"), output.join("").slice(-4_000_000))
  }
  rmSync(directory, { recursive: true, force: true })
}

try {
  const port = await freePort()
  mkdirSync(join(directory, "home"))
  mkdirSync(join(directory, "workspace"))
  const env = {
    PATH: process.env.PATH,
    HOME: join(directory, "home"),
    ...(process.env.RUST_LOG ? { RUST_LOG: process.env.RUST_LOG } : {}),
    ANTHROPIC_API_KEY: "signed-out-gateway-placeholder",
    NESSA_STAGE: "ci",
    NESSA_PORT: String(port),
    NESSA_HOST: "127.0.0.1",
    NESSA_DATA_DIR: directory,
    NESSA_INSTANCE: instance,
  }
  const token = join(directory, "owner.token")
  const init = spawnSync(
    binary,
    ["auth", "init", "--local", "--owner-token-file", token],
    {
      env,
      encoding: "utf8",
    },
  )
  if (init.status !== 0) throw new Error(`provisioning failed: ${init.stderr}`)
  const configPath = join(directory, "ci", "instances", instance, "config.json")
  writeFileSync(
    configPath,
    JSON.stringify({
      agents: {
        catalog: join(root, "crates/nessa-sdk/data/models.json"),
        workspace: join(directory, "workspace"),
        selected: "claude",
        mcpServers: [],
        runtimes: {
          claude: {
            command: agentBinary,
            args: [harnessName, harnessVersion, String(number("agent-delay-ms"))],
            model: "claude-sonnet-5",
            toolsEnabled: true,
          },
        },
      },
      ...(knob("limits") ? { limits: JSON.parse(knob("limits")) } : {}),
    }),
  )
  chmodSync(configPath, 0o600)
  gateway = spawn(binary, ["server"], { env, stdio: ["ignore", "pipe", "pipe"] })
  for (const stream of [gateway.stdout, gateway.stderr])
    stream.on("data", (chunk) => {
      output.push(chunk.toString())
      if (output.length > 20_000) output.splice(0, 10_000)
    })
  const started = Date.now()
  for (;;) {
    if (gateway.exitCode !== null)
      throw new Error(
        `gateway exited (${gateway.exitCode}):\n${output.join("").slice(-4000)}`,
      )
    try {
      if ((await fetch(`http://127.0.0.1:${port}/health`)).ok) break
    } catch {}
    if (Date.now() - started > 60_000) throw new Error("gateway never became healthy")
    await sleep(200)
  }
  let phase = "idle"
  const record = () => {
    try {
      const now = sample(gateway.pid, directory)
      if (now) samples.push({ at: Date.now() - started, phase, ...now })
    } catch {}
  }
  record()
  timer = setInterval(record, 1000)

  const credential = readFileSync(token, "utf8").trim()
  const perWorker = Array.from(
    { length: workers },
    (_, i) => Math.floor(sockets / workers) + (i < sockets % workers ? 1 : 0),
  )
  // Conversations per socket, as even as the counts allow: the first
  // `conversations % sockets` sockets take one more.
  const perSocket = Math.ceil(conversations / sockets)
  children = perWorker.map((count, worker) =>
    fork(join(root, "scripts/scale/client-worker.mjs"), [], {
      cwd: root,
      stdio: ["ignore", "inherit", "inherit", "ipc"],
      execArgv: ["--max-old-space-size=4096"],
    }),
  )
  const ask = (child, message) =>
    new Promise((done, fail) => {
      const listener = (reply) => {
        if (reply.type === "ready" && message.type === "setup") {
          child.off("message", listener)
          done()
        } else if (reply.type === "done" && reply.phase === message.type) {
          child.off("message", listener)
          if (reply.error) fail(new Error(reply.error))
          else done(reply.result)
        }
      }
      child.on("message", listener)
      child.once("exit", (code) => fail(new Error(`a client worker exited (${code})`)))
      child.send(message)
    })
  // The last worker's sockets take the remainder, so the total is exact.
  let assigned = 0
  await Promise.all(
    children.map((child, worker) => {
      const count = perWorker[worker]
      const remaining = conversations - assigned
      const mine =
        worker === workers - 1 ? remaining : Math.min(remaining, count * perSocket)
      assigned += mine
      return ask(child, {
        type: "setup",
        setup: {
          worker,
          url: `ws://127.0.0.1:${port}`,
          instance,
          credential,
          agent: "claude",
          sockets: count,
          perSocket: count === 0 ? 0 : Math.ceil(mine / count),
          limit: mine,
          parallel,
          requestTimeoutMs: number("request-timeout-ms"),
          turnTimeoutMs: number("turn-timeout-ms"),
        },
      })
    }),
  )
  const run = async (name, message = {}) => {
    phase = name
    log(`${name}...`)
    const began = Date.now()
    const results = await Promise.all(
      children.map((child) => ask(child, { type: name, ...message })),
    )
    const summary = { ...summarize(results), seconds: (Date.now() - began) / 1000 }
    if (name === "list") summary.listed = Math.max(...results.map((r) => r.listed ?? 0))
    if (results.some((r) => r.steps)) {
      summary.steps = {}
      for (const step of ["subscribe", "send", "end"]) {
        const times = results.flatMap((r) => r.steps?.[step] ?? []).sort((a, b) => a - b)
        summary.steps[step] = { p50: quantile(times, 0.5), p99: quantile(times, 0.99) }
      }
    }
    record()
    summary.after = samples.at(-1)
    verdict.phases[`${name}${message.label ?? ""}`] = summary
    log(`${name}: ${JSON.stringify({ ...summary, failureSamples: undefined })}`)
    return summary
  }
  await run("connect")
  await run("create")
  for (let round = 1; round <= number("rounds"); round += 1)
    await run("turn", { text: `round ${round}`, label: `-${round}` })
  // Every agent open, nothing asked of any: what idle conversations cost.
  phase = "hold"
  const held = samples.length
  await sleep(number("hold-seconds") * 1000)
  record()
  const during = samples.slice(held)
  if (during.length > 1) {
    const seconds = (during.at(-1).at - during[0].at) / 1000
    verdict.hold = {
      seconds,
      cpuCoresBusy: Number(
        ((during.at(-1).cpuSeconds - during[0].cpuSeconds) / seconds).toFixed(3),
      ),
      rssMib: during.at(-1).rssMib,
      threads: during.at(-1).threads,
      fds: during.at(-1).fds,
    }
    log(`hold: ${JSON.stringify(verdict.hold)}`)
  }
  await run("list")
  await run("close")
  phase = "idle"
  await sleep(2000)
  record()
  verdict.peak = {
    rssMib: Math.max(...samples.map((s) => s.rssMib)),
    threads: Math.max(...samples.map((s) => s.threads)),
    fds: Math.max(...samples.map((s) => s.fds)),
    children: Math.max(...samples.map((s) => s.children)),
    childRssMib: Math.max(...samples.map((s) => s.childRssMib)),
    dataMib: Math.max(...samples.map((s) => s.dataMib)),
    cpuSeconds: samples.at(-1)?.cpuSeconds,
  }
  verdict.gatewayAlive = gateway.exitCode === null
  const failures = []
  for (const [name, summary] of Object.entries(verdict.phases)) {
    if (summary.failed > 0)
      failures.push(
        `${name}: ${summary.failed} failed (${JSON.stringify(summary.failures)})`,
      )
    if (p99Bound > 0 && summary.p99 !== null && summary.p99 > p99Bound)
      failures.push(`${name}: p99 ${summary.p99} ms is over ${p99Bound} ms`)
  }
  if (!verdict.gatewayAlive) failures.push("the gateway exited during the run")
  // A conversation is listed once a turn in it ended, so the count is only
  // owed when every turn did; a failed turn is already reported above.
  const listed = verdict.phases.list?.listed
  const turnsFailed = Object.entries(verdict.phases).some(
    ([name, summary]) => name.startsWith("turn") && summary.failed > 0,
  )
  if (number("rounds") > 0 && !turnsFailed && listed !== conversations)
    failures.push(
      `a socket's list and observe pages named ${listed} conversations, not ${conversations}`,
    )
  verdict.failures = failures
  verdict.passed = failures.length === 0
} catch (error) {
  verdict.passed = false
  verdict.error = String(error?.stack ?? error)
} finally {
  await cleanup()
}
// A gateway that could not confirm every agent stopped exits non-zero; one
// that did not exit in time has no exit code yet.
if (verdict.shutdown && verdict.shutdown.exitCode !== 0) {
  verdict.failures = [
    ...(verdict.failures ?? []),
    `the gateway's shutdown ${verdict.shutdown.exitCode === null ? `did not end within ${SHUTDOWN_MS / 1000} s` : `exited ${verdict.shutdown.exitCode}`}`,
  ]
  verdict.passed = false
}
process.stdout.write(`${JSON.stringify(verdict)}\n`)
process.exit(verdict.passed ? 0 : 1)
