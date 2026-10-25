/**
 * How the gateway is asked to start the scripted agent: the recorded tool
 * stays the default, a scenario replaces it, and an evidence directory wraps
 * both processes with the recorder. No gateway is started.
 */
import { DatabaseSync } from "node:sqlite"
import { strict as assert } from "node:assert"
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { test } from "node:test"

import { TEXT_REPLY_SCENARIO } from "../../../../scripts/mcp-test-server/scenarios.mjs"
import { panelTarget, scriptedLaunch, webhookNoAttempt } from "./gateway-stack.mjs"

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

test("native no-send proof requires queued zero-attempt rows in the fixed complete scope", () => {
  const directory = mkdtempSync(join(tmpdir(), "webhook-native-proof-"))
  const scope = {
    receiver: "registration",
    origin: "origin",
    stream: "chat",
    incarnation: "J",
    schema: "records",
    accessEpoch: "epoch",
  }
  const saved = { ...scope, access_epoch: scope.accessEpoch }
  delete saved.accessEpoch
  const path = join(directory, "ci", "instances", "test", "auth")
  mkdirSync(path, { recursive: true })
  const db = new DatabaseSync(join(path, "webhooks.sqlite"))
  try {
    db.exec(
      "CREATE TABLE registrations(id TEXT,credential TEXT); CREATE TABLE deliveries(registration TEXT,conversation TEXT,position INTEGER,scope TEXT,state TEXT); INSERT INTO registrations VALUES('registration','credential')",
    )
    const insert = db.prepare(
      "INSERT INTO deliveries VALUES('registration','chat',2,?,?)",
    )
    const checkpoint = { scope, head: "2" }
    const probe = () => webhookNoAttempt({ directory }, "test", "credential", checkpoint)
    assert.throws(probe, /missing/)
    for (const state of [
      { phase: "sending", attempts: 1, timestamp_ms: 100 },
      { phase: "queued", attempts: "0", timestamp_ms: null },
      { phase: "queued", attempts: 0 },
    ]) {
      insert.run(JSON.stringify(saved), JSON.stringify(state))
      assert.throws(probe, /attempt admission/)
      db.exec("DELETE FROM deliveries")
    }
    insert.run(
      JSON.stringify({ ...saved, incarnation: "K" }),
      JSON.stringify({ phase: "queued", attempts: 0, timestamp_ms: null }),
    )
    assert.throws(probe, /different source scope/)
    db.exec("DELETE FROM deliveries")
    insert.run(
      JSON.stringify(saved),
      JSON.stringify({ phase: "queued", attempts: 0, timestamp_ms: null }),
    )
    assert.deepEqual(probe(), { queued: 1, attempted: 0 })
    assert.throws(
      () => webhookNoAttempt({ directory }, "test", "foreign", checkpoint),
      /missing/,
    )
    assert.throws(
      () => webhookNoAttempt({ directory }, "test", "credential", { scope, head: "3" }),
      /fixed Head/,
    )
    assert.equal(db.prepare("SELECT count(*) n FROM deliveries").get().n, 1)
  } finally {
    db.close()
    rmSync(directory, { recursive: true, force: true })
  }
})
