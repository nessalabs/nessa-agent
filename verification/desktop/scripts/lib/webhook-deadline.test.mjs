import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import { test } from "node:test"
import { runInNewContext } from "node:vm"
import { webhookNoAttempt } from "./gateway-stack.mjs"

test("cursor catch-up cannot renew the final webhook evidence deadline", async () => {
  // Exercise the actual case without starting its CLI, browser or gateway.
  const source = readFileSync(
    new URL("../conversation-stress.mjs", import.meta.url),
    "utf8",
  )
  const start = source.indexOf("  async webhook(")
  const end = source.indexOf("  async aside(", start)
  assert.ok(start >= 0 && end > start)
  let elapsed = 100
  let waits = 0
  let heads = 0
  let unshared = 0
  let closed = 0
  const scope = {
    receiver: "registration",
    origin: "origin",
    stream: "chat",
    incarnation: "J",
    schema: "records",
    accessEpoch: "epoch",
  }
  const owner = {
    conversation: {
      share: async () => {},
      unshare: async () => {
        unshared += 1
      },
      webhookCursor: async (_chat, _credential, options) =>
        options?.includeSourceHead
          ? { observedSourceHead: { scope, head: String(++heads) } }
          : { scope, position: "2", resetRequired: false },
    },
    close: () => {
      closed += 1
    },
  }
  const now = Date.now
  Date.now = () => elapsed
  try {
    const step = runInNewContext(`({${source.slice(start, end)}})`, {
      Date,
      INSTANCE: "test",
      randomUUID: () => "chat",
      connectClient: async () => owner,
      prompt: () => "synthetic",
      STRESS: {},
      SAYS: { webhook: "reply" },
      settled: async () => {},
      titleOf: async () => "chat",
      openStressConversation: async () => {},
      send: async () => new Set(),
      expectShown: async () => {},
      webhookNoAttempt,
      log: () => assert.fail("expired evidence cannot be logged as proof"),
      waitFor: async (check, budget) => {
        const ready = await check()
        if (++waits === 3) elapsed += budget
        return ready
      },
    }).webhook
    const run = {
      stack: {
        webhookCredential: "credential",
        gateway: { directory: "missing-fixture" },
      },
      client: {
        conversation: {
          create: async () => {},
          prepare: () => ({}),
          submit: async () => {},
        },
      },
      checked: [],
      main: { id: "main", title: "main" },
    }
    await assert.rejects(step(run, {}, []), /no-send evidence deadline expired/)
    assert.equal(waits, 3)
    assert.equal(unshared, 1)
    assert.equal(closed, 1)
  } finally {
    Date.now = now
  }
})
