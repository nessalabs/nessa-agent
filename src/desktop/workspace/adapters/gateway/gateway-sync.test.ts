/**
 * A client that can follow commits does not poll the list. Each chat keeps
 * its own read timer unless it holds the one record watch and nothing unsaved
 * is pending. The poller stays for a client that cannot
 * (`gateway-source.test.ts`).
 */
import type { ConnectionState, ConversationView } from "@nessa/client"
import { expect, it } from "vitest"

import { gatewaySource, type GatewayClient, type GatewayClock } from "./gateway-source"

const timing = { callMs: 5_000, pollMs: 100, activePollMs: 100, reconnectRounds: 5 }

function clock(): GatewayClock & { advance(ms: number): Promise<void> } {
  let now = 0
  const timers: { at: number; run: () => void; cancelled: boolean }[] = []
  return {
    now: () => now,
    after(ms, run) {
      const timer = { at: now + ms, run, cancelled: false }
      timers.push(timer)
      return () => {
        timer.cancelled = true
      }
    },
    async advance(ms: number) {
      const until = now + ms
      for (;;) {
        const due = timers
          .filter((timer) => !timer.cancelled && timer.at <= until)
          .sort((left, right) => left.at - right.at)[0]
        if (!due) break
        now = due.at
        due.cancelled = true
        due.run()
        await Promise.resolve()
      }
      now = until
    },
  }
}

function view(id: string, phase: "attached" | "starting" = "attached"): ConversationView {
  return {
    conversationId: id,
    revision: "r1",
    approvalMode: "ask",
    approvalModes: [],
    title: null,
    queueComplete: true,
    transcriptState: "complete_empty",
    truncated: false,
    messages: [],
    pending: [],
    permissions: [],
    questions: [],
    tools: [],
    capabilities: {
      queue: true,
      steer: true,
      resume: true,
      permissions: true,
      imageInput: false,
      agentFeatures: {
        permissionDenial: "supported_for_offered_permission_reviews",
        nativeHookSuppression: "unknown",
        compactionReporting: "unsupported_not_implemented",
        modelSwitchReporting: "unsupported_not_implemented",
        permissionDeferral: "unsupported_not_implemented",
        elicitationForwarding: "unknown",
        preToolPolicy: "unknown",
        policyEndTurn: "unknown",
        policyCloseSession: "unknown",
        incomingElicitation: "unknown",
      },
    },
    lifecycle: { phase },
  }
}

function row(id: string, updatedAtMs: number, running = false) {
  return {
    conversationId: id,
    title: id,
    preview: null,
    createdAtMs: 1_000,
    updatedAtMs,
    running,
    archived: false,
  }
}

/** Each connect is its own socket. Watches are owner-session watches. */
function syncGateway(
  ids: string[],
  phase: "attached" | "starting" = "attached",
  status: "running" | "completed" = "running",
) {
  const lists: string[] = []
  const reads: string[] = []
  const recordWatches: string[] = []
  const sockets: {
    changed: Set<(payload: { watchId: string }) => void>
    ended: Set<(payload: { watchId: string; reason?: string }) => void>
    states: Set<(state: ConnectionState) => void>
    closed: boolean
  }[] = []
  let catalogueWatch: string | undefined
  let updatedAt = 1_000
  let rowRunning = true
  const open = (): GatewayClient => {
    const bucket = {
      changed: new Set<(payload: { watchId: string }) => void>(),
      ended: new Set<(payload: { watchId: string; reason?: string }) => void>(),
      states: new Set<(state: ConnectionState) => void>(),
      closed: false,
    }
    sockets.push(bucket)
    return {
      connectionState: { status: "connected" },
      onConnectionStateChange: (handler) => {
        bucket.states.add(handler)
        return () => bucket.states.delete(handler)
      },
      close: () => {
        bucket.closed = true
      },
      on: (event, handler) => {
        const set = event === "conversation.changed" ? bucket.changed : bucket.ended
        set.add(handler)
        return () => set.delete(handler)
      },
      conversation: {
        list: async (options?: { archived?: boolean }) => {
          lists.push(options?.archived ? "archived" : "open")
          return {
            conversations: options?.archived
              ? []
              : ids.map((id, index) => row(id, updatedAt + index, rowRunning)),
            complete: true,
          }
        },
        observe: async () => ({ conversations: [], complete: true }),
        read: async (id: string) => {
          reads.push(id)
          const body = view(id, phase)
          body.messages = [
            {
              executionId: "e",
              userText: "hi",
              attachments: [],
              files: [],
              status,
              parts: [],
            },
          ]
          return body
        },
        create: async () => ({ conversationId: ids[0] ?? "chat" }),
        send: async () => ({
          requestId: "r",
          executionId: "e",
          disposition: "queued" as const,
        }),
        answer: async () => ({ requestId: "r", applied: true }),
        archive: async () => ({ requestId: "r", applied: true }),
      },
      watches: {
        ownedCatalogue: async () => {
          catalogueWatch = "watch-catalogue"
          return { watchId: catalogueWatch }
        },
        ownedRecords: async (conversationId: string) => {
          recordWatches.push(conversationId)
          return { watchId: `watch-${conversationId}` }
        },
        unwatch: async (watchId: string) => ({ watchId }),
      },
    }
  }
  return {
    open,
    lists,
    reads,
    recordWatches,
    sockets,
    emitCatalogue() {
      if (!catalogueWatch) throw new Error("catalogue watch is not registered")
      for (const socket of sockets)
        for (const handler of [...socket.changed]) handler({ watchId: catalogueWatch })
    },
    /** The next list says this chat moved. `running` is the list row's flag. */
    move(at: number, running = true) {
      updatedAt = at
      rowRunning = running
    },
    emitRecord(id: string) {
      for (const socket of sockets)
        for (const handler of [...socket.changed]) handler({ watchId: `watch-${id}` })
    },
    endCatalogue() {
      if (!catalogueWatch) throw new Error("catalogue watch is not registered")
      for (const socket of sockets)
        for (const handler of [...socket.ended])
          handler({ watchId: catalogueWatch, reason: "closed" })
    },
    emitCommandClosed() {
      const command = sockets[0]
      if (!command) throw new Error("no command socket")
      for (const handler of [...command.states])
        handler({ status: "closed", error: new Error("closed") })
    },
  }
}

async function settle() {
  for (let i = 0; i < 40; i++) await Promise.resolve()
}

it("does not list on the timer once the list watch is held", async () => {
  const gateway = syncGateway(["chat-a"])
  const time = clock()
  const source = gatewaySource({
    connect: async () => gateway.open(),
    clock: time,
    timing,
  })
  source.subscribe(() => {})
  await time.advance(500)
  await settle()
  const seeded = gateway.lists.length
  expect(seeded).toBe(1)
  await time.advance(500)
  expect(gateway.lists).toHaveLength(seeded)
  gateway.emitCatalogue()
  await settle()
  expect(gateway.lists.length).toBeGreaterThan(seeded)
  source.dispose?.()
})

it("reads a settled open chat when the list ping says its row moved", async () => {
  const gateway = syncGateway(["chat-a"], "attached", "completed")
  gateway.move(1_000, false)
  const time = clock()
  const source = gatewaySource({
    connect: async () => gateway.open(),
    clock: time,
    timing,
  })
  source.subscribe(() => {})
  await time.advance(200)
  await settle()
  await source.transcript("chat-a")
  await settle()
  expect(gateway.recordWatches).toEqual([])
  const before = gateway.reads.filter((id) => id === "chat-a").length
  await time.advance(timing.activePollMs + timing.pollMs)
  expect(gateway.reads.filter((id) => id === "chat-a").length).toBe(before)
  gateway.move(2_000, false)
  gateway.emitCatalogue()
  await settle()
  expect(gateway.reads.filter((id) => id === "chat-a").length).toBeGreaterThan(before)
  source.dispose?.()
})

it("keeps polling a running turn and does not give it the record slot", async () => {
  const gateway = syncGateway(["chat-a", "chat-b", "chat-c", "chat-d"])
  const time = clock()
  const source = gatewaySource({
    connect: async () => gateway.open(),
    clock: time,
    timing,
  })
  source.subscribe(() => {})
  await time.advance(200)
  await settle()
  for (const id of ["chat-a", "chat-b", "chat-c", "chat-d"]) await source.transcript(id)
  await settle()
  expect(gateway.recordWatches).toEqual([])
  const before = gateway.reads.filter((id) => id === "chat-a").length
  await time.advance(timing.activePollMs + 20)
  await settle()
  expect(gateway.reads.filter((id) => id === "chat-a").length).toBeGreaterThan(before)
  source.dispose?.()
})

it("reads the watched chat when its record ping arrives", async () => {
  const gateway = syncGateway(["chat-a"], "attached", "completed")
  const time = clock()
  const source = gatewaySource({
    connect: async () => gateway.open(),
    clock: time,
    timing,
  })
  source.subscribe(() => {})
  await time.advance(200)
  await settle()
  await source.transcript("chat-a")
  await settle()
  const before = gateway.reads.filter((id) => id === "chat-a").length
  gateway.emitRecord("chat-a")
  await settle()
  expect(gateway.reads.filter((id) => id === "chat-a").length).toBeGreaterThan(before)
  source.dispose?.()
})

it("resumes the list timer when the catalogue watch ends", async () => {
  const gateway = syncGateway(["chat-a"])
  const time = clock()
  const source = gatewaySource({
    connect: async () => gateway.open(),
    clock: time,
    timing,
  })
  source.subscribe(() => {})
  await time.advance(200)
  await settle()
  const seeded = gateway.lists.length
  expect(seeded).toBeGreaterThan(0)
  gateway.endCatalogue()
  await time.advance(timing.pollMs + 50)
  await settle()
  expect(gateway.lists.length).toBeGreaterThan(seeded)
  source.dispose?.()
})

it("leaves the watch connection up when the command connection closes", async () => {
  const gateway = syncGateway(["chat-a"])
  const time = clock()
  const source = gatewaySource({
    connect: async () => gateway.open(),
    clock: time,
    timing,
  })
  source.subscribe(() => {})
  await time.advance(200)
  await settle()
  const before = gateway.sockets.length
  const follow = gateway.sockets[before - 1]
  gateway.emitCommandClosed()
  await settle()
  expect(gateway.sockets).toHaveLength(before)
  expect(follow?.closed).toBe(false)
  source.dispose?.()
})

it("keeps polling a chat whose provider is still starting", async () => {
  const gateway = syncGateway(["chat-a"], "starting")
  const time = clock()
  const source = gatewaySource({
    connect: async () => gateway.open(),
    clock: time,
    timing,
  })
  source.subscribe(() => {})
  await time.advance(200)
  await settle()
  await source.transcript("chat-a")
  await settle()
  expect(gateway.recordWatches).toEqual([])
  const reads = gateway.reads.length
  await time.advance(timing.activePollMs + 20)
  await settle()
  expect(gateway.reads.length).toBeGreaterThan(reads)
  source.dispose?.()
})
