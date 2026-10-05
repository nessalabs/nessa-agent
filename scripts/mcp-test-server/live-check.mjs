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
 * It asks for five of the test server's tools, each once (`toolPrompt`). It
 * allows each permission request for a tool of the test server once
 * (`permissionDecisions`), and leaves anything else the agent asks for
 * unanswered. The calls made are recorded in `summary.json` for review, not
 * checked: it exits non-zero unless the turn completed and `show_chart`
 * yielded a widget part in the transcript.
 *
 * The harness is given a stand-in (`nessa mcp-relay`) in the test server's
 * place; for each harness session that starts it, the gateway starts the
 * server and holds the connection to it (ADR 344), so `mcp.jsonl` is the
 * gateway's traffic with each server, the harness's forwarded calls among it.
 *
 * Writes `<out-dir>/<agent>/`: `acp.jsonl` and `mcp.jsonl` (every frame, both
 * directions), `view.json` (the final conversation view), `summary.json` (the
 * MCP tool frames' shapes, the view's MCP tools with their `resourceUri`, every
 * place a `ui://` resource appeared in ACP, the MCP calls made, the servers the
 * harness was given, and the widget parts the desktop transcript makes of the
 * view) and `gateway.log`. Review them before checking any of them in. The
 * gateway's own data directory is removed at the end.
 *
 * Environment: `MCP_LIVE_HARNESSES` — the directory holding `claude-acp/` and
 * `codex-acp/` with their `node_modules` (default: this checkout's
 * `crates/nessa-sdk/harnesses`); `MCP_LIVE_NESSA` — the gateway binary
 * (default: this checkout's `target/debug/nessa`); `MCP_LIVE_OPENCODE` — an Opencode 1.18.31
 * binary; `MCP_LIVE_PORT` (default 7431); `MCP_LIVE_POLLS` (seconds, default 300).
 */
import { randomUUID } from "node:crypto"
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { setTimeout as sleep } from "node:timers/promises"
import { fileURLToPath } from "node:url"
import {
  frameShape,
  givenServers,
  permissionDecisions,
  parseRecording,
  toolFrames,
  turnEnded,
  uiMentions,
  viewTools,
} from "./evidence.mjs"
import {
  SERVER,
  agentCommand,
  serverScript,
  startLocalGateway,
  toolPrompt,
} from "./local-gateway.mjs"

const here = dirname(fileURLToPath(import.meta.url))

/** What the model is asked to do: call these five tools once each, in order. */
const PROMPT = toolPrompt([
  { name: "report_rows" },
  { name: "link_resources" },
  { name: "rows.get", args: { id: 2 } },
  { name: "always_fails" },
  { name: "show_chart" },
])

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
  let gateway = null
  let client = null
  let failed = false
  // A failed start leaves no gateway, but its output on the error.
  let startError = null
  try {
    gateway = await startLocalGateway({
      agent,
      port: Number(process.env.MCP_LIVE_PORT ?? 7431),
      instance: `mcp-live-${agent}`,
      agentArgv: [
        process.execPath,
        join(here, "acp-recorder.mjs"),
        recording,
        ...command.argv,
      ],
      model: command.model,
      path: command.path,
      mcpServer: {
        command: process.execPath,
        args: [
          join(here, "acp-recorder.mjs"),
          mcpRecording,
          process.execPath,
          serverScript,
        ],
      },
    }).catch((error) => {
      startError = error
      throw error
    })
    const token = gateway.token
    const port = Number(new URL(gateway.url).port)
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
      if (turn && turnEnded(turn.status)) break
    }
    writeFileSync(join(evidence, "view.json"), JSON.stringify(view, null, 2))
    const records = parseRecording(readFileSync(recording, "utf8"))
    // What the desktop transcript makes of the view's tools: a widget part
    // for each call whose tool declared a UI the gateway read from the server.
    const { gatewayToolWidget } =
      await import("../../src/desktop/workspace/adapters/gateway/tool-widget.ts")
    const widgets = (view?.tools ?? []).flatMap((tool) => {
      const part = gatewayToolWidget(id, tool)
      return part ? [{ tool: tool.mcp, part }] : []
    })
    const summary = {
      agent,
      turn: view?.messages.at(-1)?.status ?? null,
      error: view?.messages.at(-1)?.error ?? null,
      outstandingPermissions: outstanding,
      // The stand-ins the harness was given in place of the configured server.
      givenServers: givenServers(records),
      widgets,
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
    if (
      !widgets.some(({ tool }) => tool?.server === SERVER && tool?.tool === "show_chart")
    )
      throw new Error("show_chart did not yield a widget part in the transcript")
  } catch (error) {
    failed = true
    console.error(`[${agent}:FAILED]`, error?.stack ?? error)
    console.error((gateway?.log() ?? error?.gatewayLog ?? "").slice(-6000))
  } finally {
    client?.close()
    if (gateway && !(await gateway.stop())) {
      failed = true
      console.error(
        `[${agent}:FAILED] the gateway (pid ${gateway.server.pid}) did not exit`,
      )
    }
    // Read after the stop, so it holds what the gateway said as it stopped.
    writeFileSync(
      join(evidence, "gateway.log"),
      gateway?.log() ?? startError?.gatewayLog ?? "",
    )
  }
  process.exit(failed ? 1 : 0)
}

if (process.argv[1] === fileURLToPath(import.meta.url)) await main(process.argv.slice(2))
