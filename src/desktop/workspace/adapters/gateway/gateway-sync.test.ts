/**
 * A client that can follow commits does not poll. The poller stays for a
 * client that cannot (`gateway-source.test.ts`).
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
        return { conversations: [], complete: true }
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
  expect(lists).toEqual([])
  expect(catalogueWatch).toBe("watch-catalogue")
  expect(updates.some((update) => update.title === "Alpha")).toBe(true)
  source.dispose?.()
})
