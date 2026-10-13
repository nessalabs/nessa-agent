import { questionTargets, type QuestionTarget } from "@nessa/client"
import { expect, it, vi } from "vitest"
import { makeStore } from "../../../store"
import { createDependencies } from "../../../composition/dependencies"
import type { ConversationFollower } from "../../application/ports"
import { scenarioEffects } from "../scenario/effects"
import {
  bindConversation,
  controlConversation,
  openConversation,
  setActive,
  restoreConversations,
} from "./slice"

const first = "00000000-0000-4000-8000-000000000001"
const second = "00000000-0000-4000-8000-000000000002"

function captured(conversationId: string) {
  return questionTargets({
    conversationId,
    questions: [
      {
        executionId: "captured-turn",
        questionId: "captured-ask",
        message: "Ask",
        questions: [],
      },
    ],
  })[0]!
}
function fixture(create = vi.fn(async (conversationId: string) => ({ conversationId }))) {
  const answer = vi.fn(async () => {})
  const store = makeStore(
    createDependencies({
      conversation: {
        ...scenarioEffects("echo"),
        create,
        answerQuestion: answer,
        follow: (_id: string, follower: ConversationFollower) => {
          queueMicrotask(() => follower.failed("unavailable"))
          return () => {}
        },
      },
    }),
  )
  store.dispatch(bindConversation({ id: "c0", serverId: first }))
  store.dispatch(openConversation())
  store.dispatch(bindConversation({ id: "c1", serverId: second }))
  return { store, create, answer }
}

it("QT7: answerQuestion refuses a captured ask sent to another conversation's tab", async () => {
  const { store, create, answer } = fixture()
  await store.dispatch(
    controlConversation({
      id: "c1",
      control: { kind: "answerQuestion", target: captured(first), choices: null },
    }),
  )
  expect(create).not.toHaveBeenCalled()
  expect(answer).not.toHaveBeenCalled()
  expect(
    store.getState().conversation.conversations.every((tab) => !tab.controlPending),
  ).toBe(true)
})

it("QT3: answerQuestion rejects an edited target before preparing a conversation", async () => {
  const { store, create, answer } = fixture()
  const target = {
    ...captured(first),
    executionId: "another-turn",
  } as QuestionTarget
  const result = await store.dispatch(
    controlConversation({
      id: "c0",
      control: { kind: "answerQuestion", target, choices: null },
    }),
  )
  expect(controlConversation.rejected.match(result)).toBe(true)
  expect(create).not.toHaveBeenCalled()
  expect(answer).not.toHaveBeenCalled()
})

it("QT6: answerQuestion keeps its captured ask while preparation waits and the active tab changes", async () => {
  let release!: (value: { conversationId: string }) => void
  const gate = new Promise<{ conversationId: string }>((done) => {
    release = done
  })
  const create = vi.fn(() => gate)
  const { store, answer } = fixture(create)
  const target = captured(first)
  const pending = store.dispatch(
    controlConversation({
      id: "c0",
      control: { kind: "answerQuestion", target, choices: null },
    }),
  )
  expect(create).toHaveBeenCalledWith(first, undefined)
  store.dispatch(setActive("c1"))
  release({ conversationId: first })
  await pending
  expect(answer).toHaveBeenCalledWith(target, null)
  expect(create).toHaveBeenCalledTimes(1)
  expect(store.getState().conversation.activeId).toBe("c1")
})

it("QT7: a delayed answerQuestion does nothing after restoration replaces its tab with another conversation", async () => {
  const { store, create, answer } = fixture()
  const target = captured(first)
  store.dispatch(
    restoreConversations({ tabs: [{ conversationId: second, title: "Restored" }] }),
  )
  expect(store.getState().conversation.conversations[0]?.serverConversationId).toBe(
    second,
  )
  await store.dispatch(
    controlConversation({
      id: "c0",
      control: { kind: "answerQuestion", target, choices: null },
    }),
  )
  expect(create).not.toHaveBeenCalled()
  expect(answer).not.toHaveBeenCalled()
})
