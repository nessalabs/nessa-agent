#!/usr/bin/env node
/** Direct live provider probe. Existing sign-in; empty workspace; no product binding. */
import { createHash } from "node:crypto"
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { startProbeSession } from "./acp-session.mjs"
import { providerVersion } from "./processes.mjs"
import { selectCapture, inspectCapture } from "./evidence.mjs"
import { initializationMetadata, sessionMetadata } from "./metadata.mjs"

export const PROMPT =
  "Use your native subagent capability to spawn one child agent. Tell the child to reply with exactly CHILD_DONE without tools, files, network, or commands. Wait for the child to finish, close it if a close operation is available, then reply exactly PARENT_DONE. Do not perform any other actions."
const capabilities = {
  subagents: {},
  fs: { readTextFile: false, writeTextFile: false },
  terminal: false,
}

function attemptFailure(error, cleanupCode) {
  const code = typeof error?.code === "string" ? error.code : "probe_failed"
  const signal = error?.signal
  return {
    kind: "failed",
    code,
    ...(Number.isSafeInteger(error?.rpcCode) ? { rpcCode: error.rpcCode } : {}),
    ...(signal === "SIGINT" || signal === "SIGTERM" ? { signal } : {}),
    ...(cleanupCode ? { cleanupCode } : {}),
  }
}

function retainFailure(outcome, cleanupCode) {
  return attemptFailure(
    {
      code: outcome?.code ?? cleanupCode,
      rpcCode: outcome?.rpcCode,
      signal: outcome?.signal,
    },
    cleanupCode,
  )
}

/** Catchable manual SIGINT/SIGTERM. SIGKILL and a crash do not run this. */
function manualInterruption() {
  let interruption
  let deliver = () => {}
  const note = (signal) => {
    if (interruption) return
    interruption = { code: "interrupted", signal }
    deliver(interruption)
  }
  const listeners = [
    ["SIGINT", () => note("SIGINT")],
    ["SIGTERM", () => note("SIGTERM")],
  ]
  for (const [signal, listener] of listeners) process.on(signal, listener)
  return {
    current: () => interruption,
    subscribe(fn) {
      deliver = fn
      if (interruption) fn(interruption)
    },
    stop() {
      for (const [signal, listener] of listeners) process.removeListener(signal, listener)
      deliver = () => {}
    },
  }
}

/** A truthful attempt result is returned only after the common cleanup owner settles. */
export async function captureProbe({
  command = process.execPath,
  args,
  env,
  source: provenance,
  expectedAgent,
  versionCommand,
  versionArgs,
  workspaceParent = tmpdir(),
  maxBytes,
  cleanupOptions,
  requestBudgetMs = 45_000,
  promptBudgetMs = 90_000,
  versionOptions,
  interrupt,
} = {}) {
  const source = {
    ...provenance,
    capturedAt: new Date().toISOString(),
    declaredClientCapabilities: capabilities,
  }
  const noted = () => {
    const error = interrupt?.current()
    if (error) throw error
  }
  // One in-flight RPC. A catchable signal rejects it; the caller still awaits
  // the shared cleanup owner before returning.
  const settle = (promise) => {
    if (!interrupt) return promise
    return new Promise((resolve, reject) => {
      let settled = false
      const finish = (failed, value) => {
        if (settled) return
        settled = true
        if (failed) reject(value)
        else resolve(value)
      }
      const pending = interrupt.current()
      if (pending) finish(true, pending)
      else interrupt.subscribe((error) => finish(true, error))
      promise.then(
        (value) => finish(false, value),
        (error) => finish(true, error),
      )
    })
  }
  let workspace
  let session
  let outcome
  let frames = []
  let admission
  try {
    noted()
    workspace = mkdtempSync(join(workspaceParent, "nessa-subagent-contract-"))
    noted()
    session = startProbeSession(command, args, {
      workspace,
      env,
      maxBytes,
      cleanupOptions,
    })
    noted()
    const initialized = await settle(
      session.request(
        "initialize",
        {
          protocolVersion: 1,
          clientInfo: { name: "nessa-subagent-probe", version: "1" },
          clientCapabilities: capabilities,
        },
        requestBudgetMs,
      ),
    )
    noted()
    Object.assign(source, initializationMetadata(initialized.result, expectedAgent))
    noted()
    const opened = await settle(
      session.request("session/new", { cwd: workspace, mcpServers: [] }, requestBudgetMs),
    )
    noted()
    if (!opened.result?.modes || !Array.isArray(opened.result.configOptions))
      throw { code: "session_invalid" }
    Object.assign(source, sessionMetadata(opened.result))
    const sessionId = opened.result.sessionId
    source.prompt = PROMPT
    noted()
    await settle(
      session.request(
        "session/prompt",
        { sessionId, prompt: [{ type: "text", text: PROMPT }] },
        promptBudgetMs,
      ),
    )
    noted()
    await settle(session.request("session/close", { sessionId }, requestBudgetMs))
    const selected = selectCapture(session.seal())
    frames = selected.frames
    admission = selected.admission
    source.parentClose = { kind: "rpc_response", result: {} }
    source.permissionRequests = selected.statistics.permissionRequests
    source.nativeSessionUpdates = selected.statistics.nativeSessionUpdates
    const verdict = inspectCapture(selected)
    const version = await providerVersion(versionCommand, versionArgs, versionOptions)
    source.providerVersion = version.value
    source.versionCleanup = version.cleanup
    noted()
    if (version.cleanup.kind !== "stopped") throw { code: "version_cleanup_unconfirmed" }
    noted()
    outcome = { kind: "completed", ...verdict }
  } catch (error) {
    outcome = attemptFailure(error)
  } finally {
    source.probeCleanup = session
      ? await session.close()
      : { kind: "stopped", leaderReaped: true, liveGroupMembers: 0 }
    if (source.probeCleanup.kind !== "stopped")
      outcome = retainFailure(outcome, "cleanup_unconfirmed")
    else if (workspace) {
      try {
        rmSync(workspace, { recursive: true, force: true })
      } catch {
        outcome = retainFailure(outcome, "workspace_cleanup_failed")
      }
    }
  }
  return { source, outcome, ...(admission ? { admission } : {}), frames }
}

const hash = (path) => createHash("sha256").update(readFileSync(path)).digest("hex")
const USAGE =
  "capture.mjs <output.json> [--controlled <provider> <scenario> <marker> <workspaceParent> [--grace ms --confirm ms] [--version-scenario name] [--version-budget ms]]\n"

/** Live capture, or the same command with a scripted provider for interruption regressions. */
export function parseManualArgs(argv) {
  if (
    !Array.isArray(argv) ||
    argv.length === 0 ||
    typeof argv[0] !== "string" ||
    argv[0] === ""
  )
    return { kind: "invalid" }
  if (argv.length === 1) {
    if (argv[0].startsWith("--")) return { kind: "invalid" }
    return { kind: "live", out: argv[0] }
  }
  if (argv[1] !== "--controlled") return { kind: "invalid" }
  const positional = []
  const flags = {}
  for (let index = 2; index < argv.length; index++) {
    const token = argv[index]
    if (
      token === "--grace" ||
      token === "--confirm" ||
      token === "--version-scenario" ||
      token === "--version-budget"
    ) {
      const value = argv[index + 1]
      if (value === undefined || value.startsWith("--") || Object.hasOwn(flags, token))
        return { kind: "invalid" }
      flags[token] = value
      index += 1
      continue
    }
    if (typeof token !== "string" || token.startsWith("--")) return { kind: "invalid" }
    positional.push(token)
  }
  if (positional.length !== 4) return { kind: "invalid" }
  const [provider, scenario, marker, workspaceParent] = positional
  if (![provider, scenario, marker, workspaceParent].every((value) => value.length > 0))
    return { kind: "invalid" }
  if (!/^[a-z0-9-]+$/.test(scenario)) return { kind: "invalid" }
  let cleanupOptions
  if (Object.hasOwn(flags, "--grace") || Object.hasOwn(flags, "--confirm")) {
    if (!Object.hasOwn(flags, "--grace") || !Object.hasOwn(flags, "--confirm"))
      return { kind: "invalid" }
    const graceMs = Number(flags["--grace"])
    const confirmMs = Number(flags["--confirm"])
    if (
      !Number.isSafeInteger(graceMs) ||
      !Number.isSafeInteger(confirmMs) ||
      graceMs < 0 ||
      confirmMs < 0
    )
      return { kind: "invalid" }
    cleanupOptions = { graceMs, confirmMs }
  }
  const versionScenario = Object.hasOwn(flags, "--version-scenario")
    ? flags["--version-scenario"]
    : "version-ok"
  if (!/^[a-z0-9-]+$/.test(versionScenario)) return { kind: "invalid" }
  let versionBudgetMs = 2000
  if (Object.hasOwn(flags, "--version-budget")) {
    versionBudgetMs = Number(flags["--version-budget"])
    if (!Number.isSafeInteger(versionBudgetMs) || versionBudgetMs < 0)
      return { kind: "invalid" }
  }
  return {
    kind: "controlled",
    out: argv[0],
    provider,
    scenario,
    marker,
    workspaceParent,
    ...(cleanupOptions ? { cleanupOptions } : {}),
    versionScenario,
    versionBudgetMs,
  }
}

async function main(invocation, interrupt) {
  if (!invocation || invocation.kind === "invalid") {
    process.stderr.write(USAGE)
    process.exitCode = 2
    return
  }
  const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..")
  const harness = join(root, "crates/nessa-sdk/harnesses/codex-acp")
  const entry = join(harness, "node_modules/@agentclientprotocol/codex-acp/dist/index.js")
  if (invocation.kind === "live" && !existsSync(entry)) {
    process.stderr.write(
      "capture.mjs <output.json> requires the installed pinned Codex ACP harness\n",
    )
    process.exitCode = 2
    return
  }
  // Load provenance before acquiring a process, so filesystem/setup failure
  // cannot strand a spawned adapter outside the capture's cleanup owner.
  let result
  try {
    if (invocation.kind === "controlled") {
      result = await captureProbe({
        args: [invocation.provider, invocation.scenario, invocation.marker],
        source: { kind: "controlled-manual-probe" },
        expectedAgent: { name: "scripted-fixture", version: "1" },
        workspaceParent: invocation.workspaceParent,
        versionCommand: process.execPath,
        versionArgs: [
          invocation.provider,
          invocation.versionScenario,
          `${invocation.marker}.version`,
        ],
        versionOptions: {
          budgetMs: invocation.versionBudgetMs,
          graceMs: 100,
          confirmMs: 2000,
        },
        cleanupOptions: invocation.cleanupOptions,
        requestBudgetMs: 5000,
        promptBudgetMs: 5000,
        interrupt,
      })
    } else {
      const packageInfo = JSON.parse(
        readFileSync(
          join(harness, "node_modules/@agentclientprotocol/codex-acp/package.json"),
          "utf8",
        ),
      )
      const source = {
        kind: "live-provider-acp",
        adapter: `${packageInfo.name}@${packageInfo.version}`,
        adapterSha256: hash(entry),
        lockSha256: hash(join(harness, "package-lock.json")),
        probeSha256: hash(fileURLToPath(import.meta.url)),
        sessionSha256: hash(fileURLToPath(new URL("./acp-session.mjs", import.meta.url))),
        processSha256: hash(fileURLToPath(new URL("./processes.mjs", import.meta.url))),
        selectorSha256: hash(fileURLToPath(new URL("./evidence.mjs", import.meta.url))),
        metadataSha256: hash(fileURLToPath(new URL("./metadata.mjs", import.meta.url))),
        model: "gpt-5.6-luna",
        platform: `${process.platform}/${process.arch}`,
      }
      result = await captureProbe({
        args: [entry],
        env: {
          ...process.env,
          PATH: `${join(harness, "node_modules/.bin")}:${process.env.PATH}`,
          INITIAL_AGENT_MODE: "read-only",
          CODEX_CONFIG: JSON.stringify({
            model: source.model,
            features: { multi_agent: true },
          }),
        },
        source,
        expectedAgent: { name: packageInfo.name, version: packageInfo.version },
        versionCommand: join(harness, "node_modules/.bin/codex"),
        interrupt,
      })
    }
  } catch {
    result = {
      source: {
        kind:
          invocation.kind === "controlled"
            ? "controlled-manual-probe"
            : "live-provider-acp",
      },
      outcome: { kind: "failed", code: "setup_failed" },
      frames: [],
    }
  }
  writeFileSync(invocation.out, `${JSON.stringify(result, null, 2)}\n`)
  process.stdout.write(
    `${JSON.stringify({ ...result.outcome, ...(result.outcome.kind === "completed" ? { nativeSessionUpdates: result.source.nativeSessionUpdates, permissionRequests: result.source.permissionRequests } : {}) })}\n`,
  )
  if (result.outcome.kind !== "completed") process.exitCode = 1
}
if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const interrupt = manualInterruption()
  main(parseManualArgs(process.argv.slice(2)), interrupt)
    .catch(() => {
      process.stderr.write("capture output could not be written\n")
      process.exitCode = 1
    })
    .finally(() => interrupt.stop())
}
