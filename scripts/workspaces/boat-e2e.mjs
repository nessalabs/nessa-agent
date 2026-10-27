#!/usr/bin/env node
import { settledMessage } from "./evidence.mjs"
/** Opt-in, paid-provider check. Uses an isolated gateway and disposable VMs.
 * The token is read from a private file, never argv, environment or evidence.
 * See README.md before running; Boat must already have a connected subscription. */
import assert from "node:assert/strict"
import { randomUUID } from "node:crypto"
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs"
import { tmpdir, platform } from "node:os"
import { join, resolve } from "node:path"
import { setTimeout as sleep } from "node:timers/promises"
import { parseArgs } from "node:util"
import { startLocalGateway, repoRoot } from "../mcp-test-server/local-gateway.mjs"
import {
  connectClient,
  scriptedLaunch,
} from "../../verification/desktop/scripts/lib/gateway-stack.mjs"
import { freePort } from "../../verification/desktop/scripts/lib/server.mjs"
const { values } = parseArgs({
  options: { "token-file": { type: "string" }, evidence: { type: "string" } },
})
assert(
  values["token-file"] && values.evidence,
  "Pass --token-file <private token file> --evidence <directory>",
)
const token = readFileSync(values["token-file"], "utf8").trim()
assert(token.length > 0, "Token file is empty")
const evidence = resolve(values.evidence)
mkdirSync(evidence, { recursive: true, mode: 0o700 })
const local = mkdtempSync(join(tmpdir(), "nessa-boat-files-"))
const marker = `NESSA_LOCAL_TEST_${randomUUID()}`
writeFileSync(join(local, "error.log"), marker, { mode: 0o600 })
const passphrase = `Disposable boat test ${randomUUID()}`
const instance = `boat-${randomUUID()}`
const command = scriptedLaunch("claude", { tool: "read_only" }).command
let gateway, client
const conversations = new Set(),
  owned = new Map(),
  checks = [],
  cleanup = []
const record = (name, data = {}) => {
  checks.push({ name, ...data })
  writeFileSync(join(evidence, "checks.json"), JSON.stringify(checks, null, 2))
  console.log(name)
}
async function boat(path, method = "GET") {
  const response = await fetch(`https://boat.dev/api/v1${path}`, {
    method,
    headers: {
      Authorization: `Bearer ${token}`,
      ...(method === "DELETE"
        ? { "X-Ascii-Confirm-Delete": path.split("/").at(-1) }
        : {}),
    },
    signal: AbortSignal.timeout(45_000),
  })
  if (response.status === 404) return null
  assert(response.ok, `Boat ${method} failed (${response.status})`)
  return response.json()
}
async function until(check, label, ms = 180_000) {
  const end = Date.now() + ms
  while (Date.now() < end) {
    const value = await check()
    if (value) return value
    await sleep(500)
  }
  throw new Error(`Timed out: ${label}`)
}
function assignments() {
  if (!gateway) return
  const root = join(gateway.directory, "ci", "instances", instance, "boat-workspaces")
  let files
  try {
    files = readdirSync(root)
  } catch (error) {
    if (error.code === "ENOENT") return
    throw error
  }
  for (const file of files) {
    const history = JSON.parse(readFileSync(join(root, file), "utf8"))
    for (const assignment of history.transitions ?? [])
      if (assignment.state === "assigned") owned.set(assignment.vm, assignment.thread)
  }
  writeFileSync(
    join(evidence, "owned-vms.json"),
    JSON.stringify(
      [...owned].map(([vm, thread]) => ({ vm, thread })),
      null,
      2,
    ),
  )
}
async function turn(id, text, permission = "none") {
  const approved = new Set()
  const executionId = randomUUID()
  const submission = await client.sendMessage(id, text, { requestId: executionId })
  assert.equal(
    submission.target.executionId,
    executionId,
    "Turn admission changed its owned execution identity",
  )
  return until(async () => {
    const view = await client.conversation.read(id)
    for (const entry of view.permissions) {
      const review = await client.conversation.permissionReview(id, entry.review)
      if (review.executionId !== executionId || approved.has(review.review)) continue
      assert(
        (permission === "log" && review.toolName.includes("read_local_log")) ||
          (permission === "delegate" &&
            ["create_thread", "send_message"].some(
              (name) =>
                review.toolName.includes("nessa-agents") &&
                review.toolName.endsWith(name),
            )),
        "Unexpected approval; no broad permission is granted",
      )
      const option = review.options.find((value) => value.effect === "allow")
      const { permissionTargets } = await import("@nessa/client")
      const target = permissionTargets({ conversationId: id, permissions: [review] })[0]
      assert(option && target, "No offered one-time allow")
      await client.conversation.answer(target, option.id)
      approved.add(review.review)
    }
    const message = settledMessage(view, executionId)
    if (!message || !["completed", "failed", "cancelled"].includes(message.status))
      return null
    if (message.status !== "completed") {
      writeFileSync(
        join(evidence, "failed-turn.json"),
        JSON.stringify(
          {
            conversationId: id,
            status: message.status,
            error: message.error,
            tools: view.tools
              .filter((tool) => tool.executionId === message.executionId)
              .map((tool) => ({ title: tool.title, status: tool.status })),
          },
          null,
          2,
        ),
      )
    }
    assert.equal(message.status, "completed", "Provider turn failed")
    return {
      reply: message.parts
        .filter((part) => part.kind === "text")
        .map((part) => part.text)
        .join(""),
      tools: view.tools.filter((tool) => tool.executionId === message.executionId),
      approved: approved.size,
    }
  }, "provider turn")
}
async function open(channel, id, approvalMode = "ask") {
  const thread = await client.createThread({
    channelId: channel.channelId,
    ...(id ? { conversationId: id } : {}),
    agent: "claude",
    model: "claude-sonnet-5",
    approvalMode,
  })
  conversations.add(thread.conversationId)
  assignments()
  return thread.conversationId
}
try {
  gateway = await startLocalGateway({
    agent: "claude",
    port: await freePort(),
    instance,
    agentArgv: command.argv,
    stagedRuntime: command.stagedRuntime,
    semanticArgs: command.semanticArgs,
    model: command.model,
    signedOut: true,
    mcpServers: [],
  })
  // Use the real panel grants, including credential administration.
  client = await connectClient(gateway, instance, "panel")
  await client.vault.setup(passphrase)
  await client.boat.save({ token })
  assert.equal((await client.boat.status()).state, "connected")
  record("token validated and encrypted")
  const localChannel = await client.channels.create({
    requestId: randomUUID(),
    name: "Local fixture",
    agent: "claude",
    model: "claude-sonnet-5",
  })
  await open(localChannel)
  assert.equal(owned.size, 0, "Local allocated a Boat VM")
  record("Local creates no cloud VM")
  const channel = await client.channels.create({
    requestId: randomUUID(),
    name: "Boat E2E",
    agent: "claude",
    model: "claude-sonnet-5",
    coordinatorPlacement: "boat",
    threadPlacement: "boat",
  })
  assert(channel.coordinatorConversationId)
  const main = await open(channel, channel.coordinatorConversationId)
  const worker = await open(channel, undefined, "full")
  assert.equal(owned.size, 2, "Distinct threads did not receive distinct VMs")
  const workerVm = [...owned].find(([, thread]) => thread === worker)?.[0]
  assert(workerVm, "Worker allocation receipt missing")
  record("two threads use distinct saved VMs", { main, worker, workerVm })
  const fileMarker = `NESSA_WORKSPACE_TEST_${randomUUID()}`
  const wrote = await turn(
    worker,
    `Write the exact text ${fileMarker} to alpha-test.txt in your workspace, then read it back and reply with it. Use only your native file tools; no MCP tools or subagents.`,
  )
  assert(wrote.reply.includes(fileMarker))
  record("subscription inference writes workspace file")
  const before = owned.size
  await client.conversation.close(worker)
  await boat(`/sandboxes/${workerVm}/stop`, "POST")
  await until(
    async () => (await boat(`/sandboxes/${workerVm}`))?.sandbox.state === "archived",
    "VM archived",
  )
  // Restart the gateway to exercise durable receipts and a fresh connection,
  // rather than expecting a disconnected socket to detect loss synchronously.
  client.close()
  await gateway.restart("SIGTERM")
  client = await connectClient(gateway, instance, "panel")
  // Vaults deliberately seal on session loss. Reuse only this test's passphrase.
  await client.vault.unlock(passphrase)
  await open(channel, worker, "full")
  assert.equal(owned.size, before, "Reopen allocated a replacement VM")
  const resumed = await turn(
    worker,
    "Read alpha-test.txt in your workspace and return its contents. Do not change it or use MCP tools.",
  )
  assert(resumed.reply.includes(fileMarker))
  assert(
    resumed.tools.some((tool) => tool.kind === "read" && tool.status === "completed"),
    "Resumed turn did not actually read its workspace file",
  )
  record("restart and VM resume retain identity and workspace files")
  await open(channel, main)
  const warmed = await turn(
    main,
    "Reply NESSA_MAIN_READY. Do not use tools or create threads.",
  )
  assert(warmed.reply.includes("NESSA_MAIN_READY"))
  record("cold main subscription starts after gateway restart")
  const settings = await client.mcpServers.list()
  await client.mcpServers.save({
    revision: settings.revision,
    server: {
      kind: "stdio",
      name: "boat-local-files",
      command: process.execPath,
      args: [join(repoRoot, "scripts/mcp-test-server/boat-local-files.mjs"), local],
      env: [],
      enabled: true,
    },
  })
  const reader = await open(channel)
  const read = await turn(
    reader,
    'Call read_local_log from the boat-local-files MCP server exactly once with {"name":"error.log"}. Return its actual text and host. Read no other files and use no other MCP tools.',
    "log",
  )
  assert(read.reply.includes(marker), "Cloud harness did not receive local fixture log")
  assert(
    read.tools.some(
      (tool) =>
        tool.status === "completed" &&
        typeof tool.structuredContent === "string" &&
        JSON.parse(tool.structuredContent).host === platform(),
    ),
    "No local-host MCP result",
  )
  assert(read.approved > 0, "Local fixture read bypassed the expected approval")
  record("cloud subscription reads approved local log through existing MCP", { reader })
  const current = await client.mcpServers.list()
  await client.mcpServers.remove({ revision: current.revision, name: "boat-local-files" })
  assert(
    !(await client.mcpServers.list()).servers.some(
      (server) => server.name === "boat-local-files",
    ),
    "Removed server remained configured",
  )
  // Configuration replacement intentionally preserves already-open MCP sessions.
  // Closing revokes their per-open grants; reopening uses the new server set.
  await client.conversation.close(reader)
  await open(channel, reader)
  const secretAfterRevocation = `REVOKED_${randomUUID()}`
  writeFileSync(join(local, "error.log"), secretAfterRevocation)
  const revoked = await turn(
    reader,
    "Read error.log again using the same boat-local-files tool. If the connection is unavailable, say so. Do not use other tools.",
    "log",
  )
  assert(
    !revoked.reply.includes(secretAfterRevocation),
    "Revoked connection exposed new content",
  )
  assert(
    !revoked.tools.some(
      (tool) => tool.title.includes("read_local_log") && tool.status === "completed",
    ),
    "Revoked tool still completed",
  )
  record("removal plus session close revokes the old local connection")
  await open(channel, main)
  const delegated = await turn(
    main,
    "Use the nessa-agents MCP tools to create exactly one new thread in this channel, then send that thread a message with mode queued asking it to reply NESSA_DELEGATED_OK without tools. Use create_thread and send_message, never your native subagent tool. Return the real thread ID. Do not create another thread if a call fails.",
    "delegate",
  )
  // Tool titles are provider presentation, not the delegation contract.
  // The gateway's allocation receipt plus the child's own completed message
  // establish actual Nessa creation/dispatch, rather than a model's claim.
  writeFileSync(
    join(evidence, "coordinator-tools.json"),
    JSON.stringify(
      delegated.tools.map((tool) => ({
        title: tool.title,
        status: tool.status,
        input: tool.input,
        details: tool.details,
        structuredContent: tool.structuredContent,
      })),
      null,
      2,
    ),
  )
  assignments()
  assert.equal(owned.size, 4, "Delegation allocated an unexpected number of VMs")
  const child = [...owned].find(([, thread]) => !conversations.has(thread))
  assert(child, "Delegation did not allocate a distinct VM")
  conversations.add(child[1])
  await until(async () => {
    const view = await client.conversation.read(child[1])
    writeFileSync(
      join(evidence, "child-state.json"),
      JSON.stringify(
        { messages: view.messages, pending: view.pending, permissions: view.permissions },
        null,
        2,
      ),
      { mode: 0o600 },
    )
    assert(
      !view.permissions.length,
      "Delegated reply unexpectedly requires tool approval",
    )
    assert(
      !view.messages.some((message) => ["failed", "cancelled"].includes(message.status)),
      "Delegated worker failed before replying; see child-state.json",
    )
    return view.messages.some(
      (message) =>
        message.status === "completed" &&
        message.parts.some(
          (part) => part.kind === "text" && part.text.includes("NESSA_DELEGATED_OK"),
        ),
    )
  }, "delegated worker reply")
  record("coordinator dispatches real child on another VM", {
    thread: child[1],
    vm: child[0],
  })
  await client.conversation.close(worker)
  const deleted = await boat(`/sandboxes/${workerVm}`, "DELETE")
  await until(
    async () => (await boat(`/sandboxes/${workerVm}`)) === null,
    "deleted worker absent",
  )
  let missingRefused = false
  try {
    await open(channel, worker, "full")
  } catch (error) {
    missingRefused = error.code === "environment_unavailable"
  }
  assert(missingRefused, "Missing VM was not refused with its typed environment failure")
  assignments()
  assert.equal(owned.size, 4, "Missing VM silently allocated a replacement")
  record("missing VM fails without replacement or Local fallback", {
    vm: workerVm,
    operation: deleted?.operation?.id,
  })
  record("PASS")
} finally {
  try {
    assignments()
  } catch {
    cleanup.push({ result: "assignment ledger could not be read" })
  }
  if (client) {
    for (const id of conversations) {
      try {
        await client.conversation.close(id)
      } catch {
        cleanup.push({ thread: id, result: "close unconfirmed" })
      }
    }
    client.close()
  }
  // IDs come exclusively from this fresh gateway's own allocation receipts.
  for (const [vm, thread] of owned) {
    const entry = { vm, thread, state: "deletion requested" }
    cleanup.push(entry)
    try {
      const receipt = await boat(`/sandboxes/${vm}`, "DELETE")
      entry.operation = receipt?.operation?.id
      await until(
        async () => (await boat(`/sandboxes/${vm}`)) === null,
        "VM deletion confirmed",
      )
      entry.state = "confirmed absent"
    } catch {
      entry.state = "cleanup unconfirmed; use Boat dashboard"
      process.exitCode = 1
    }
    writeFileSync(join(evidence, "cleanup.json"), JSON.stringify(cleanup, null, 2))
  }
  if (gateway) {
    writeFileSync(
      join(evidence, "gateway.log"),
      gateway.log().replaceAll(token, "[redacted]"),
      { mode: 0o600 },
    )
    await gateway.stop()
  }
  rmSync(local, { recursive: true, force: true })
}
