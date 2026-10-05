/**
 * How the gateway is asked to start the scripted agent: the recorded tool
 * stays the default, a scenario replaces it, and an evidence directory wraps
 * both processes with the recorder. No gateway is started.
 */
import { strict as assert } from "node:assert"
import { mkdtempSync, readFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { test } from "node:test"

import { TEXT_REPLY_SCENARIO } from "../../../../scripts/mcp-test-server/scenarios.mjs"
import { scriptedLaunch } from "./gateway-stack.mjs"

test("a tool launch is the recorded agent, and a scenario launch is not", () => {
  const tool = scriptedLaunch("claude", { tool: "review_rows" })
  assert.equal(tool.signedOut, true)
  assert.deepEqual(tool.command.argv.slice(-2), ["claude", "review_rows"])
  assert.equal(tool.command.argv.includes("--scenario"), false)
  assert.equal(tool.mcp.args.at(-1).endsWith("server.mjs"), true)
  const scenario = scriptedLaunch("claude", { scenario: TEXT_REPLY_SCENARIO })
  assert.equal(scenario.command.argv.at(-2), "--scenario")
  assert.equal(scenario.command.argv.at(-1), TEXT_REPLY_SCENARIO)
  assert.equal(scenario.command.argv.includes("review_rows"), false)
  assert.throws(() => scriptedLaunch("claude", {}))
})

test("evidence wraps the agent and the MCP server with the recorder", () => {
  const directory = mkdtempSync(join(tmpdir(), "scripted-launch-"))
  const launch = scriptedLaunch("codex", {
    scenario: TEXT_REPLY_SCENARIO,
    evidence: directory,
  })
  assert.equal(launch.command.argv[1].endsWith("acp-recorder.mjs"), true)
  assert.equal(launch.command.argv[2], join(directory, "acp.jsonl"))
  assert.equal(launch.command.argv.at(-2), "--scenario")
  assert.equal(launch.mcp.args[0].endsWith("acp-recorder.mjs"), true)
  assert.equal(launch.mcp.args[1], join(directory, "mcp.jsonl"))
  assert.equal(launch.mcp.args.at(-1).endsWith("server.mjs"), true)
  assert.equal(readFileSync(join(directory, "acp.jsonl"), "utf8"), "")
  assert.equal(readFileSync(join(directory, "mcp.jsonl"), "utf8"), "")
})
