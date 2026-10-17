import { permissionTargets } from "@nessa/client"
import { expect, it, vi } from "vitest"
import { makeStore } from "../../../store"
import { createDependencies } from "../../../composition/dependencies"
import type { ConversationEffects } from "../../application/ports"
import { scenarioEffects } from "../scenario/effects"
import { textContent } from "../../model"
import {
  bindConversation,
  controlConversation,
  restoreConversations,
  sendDraft,
  setDraft,
  readStarted,
} from "./slice"

const A = "00000000-0000-4000-8000-000000000001"
const B = "00000000-0000-4000-8000-000000000002"
function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (error: Error) => void
  const promise = new Promise<T>((yes, no) => {
    resolve = yes
    reject = no
  })
  return { promise, resolve, reject }
}
function fixture(overrides: Partial<ConversationEffects> = {}) {
  const follows: string[] = []
  const close = vi.fn(async () => {})
  const effects: ConversationEffects = {
    ...scenarioEffects("echo"),
    create: async (conversationId) => ({ conversationId }),
    close,
    setApprovalMode: async (_id, mode) => mode,
    follow: (id, follower) => {
      follows.push(id)
      queueMicrotask(() => follower.failed("unavailable"))
      return () => {}
    },
    ...overrides,
  }
  const store = makeStore(createDependencies({ conversation: effects }))
  store.dispatch(bindConversation({ id: "c0", serverId: A }))
  return { store, close, follows }
}
function replace(store: ReturnType<typeof makeStore>, remote = B) {
  store.dispatch(
    restoreConversations({ tabs: [{ conversationId: remote, title: "Replacement" }] }),
  )
  const id = store.getState().conversation.activeId
  store.dispatch(setDraft({ id, draft: textContent("replacement draft") }))
  store.dispatch(readStarted({ id, requestId: "replacement-read" }))
  return id
}

it.each([A, B])(
  "LT1: a close waiting for preparation cannot publish into a restored tab for %s",
  async (remote) => {
    const opening = deferred<{ conversationId: string }>()
    const { store, close, follows } = fixture({ create: () => opening.promise })
    const pending = store.dispatch(
      controlConversation({ id: "c0", control: { kind: "close" } }),
    )
    const id = replace(store, remote)
    const before = store.getState().conversation.conversations[0]
    opening.resolve({ conversationId: A })
    await pending
    expect(close).toHaveBeenCalledWith(A)
    expect(store.getState().conversation.conversations[0]).toEqual(before)
    expect(id).not.toBe("c0")
    expect(follows).toEqual([])
  },
)

it("LT2: an old permission failure after effect admission does not mark or follow a replacement", async () => {
  const entered = deferred<void>()
  const answer = deferred<void>()
  const { store, follows } = fixture({
    answer: () => {
      entered.resolve()
      return answer.promise
    },
  })
  const [target] = permissionTargets({
    conversationId: A,
    permissions: [
      {
        review: "0".repeat(32),
        executionId: "turn",
        permissionId: "review",
        options: [{ id: "allow" }],
      },
    ],
  })
  const pending = store.dispatch(
    controlConversation({
      id: "c0",
      control: { kind: "answer", target: target!, optionId: "allow" },
    }),
  )
  await entered.promise
  replace(store)
  const before = store.getState().conversation.conversations[0]
  answer.reject(new Error("Old A answer failed"))
  const result = await pending
  expect(controlConversation.rejected.match(result)).toBe(true)
  expect(store.getState().conversation.conversations[0]).toEqual(before)
  expect(follows).toEqual([])
})

it("LT3: finishing an older control does not clear a newer same-conversation control", async () => {
  const oldOpening = deferred<{ conversationId: string }>()
  const newOpening = deferred<{ conversationId: string }>()
  const create = vi
    .fn()
    .mockReturnValueOnce(oldOpening.promise)
    .mockReturnValueOnce(newOpening.promise)
  const { store } = fixture({ create })
  const old = store.dispatch(
    controlConversation({ id: "c0", control: { kind: "close" } }),
  )
  const id = replace(store, A)
  const newer = store.dispatch(
    controlConversation({ id, control: { kind: "setApprovalMode", mode: "ask" } }),
  )
  expect(store.getState().conversation.conversations[0].controlPending).toBe(true)
  oldOpening.resolve({ conversationId: A })
  await old
  expect(store.getState().conversation.conversations[0].controlPending).toBe(true)
  expect(
    store.getState().conversation.conversations[0].cancellationStatus,
  ).toBeUndefined()
  newOpening.resolve({ conversationId: A })
  await newer
  expect(store.getState().conversation.conversations[0].controlPending).toBe(false)
})

it.each(["success", "failure"] as const)(
  "LT4: an old send's %s leaves replacement draft/read/error ownership alone",
  async (outcome) => {
    const entered = deferred<void>()
    const sending = deferred<Awaited<ReturnType<ConversationEffects["submit"]>>>()
    let executionId = ""
    const { store, follows } = fixture({
      submit: (input) => {
        executionId = input.executionId
        entered.resolve()
        return sending.promise
      },
    })
    const pending = store.dispatch(sendDraft({ content: textContent("old message") }))
    await entered.promise
    replace(store)
    const before = store.getState().conversation.conversations[0]
    if (outcome === "success")
      sending.resolve({ conversationId: A, executionId, disposition: "queued" })
    else sending.reject(new Error("Old A send failed"))
    await pending
    expect(store.getState().conversation.conversations[0]).toEqual(before)
    expect(follows).toEqual([])
  },
)
