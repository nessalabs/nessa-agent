/**
 * A client that can follow commits does not poll. The poller stays for a
 * client that cannot (`gateway-source.test.ts`).
 */
import { NessaRpcError, type ConnectionState, type ConversationView } from "@nessa/client"
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

function view(id: string): ConversationView {
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
    lifecycle: { phase: "attached" },
  }
}

it("does not list on the timer once the commit watch is held", async () => {
  const lists: unknown[] = []
  const reads: string[] = []
  let catalogueWatch: string | undefined
  const changed = new Set<(payload: { watchId: string }) => void>()
  const scope = {
    receiver: "receiver-1",
    origin: "origin",
    stream: "stream",
    incarnation: "inc",
    schema: "schema",
    accessEpoch: "epoch-1",
  }
  const client = {
    connectionState: { status: "connected" } as ConnectionState,
    onConnectionStateChange: () => () => {},
    close: () => {},
    on: (
      event: "conversation.changed" | "conversation.watchEnded",
      handler: (payload: { watchId: string }) => void,
    ) => {
      if (event === "conversation.changed") changed.add(handler)
      return () => changed.delete(handler)
    },
    conversation: {
      binding: async () => ({ receiverId: "receiver-1", accessEpoch: "1" }),
      list: async () => {
        lists.push(true)
        return {
          conversations: [
            {
              conversationId: "chat-a",
              title: "Alpha",
              preview: "Last said",
              createdAtMs: 1_000,
              updatedAtMs: 2_000,
              running: false,
              archived: false,
            },
          ],
          complete: true,
        }
      },
      observe: async () => ({ conversations: [], complete: true }),
      read: async (id: string) => {
        reads.push(id)
        return view(id)
      },
      create: async () => ({ conversationId: "chat-a" }),
      send: async () => ({ requestId: "r", executionId: "e", disposition: "queued" }),
      answer: async () => ({ requestId: "r", applied: true }),
      archive: async () => ({ requestId: "r", applied: true }),
    },
    records: {
      head: async () => ({ scope, head: "1" }),
    },
    catalogue: {
      head: async () => ({ scope, head: "1" }),
      manifest: async () => ({
        request: {
          maxEntries: 256,
          pass: { scope, completed: "0", boundary: "1", generation: "1" },
        },
        entries: [
          {
            key: { creation: "1", id: "chat-a" },
            revision: "1",
            deleted: false,
          },
        ],
        hasMore: false,
      }),
      resolve: async () => ({
        entry: { key: { creation: "1", id: "chat-a" }, revision: "1", deleted: false },
        payload: new TextEncoder().encode(
          JSON.stringify({
            id: "chat-a",
            createdAtMs: 1_000,
            agent: null,
            model: "claude",
            approvalMode: "ask",
            summary: {
              title: "Alpha",
              preview: "Last said",
              updatedAtMs: 2_000,
              archived: false,
            },
          }),
        ),
      }),
    },
    watches: {
      catalogue: async () => {
        catalogueWatch = "watch-catalogue"
        return { watchId: catalogueWatch }
      },
      records: async () => ({ watchId: "watch-chat-a" }),
      unwatch: async () => ({ watchId: "gone" }),
    },
  } satisfies GatewayClient
  const time = clock()
  const updates: { kind: string; title?: string }[] = []
  const source = gatewaySource({
    connect: async () => client,
    clock: time,
    timing,
  })
  source.subscribe((update) => {
    if (update.kind === "session")
      updates.push({ kind: update.kind, title: update.session.title })
  })
  await time.advance(500)
  for (let i = 0; i < 40; i++) await Promise.resolve()
  // Catch-up lists once for open rows and once for archived rows, then the timer stops.
  const seeded = lists.length
  expect(seeded).toBe(2)
  await time.advance(500)
  expect(lists).toHaveLength(seeded)
  expect(catalogueWatch).toBe("watch-catalogue")
  expect(updates.some((update) => update.title === "Alpha")).toBe(true)
  source.dispose?.()
})

function row(id: string, updatedAtMs: number) {
  return {
    conversationId: id,
    title: id,
    preview: null,
    createdAtMs: 1_000,
    updatedAtMs,
    running: false,
    archived: false,
  }
}

/** A gateway whose watches, lists, and connections the test can move. Each connect is its own socket. */
function syncGateway(ids: string[]) {
  const lists: string[] = []
  const reads: string[] = []
  const sockets: {
    changed: Set<(payload: { watchId: string }) => void>
    ended: Set<(payload: { watchId: string; reason?: string }) => void>
    states: Set<(state: ConnectionState) => void>
    closed: boolean
  }[] = []
  let catalogueHead = "1"
  let bindingError: unknown
  let live = false
  let catalogueWatch: string | undefined
  const scope = {
    receiver: "receiver-1",
    origin: "origin",
    stream: "stream",
    incarnation: "inc",
    schema: "schema",
    accessEpoch: "epoch-1",
  }
  const entries = () => ids.map((id, index) => row(id, 1_000 + index))
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
        binding: async () => {
          if (bindingError) throw bindingError
          return { receiverId: "receiver-1", accessEpoch: "1" }
        },
        list: async (options?: { archived?: boolean }) => {
          lists.push(options?.archived ? "archived" : "open")
          return {
            conversations: options?.archived ? [] : entries(),
            complete: true,
          }
        },
        observe: async () => ({ conversations: entries(), complete: true }),
        read: async (id: string) => {
          reads.push(id)
          const body = view(id)
          if (live)
            body.messages = [
              {
                executionId: "e",
                userText: "hi",
                attachments: [],
                files: [],
                status: "running",
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
      records: { head: async () => ({ scope, head: "1" }) },
      catalogue: {
        head: async () => ({ scope, head: catalogueHead }),
        manifest: async () => ({
          request: {
            maxEntries: 256,
            pass: { scope, completed: "0", boundary: catalogueHead, generation: "1" },
          },
          entries: ids.map((id) => ({
            key: { creation: "1", id },
            revision: catalogueHead,
            deleted: false,
          })),
          hasMore: false,
        }),
        resolve: async (params) => ({
          entry: { ...params.descriptor, revision: catalogueHead, deleted: false },
          payload: new TextEncoder().encode(
            JSON.stringify({
              id: params.descriptor.key.id,
              createdAtMs: 1_000,
              agent: null,
              model: "claude",
              approvalMode: "ask",
              summary: {
                title: params.descriptor.key.id,
                preview: null,
                updatedAtMs: Number(catalogueHead) * 1_000,
                archived: false,
              },
            }),
          ),
        }),
      },
      watches: {
        catalogue: async () => {
          catalogueWatch = "watch-catalogue"
          return { watchId: catalogueWatch }
        },
        records: async (params: { conversationId: string }) => ({
          watchId: `watch-${params.conversationId}`,
        }),
        unwatch: async () => ({ watchId: "gone" }),
      },
    }
  }
  return {
    open,
    lists,
    reads,
    sockets,
    set catalogueHead(value: string) {
      catalogueHead = value
    },
    failBinding(error: unknown) {
      bindingError = error
    },
    showRunning() {
      live = true
    },
    emitCatalogue() {
      if (!catalogueWatch) throw new Error("catalogue watch is not registered")
      for (const socket of sockets)
        for (const handler of [...socket.changed]) handler({ watchId: catalogueWatch })
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

it("reads a fourth open chat from the catalogue when only three record slots exist", async () => {
  const gateway = syncGateway(["chat-a", "chat-b", "chat-c", "chat-d"])
  const time = clock()
  const source = gatewaySource({
    connect: async () => gateway.open(),
    clock: time,
    timing,
  })
  source.subscribe(() => {})
  await time.advance(200)
  for (let i = 0; i < 30; i++) await Promise.resolve()
  for (const id of ["chat-a", "chat-b", "chat-c", "chat-d"]) await source.transcript(id)
  const before = gateway.reads.filter((id) => id === "chat-d").length
  gateway.catalogueHead = "2"
  gateway.emitCatalogue()
  for (let i = 0; i < 40; i++) await Promise.resolve()
  expect(gateway.reads.filter((id) => id === "chat-d").length).toBeGreaterThan(before)
  source.dispose?.()
})

it("resumes the list poller when the catalogue watch ends and the binding is gone", async () => {
  const gateway = syncGateway(["chat-a"])
  const time = clock()
  const source = gatewaySource({
    connect: async () => gateway.open(),
    clock: time,
    timing,
  })
  source.subscribe(() => {})
  await time.advance(200)
  for (let i = 0; i < 30; i++) await Promise.resolve()
  const seeded = gateway.lists.length
  expect(seeded).toBeGreaterThan(0)
  gateway.failBinding(new NessaRpcError("unauthorized", "unauthorized"))
  gateway.endCatalogue()
  await time.advance(timing.pollMs + 50)
  for (let i = 0; i < 40; i++) await Promise.resolve()
  expect(gateway.lists.length).toBeGreaterThan(seeded)
  source.dispose?.()
})

it("rejoins the watch after the command connection closes", async () => {
  const gateway = syncGateway(["chat-a"])
  const time = clock()
  const source = gatewaySource({
    connect: async () => gateway.open(),
    clock: time,
    timing,
  })
  source.subscribe(() => {})
  await time.advance(200)
  for (let i = 0; i < 30; i++) await Promise.resolve()
  const before = gateway.sockets.length
  const command = gateway.sockets[0]
  if (!command) throw new Error("no command socket")
  expect(command.closed).toBe(false)
  gateway.emitCommandClosed()
  for (let i = 0; i < 40; i++) await Promise.resolve()
  // A saved catalogue is rechecked, not listed again. The new sockets are the rejoin.
  expect(gateway.sockets.length).toBeGreaterThan(before)
  expect(gateway.lists).toHaveLength(2)
  expect(gateway.sockets.slice(before).some((socket) => !socket.closed)).toBe(true)
  source.dispose?.()
})

it("keeps reading a running chat on the fast timer while the watch is held", async () => {
  const gateway = syncGateway(["chat-a"])
  gateway.showRunning()
  const time = clock()
  const source = gatewaySource({
    connect: async () => gateway.open(),
    clock: time,
    timing,
  })
  source.subscribe(() => {})
  await time.advance(200)
  for (let i = 0; i < 30; i++) await Promise.resolve()
  await source.transcript("chat-a")
  const reads = gateway.reads.length
  await time.advance(timing.activePollMs + 20)
  for (let i = 0; i < 20; i++) await Promise.resolve()
  expect(gateway.reads.length).toBeGreaterThan(reads)
  source.dispose?.()
})
