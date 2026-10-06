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
} = {}) {
  const source = {
    ...provenance,
    capturedAt: new Date().toISOString(),
    declaredClientCapabilities: capabilities,
  }
  let workspace
  let session
  let outcome
  let frames = []
  let admission
  try {
    workspace = mkdtempSync(join(workspaceParent, "nessa-subagent-contract-"))
    session = startProbeSession(command, args, {
      workspace,
      env,
      maxBytes,
      cleanupOptions,
    })
    const initialized = await session.request(
      "initialize",
      {
        protocolVersion: 1,
        clientInfo: { name: "nessa-subagent-probe", version: "1" },
        clientCapabilities: capabilities,
      },
      requestBudgetMs,
    )
    Object.assign(source, initializationMetadata(initialized.result, expectedAgent))
    const opened = await session.request(
      "session/new",
      { cwd: workspace, mcpServers: [] },
      requestBudgetMs,
    )
    if (!opened.result?.modes || !Array.isArray(opened.result.configOptions))
      throw { code: "session_invalid" }
    Object.assign(source, sessionMetadata(opened.result))
    const sessionId = opened.result.sessionId
    source.prompt = PROMPT
    await session.request(
      "session/prompt",
      { sessionId, prompt: [{ type: "text", text: PROMPT }] },
      promptBudgetMs,
    )
    await session.request("session/close", { sessionId }, requestBudgetMs)
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
    if (version.cleanup.kind !== "stopped") throw { code: "version_cleanup_unconfirmed" }
    outcome = { kind: "completed", ...verdict }
  } catch (error) {
    outcome = {
      kind: "failed",
      code: typeof error.code === "string" ? error.code : "probe_failed",
      ...(Number.isSafeInteger(error.rpcCode) ? { rpcCode: error.rpcCode } : {}),
    }
  } finally {
    source.probeCleanup = session
      ? await session.close()
      : { kind: "stopped", leaderReaped: true, liveGroupMembers: 0 }
    if (source.probeCleanup.kind !== "stopped") {
      outcome = {
        kind: "failed",
        code: outcome?.code ?? "cleanup_unconfirmed",
        cleanupCode: "cleanup_unconfirmed",
      }
    } else if (workspace) {
      try {
        rmSync(workspace, { recursive: true, force: true })
      } catch {
        outcome = {
          kind: "failed",
          code: outcome?.code ?? "workspace_cleanup_failed",
          cleanupCode: "workspace_cleanup_failed",
        }
      }
    }
  }
  return { source, outcome, ...(admission ? { admission } : {}), frames }
}

const hash = (path) => createHash("sha256").update(readFileSync(path)).digest("hex")
async function main(out) {
  const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..")
  const harness = join(root, "crates/nessa-sdk/harnesses/codex-acp")
  const entry = join(harness, "node_modules/@agentclientprotocol/codex-acp/dist/index.js")
  if (!out || !existsSync(entry)) {
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
    })
  } catch {
    result = {
      source: { kind: "live-provider-acp" },
      outcome: { kind: "failed", code: "setup_failed" },
      frames: [],
    }
  }
  writeFileSync(out, `${JSON.stringify(result, null, 2)}\n`)
  process.stdout.write(
    `${JSON.stringify({ ...result.outcome, ...(result.outcome.kind === "completed" ? { nativeSessionUpdates: result.source.nativeSessionUpdates, permissionRequests: result.source.permissionRequests } : {}) })}\n`,
  )
  if (result.outcome.kind !== "completed") process.exitCode = 1
}
if (process.argv[1] === fileURLToPath(import.meta.url)) {
  main(process.argv[2]).catch(() => {
    process.stderr.write("capture output could not be written\n")
    process.exitCode = 1
  })
}
