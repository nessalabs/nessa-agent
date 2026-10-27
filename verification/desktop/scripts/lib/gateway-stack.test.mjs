/**
 * How the gateway is asked to start the scripted agent: the recorded tool
 * stays the default, a scenario replaces it, and an evidence directory wraps
 * both processes with the recorder. The failed-start retention check also
 * starts a disposable stand-in gateway.
 */
import { strict as assert } from "node:assert"
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { test } from "node:test"

import { TEXT_REPLY_SCENARIO } from "../../../../scripts/mcp-test-server/scenarios.mjs"
import { panelTarget, scriptedLaunch, startGatewayStack } from "./gateway-stack.mjs"

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

/**
 * A stack whose only live resource is `directory`. `close` counts as done
 * only after a turn of the event loop: calling it without awaiting returns
 * while `closed()` is still false, which is the leak.
 */
function credentialStack(directory) {
  let closed = false
  return {
    stack: {
      gateway: { directory },
      close: async () => {
        await new Promise((resolve) => setTimeout(resolve, 30))
        closed = true
      },
    },
    closed: () => closed,
  }
}

test("a missing panel credential closes the stack before it escapes", async () => {
  const directory = mkdtempSync(join(tmpdir(), "panel-target-"))
  const { stack, closed } = credentialStack(directory)
  await assert.rejects(() => panelTarget(stack), /panel credentials/)
  assert.equal(closed(), true)
  rmSync(directory, { recursive: true, force: true })
})

test("a panel credential is returned and the stack stays open", async () => {
  const directory = mkdtempSync(join(tmpdir(), "panel-target-"))
  mkdirSync(join(directory, "auth", "surfaces"), { recursive: true })
  writeFileSync(join(directory, "auth", "surfaces", "nessa-panel.token"), "secret\n")
  const { stack, closed } = credentialStack(directory)
  const target = await panelTarget(stack, () => ({ endpoint: "ws://127.0.0.1:1" }))
  assert.equal(target.credential, "secret")
  assert.equal(target.endpoint, "ws://127.0.0.1:1")
  assert.equal(target.gateway, stack.gateway)
  assert.equal(closed(), false)
  const plain = await panelTarget(stack, { marker: "kept" })
  assert.equal(plain.credential, "secret")
  assert.equal(plain.marker, "kept")
  assert.equal(closed(), false)
  rmSync(directory, { recursive: true, force: true })
})

test("a throw while building the target closes the stack", async () => {
  const directory = mkdtempSync(join(tmpdir(), "panel-target-"))
  mkdirSync(join(directory, "auth", "surfaces"), { recursive: true })
  writeFileSync(join(directory, "auth", "surfaces", "nessa-panel.token"), "secret\n")
  const { stack, closed } = credentialStack(directory)
  await assert.rejects(
    () =>
      panelTarget(stack, () => {
        throw new Error("endpoint")
      }),
    /endpoint/,
  )
  assert.equal(closed(), true)
  rmSync(directory, { recursive: true, force: true })
})

test("the stack preserves startup and retention causes instead of rewriting failed evidence", async (t) => {
  const scratch = mkdtempSync(join(tmpdir(), "nessa-stack-retention-test-"))
  const fake = join(scratch, "nessa")
  const evidence = join(scratch, "evidence")
  const owned = join(scratch, "owned-directory.txt")
  const previous = process.env.MCP_LIVE_NESSA
  t.after(() => {
    if (previous === undefined) delete process.env.MCP_LIVE_NESSA
    else process.env.MCP_LIVE_NESSA = previous
    rmSync(scratch, { recursive: true, force: true })
  })
  writeFileSync(
    fake,
    `#!${process.execPath}
const {mkdirSync, rmSync, writeFileSync} = require("node:fs")
const {join} = require("node:path")
if (process.argv[2] === "auth") {
  mkdirSync(join(process.env.NESSA_DATA_DIR, "ci", "instances", process.env.NESSA_INSTANCE), {recursive: true})
  writeFileSync(${JSON.stringify(owned)}, process.env.NESSA_DATA_DIR)
  rmSync(${JSON.stringify(evidence)}, {recursive: true, force: true})
  writeFileSync(${JSON.stringify(evidence)}, "occupied")
  process.exit(0)
}
process.stderr.write("original-start-failure\\n", () => process.exit(3))
`,
  )
  chmodSync(fake, 0o755)
  process.env.MCP_LIVE_NESSA = fake
  await assert.rejects(
    startGatewayStack({ agent: "claude" }, "stack-retention-test", {
      scripted: "text-reply",
      evidence,
    }),
    (error) => {
      assert.equal(error.name, "CannotRun")
      assert.ok(error.cause instanceof AggregateError)
      assert.equal(error.cause.errors.length, 2)
      assert.equal(error.cause.errors[1].code, "EEXIST")
      assert.equal(error.cause.gatewayLog, "original-start-failure\n")
      assert.match(error.cause.errors[0].message, /gateway|exit|start/)
      return true
    },
  )
  assert.equal(existsSync(readFileSync(owned, "utf8")), false)
})
