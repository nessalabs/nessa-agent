// @vitest-environment jsdom
import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import {
  NessaConnectionClosedError,
  NessaRpcError,
  type ConnectionState,
  type NessaClient,
} from "@nessa/client"
import { afterEach, beforeEach, expect, it, vi } from "vitest"

import { createDependencies } from "../../composition/dependencies"
import { sessionReady } from "../../session/testing"
import { makeStore } from "../../store"
import { recordFollowSet } from "../adapters/store/history"
import { openListed, refreshConversation } from "../adapters/store/slice"
import { scenarioEffects } from "../testing"
import { useConversation } from "./use-conversation"
import { ConversationFollow } from "./commit-follow"

const written = "0b8f1c2e-1111-4a4a-8b8b-000000000001"

let container: HTMLDivElement
let root: Root

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  container = document.createElement("div")
  document.body.appendChild(container)
  root = createRoot(container)
})

afterEach(async () => {
  await React.act(async () => root.unmount())
  container.remove()
})

function followClient() {
  const changed = new Set<(payload: { watchId: string; reason?: string }) => void>()
  const ended = new Set<(payload: { watchId: string; reason?: string }) => void>()
  const states = new Set<(state: ConnectionState) => void>()
  const calls: string[] = []
  let catalogueError: string | undefined
  const client = {
    watches: {
      ownedCatalogue: async () => {
        calls.push("catalogue")
        if (catalogueError) throw new NessaRpcError(catalogueError, catalogueError)
        return { watchId: "catalogue-watch" }
      },
      ownedRecords: async (conversationId: string) => {
        calls.push(`records:${conversationId}`)
        return { watchId: `record-${conversationId}` }
      },
      unwatch: async (watchId: string) => {
        calls.push(`unwatch:${watchId}`)
        return { watchId }
      },
    },
    on: (
      event: "conversation.changed" | "conversation.watchEnded",
      handler: (payload: { watchId: string; reason?: string }) => void,
    ) => {
      const set = event === "conversation.changed" ? changed : ended
      set.add(handler)
      return () => set.delete(handler)
    },
    onConnectionStateChange: (handler: (state: ConnectionState) => void) => {
      states.add(handler)
      return () => states.delete(handler)
    },
    close: () => {
      calls.push("close")
    },
  }
  return {
    client: client as unknown as NessaClient,
    calls,
    refuse(code: string) {
      catalogueError = code
    },
    emitCatalogue() {
      for (const handler of changed) handler({ watchId: "catalogue-watch" })
    },
    emitRecord() {
      for (const handler of changed) handler({ watchId: `record-${written}` })
    },
    reconnecting() {
      for (const handler of states)
        handler({
          status: "reconnecting",
          attempt: 1,
          error: new NessaConnectionClosedError(1013, ""),
        })
    },
    connected() {
      for (const handler of states) handler({ status: "connected" })
    },
  }
}

function sessionFor(client: NessaClient | null) {
  const listeners = new Set<() => void>()
  return {
    get: () => client,
    subscribe: (listener: () => void) => {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    replace(next: NessaClient | null) {
      client = next
      for (const listener of listeners) listener()
    },
  }
}

async function openedStore() {
  const effects = scenarioEffects("echo")
  await effects.create(written)
  await effects.send({
    conversationId: written,
    executionId: "e1",
    actionId: "a1",
    text: "Book the flight",
    attachments: [],
    files: [],
  })
  const list = vi.fn(effects.list)
  const read = vi.fn(effects.read)
  const store = makeStore(
    createDependencies({ conversation: { ...effects, list, read } }),
  )
  store.dispatch(openListed({ serverConversationId: written, title: "Lisbon" }))
  return { store, list, read }
}

async function renderFollow(
  store: ReturnType<typeof makeStore>,
  session: ReturnType<typeof sessionFor>,
  connect: () => Promise<NessaClient>,
) {
  await React.act(async () => {
    root.render(
      React.createElement(Provider, {
        store,
        children: React.createElement(ConversationFollow, { session, connect }),
      }),
    )
  })
}

it("asks for the list when the catalogue ping arrives and drops the record watch on cleanup", async () => {
  const { store, list, read } = await openedStore()
  const follow = followClient()
  const session = sessionFor(follow.client)
  await renderFollow(store, session, async () => follow.client)
  await React.act(async () => {
    await Promise.resolve()
  })
  expect(follow.calls).toContain("catalogue")
  expect(follow.calls).toContain(`records:${written}`)
  expect(store.getState().conversationHistory.recordFollowed).toBe(written)
  const lists = list.mock.calls.length
  await React.act(async () => {
    follow.emitCatalogue()
  })
  expect(list.mock.calls.length).toBeGreaterThan(lists)
  await React.act(async () => {
    follow.emitRecord()
  })
  expect(read.mock.calls.length).toBeGreaterThan(0)
  await React.act(async () => root.unmount())
  expect(store.getState().conversationHistory.recordFollowed).toBeNull()
  root = createRoot(container)
})

it("clears the record watch while the socket reconnects and registers it again", async () => {
  const { store } = await openedStore()
  const follow = followClient()
  const session = sessionFor(follow.client)
  await renderFollow(store, session, async () => follow.client)
  await React.act(async () => {
    await Promise.resolve()
  })
  expect(store.getState().conversationHistory.recordFollowed).toBe(written)
  const catalogues = follow.calls.filter((call) => call === "catalogue").length
  await React.act(async () => {
    follow.reconnecting()
  })
  expect(store.getState().conversationHistory.recordFollowed).toBeNull()
  expect(follow.calls).not.toContain("close")
  await React.act(async () => {
    follow.connected()
    await Promise.resolve()
  })
  expect(follow.calls.filter((call) => call === "catalogue").length).toBe(catalogues + 1)
  expect(store.getState().conversationHistory.recordFollowed).toBe(written)
  session.replace(null)
  await React.act(async () => {
    await Promise.resolve()
  })
  expect(store.getState().conversationHistory.recordFollowed).toBeNull()
})

it("does not open another follow after the catalogue watch is refused", async () => {
  vi.useFakeTimers()
  const effects = scenarioEffects("echo")
  const store = makeStore(createDependencies({ conversation: effects }))
  const follow = followClient()
  follow.refuse("forbidden")
  const session = sessionFor(follow.client)
  let connects = 0
  await renderFollow(store, session, async () => {
    connects += 1
    return follow.client
  })
  await React.act(async () => {
    await Promise.resolve()
  })
  expect(connects).toBe(1)
  await React.act(async () => {
    await vi.advanceTimersByTimeAsync(5_000)
  })
  expect(connects).toBe(1)
  vi.useRealTimers()
})

function Probe() {
  useConversation()
  return null
}

it("does not poll the open chat while its record watch is held", async () => {
  const effects = scenarioEffects("echo")
  await effects.create(written)
  await effects.send({
    conversationId: written,
    executionId: "e1",
    actionId: "a1",
    text: "Book the flight",
    attachments: [],
    files: [],
  })
  const read = vi.fn(effects.read)
  const store = makeStore(createDependencies({ conversation: { ...effects, read } }))
  store.dispatch(
    sessionReady({ hello: {}, health: {} } as Parameters<typeof sessionReady>[0]),
  )
  store.dispatch(openListed({ serverConversationId: written, title: "Lisbon" }))
  const tab = store
    .getState()
    .conversation.conversations.find((item) => item.serverConversationId === written)
  if (!tab) throw new Error("opened chat missing")
  await store.dispatch(refreshConversation(tab.id))
  const polled = read.mock.calls.length
  await React.act(async () => {
    root.render(
      React.createElement(Provider, {
        store,
        children: React.createElement(Probe),
      }),
    )
  })
  expect(read.mock.calls.length).toBeGreaterThan(polled)
  await React.act(async () => root.unmount())
  root = createRoot(container)
  store.dispatch(recordFollowSet(written))
  const held = read.mock.calls.length
  await React.act(async () => {
    root.render(
      React.createElement(Provider, {
        store,
        children: React.createElement(Probe),
      }),
    )
  })
  await React.act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 40))
  })
  expect(read.mock.calls.length).toBe(held)
})
