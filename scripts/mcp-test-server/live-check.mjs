#!/usr/bin/env node
/**
 * Live check: a real gateway, a real agent harness, real model turns, and the
 * test MCP server — recording what the harness sends over ACP for each MCP
 * tool call and what the gateway's conversation view then says about it.
 *
 *   cargo build -p nessa-server
 *   pnpm exec tsx scripts/mcp-test-server/live-check.mjs <claude|codex|opencode> [out-dir]
 *
 * (`tsx`, because `@nessa/client` is TypeScript in this checkout.) It uses
 * whatever sign-in the agent already has on this machine — Claude's from the
 * keychain the gateway reads, Codex's from its own home, Opencode's from
 * Nessa's credential store — and creates no account and writes no credential.
 * A gateway without one refuses the conversation, and the run fails there.
 * It allows only calls to the test server's tools, each once; anything else
 * the agent asks for is left unanswered. It exits non-zero unless the turn
 * completed.
 *
 * Writes `<out-dir>/<agent>/`: `acp.jsonl` and `mcp.jsonl` (every frame, both
 * directions), `view.json` (the final conversation view), `summary.json` (the
 * MCP tool frames' shapes, the view's MCP tools, every place a `ui://`
 * resource appeared, the MCP calls made) and `gateway.log`. Review them before
 * checking any of them in. The gateway's own data directory is removed at the
 * end.
 *
 * Environment: `MCP_LIVE_HARNESSES` — the directory holding `claude-acp/` and
 * `codex-acp/` with their `node_modules` (default: this checkout's
 * `crates/nessa-sdk/harnesses`); `MCP_LIVE_OPENCODE` — an Opencode 1.18.31
 * binary; `MCP_LIVE_PORT` (default 7431); `MCP_LIVE_POLLS` (seconds, default 300).
 */
import { spawn, spawnSync } from "node:child_process"
import { randomUUID } from "node:crypto"
import {
  rmSync,
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  writeFileSync,
} from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { setTimeout as sleep } from "node:timers/promises"
import { fileURLToPath } from "node:url"
import {
  frameShape,
  permissionDecisions,
  parseRecording,
  toolFrames,
  uiMentions,
  viewTools,
} from "./evidence.mjs"

const here = dirname(fileURLToPath(import.meta.url))
const root = resolve(here, "../..")
const SERVER = "mcptest"

/** What the model is asked to do: call every tool once, in order. */
export const PROMPT = [
  `Use the tools of the "${SERVER}" MCP server. Call each of these exactly once, in this order,`,
  "waiting for each result before the next: report_rows (no arguments), link_resources",
  '(no arguments), rows.get with {"id": 2}, always_fails (no arguments), show_chart (no',
  "arguments). Do not use any other tool. When all five have returned, reply with DONE.",
].join(" ")

function agentCommand(agent) {
  const harnesses =
    process.env.MCP_LIVE_HARNESSES ?? join(root, "crates/nessa-sdk/harnesses")
  const entry = (name, path) => {
    const file = join(harnesses, name, "node_modules", path)
    if (!existsSync(file)) throw new Error(`no ${name} harness at ${file}`)
    return file
  }
  switch (agent) {
    case "claude":
      return {
        argv: [
          process.execPath,
          entry("claude-acp", "@agentclientprotocol/claude-agent-acp/dist/index.js"),
        ],
        model: "claude-sonnet-5",
        path: null,
      }
    case "codex":
      return {
        argv: [
          process.execPath,
          entry("codex-acp", "@agentclientprotocol/codex-acp/dist/index.js"),
        ],
        model: "gpt-5.6-terra",
        path: join(harnesses, "codex-acp", "node_modules", ".bin"),
      }
    case "opencode": {
      const binary = process.env.MCP_LIVE_OPENCODE
      if (!binary || !existsSync(binary))
        throw new Error("set MCP_LIVE_OPENCODE to an Opencode 1.18.31 binary")
      return {
        argv: [binary, "acp"],
        model: "opencode/nemotron-3-ultra-free",
        path: null,
      }
    }
    default:
      throw new Error(`unknown agent ${agent}`)
  }
}

/** Wait up to `ms` for `child` to exit; whether it did. */
async function exited(child, ms) {
  if (child.exitCode !== null || child.signalCode !== null) return true
  return Promise.race([
    new Promise((done) => child.once("exit", () => done(true))),
    sleep(ms).then(() => false),
  ])
}

async function main([agent, out = mkdtempSync(join(tmpdir(), "nessa-mcp-live-"))]) {
  const command = agentCommand(agent)
  const evidence = join(resolve(out), agent)
  mkdirSync(evidence, { recursive: true })
  const recording = join(evidence, "acp.jsonl")
  writeFileSync(recording, "")
  // The MCP server's own traffic, through the same recorder: what the
  // harness asked the server for, beside what it then told Nessa.
  const mcpRecording = join(evidence, "mcp.jsonl")
  writeFileSync(mcpRecording, "")
  const step = (name, detail) => console.error(`[${agent}:${name}] ${detail}`)
  const log = []
  let directory = null
  let server = null
  let client = null
  let failed = false
  try {
    const nessa = join(root, "target/debug/nessa")
    if (!existsSync(nessa))
      throw new Error("build the gateway first: cargo build -p nessa-server")
    directory = mkdtempSync(join(tmpdir(), `nessa-mcp-live-${agent}-`))
    const workspace = join(directory, "workspace")
    mkdirSync(workspace)
    const port = Number(process.env.MCP_LIVE_PORT ?? 7431)
    const instance = `mcp-live-${agent}`
    const env = {
      ...process.env,
      NESSA_STAGE: "ci",
      NESSA_PORT: String(port),
      NESSA_DATA_DIR: directory,
      NESSA_INSTANCE: instance,
      ...(command.path ? { PATH: `${command.path}:${process.env.PATH}` } : {}),
    }
    const token = join(directory, "owner.token")
    const init = spawnSync(
      nessa,
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
          workspace,
          selected: agent,
          mcpServers: [
            {
              name: SERVER,
              command: process.execPath,
              args: [
                join(here, "acp-recorder.mjs"),
                mcpRecording,
                process.execPath,
                join(here, "server.mjs"),
              ],
            },
          ],
          runtimes: {
            [agent]: {
              command: process.execPath,
              args: [join(here, "acp-recorder.mjs"), recording, ...command.argv],
              model: command.model,
              toolsEnabled: true,
            },
          },
        },
      }),
    )
    chmodSync(configPath, 0o600)
    server = spawn(nessa, ["server"], { env, stdio: ["ignore", "pipe", "pipe"] })
    for (const stream of [server.stdout, server.stderr])
      stream.on("data", (chunk) => log.push(chunk.toString()))
    for (let i = 0; ; i += 1) {
      // A gateway that already exited is not the one answering on the port.
      if (server.exitCode !== null)
        throw new Error(
          `gateway exited (${server.exitCode}):\n${log.join("").slice(-4000)}`,
        )
      try {
        if ((await fetch(`http://127.0.0.1:${port}/health`)).ok) break
      } catch {}
      if (i > 240)
        throw new Error(`gateway never became healthy:\n${log.join("").slice(-4000)}`)
      await sleep(250)
    }
    const { NessaClient } = await import("@nessa/client")
    client = await NessaClient.connect({
      stage: "ci",
      url: `ws://127.0.0.1:${port}`,
      role: "surface",
      surface: { kind: "panel", instance: "mcp-live" },
      client: { id: "mcp-live", version: "0.1.0", platform: "node" },
      profile: "product",
      auth: { credential: readFileSync(token, "utf8").trim() },
    })
    const id = randomUUID()
    await client.conversation.create({ conversationId: id, agent })
    step("send", (await client.conversation.send(id, PROMPT)).disposition ?? "admitted")
    const answered = new Set()
    const reported = new Set()
    let view = null
    let outstanding = []
    const polls = Number(process.env.MCP_LIVE_POLLS ?? 300)
    for (let i = 0; i < polls; i += 1) {
      await sleep(1000)
      view = await client.conversation.read(id)
      // Allow each test-server tool once; say so, once, of anything else.
      const { allow, declined } = permissionDecisions(view, answered, SERVER)
      for (const { key, permission, option } of allow) {
        answered.add(key)
        step("allow", `${permission.toolName} → ${option.id}`)
        await client.conversation.answer(
          id,
          permission.executionId,
          permission.permissionId,
          option.id,
        )
      }
      outstanding = declined.map(
        ({ permission }) => `${permission.toolName} (tool ${permission.toolId})`,
      )
      for (const { key, permission } of declined) {
        if (reported.has(key)) continue
        reported.add(key)
        step("not allowed", `${permission.toolName} (tool ${permission.toolId})`)
      }
      const turn = view.messages.at(-1)
      if (turn && !["running", "queued"].includes(turn.status)) break
    }
    writeFileSync(join(evidence, "view.json"), JSON.stringify(view, null, 2))
    const records = parseRecording(readFileSync(recording, "utf8"))
    const summary = {
      agent,
      turn: view?.messages.at(-1)?.status ?? null,
      error: view?.messages.at(-1)?.error ?? null,
      outstandingPermissions: outstanding,
      frames: toolFrames(records).map(frameShape),
      viewTools: viewTools(view, SERVER),
      allTools: (view?.tools ?? []).map(({ title, kind, status, mcp }) => ({
        title,
        kind,
        status,
        mcp,
      })),
      uiMentions: uiMentions(records),
      mcpCalls: parseRecording(readFileSync(mcpRecording, "utf8"))
        .filter(({ direction, frame }) => direction === "to-agent" && frame?.method)
        .map(({ frame }) =>
          frame.method === "tools/call"
            ? `tools/call ${frame.params?.name}`
            : frame.method,
        ),
    }
    writeFileSync(join(evidence, "summary.json"), JSON.stringify(summary, null, 2))
    step("done", `turn ${summary.turn}; evidence in ${evidence}`)
    if (summary.turn !== "completed")
      throw new Error(
        `the turn did not complete: ${summary.turn} ${summary.error ?? ""}` +
          (outstanding.length ? `; waiting on ${outstanding.join(", ")}` : ""),
      )
  } catch (error) {
    failed = true
    console.error(`[${agent}:FAILED]`, error?.stack ?? error)
    console.error(log.join("").slice(-6000))
  } finally {
    client?.close()
    // Stop the gateway, and wait for it, before its directory is removed: it
    // stops the agent and the MCP server it started, and its last words go in
    // the log.
    if (server) {
      server.kill("SIGTERM")
      if (!(await exited(server, 10_000))) {
        server.kill("SIGKILL")
        if (!(await exited(server, 5_000))) {
          failed = true
          console.error(`[${agent}:FAILED] the gateway (pid ${server.pid}) did not exit`)
        }
      }
    }
    writeFileSync(join(evidence, "gateway.log"), log.join(""))
    if (directory) rmSync(directory, { recursive: true, force: true })
  }
  process.exit(failed ? 1 : 0)
}

if (process.argv[1] === fileURLToPath(import.meta.url)) await main(process.argv.slice(2))
