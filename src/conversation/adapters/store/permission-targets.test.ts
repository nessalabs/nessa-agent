import { permissionTargets, type PermissionTarget } from "@nessa/client"
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
  return permissionTargets({
    conversationId,
    permissions: [
      {
        review: "0".repeat(32),
        executionId: "captured-turn",
        permissionId: "captured-review",
        options: [{ id: "allow" }],
      },
    ],
  })[0]!
}
function fixture(create = vi.fn(async (conversationId: string) => ({ conversationId }))) {
  const answer = vi.fn(async () => {})
  const cancel = vi.fn(async () => {})
  const store = makeStore(
    createDependencies({
      conversation: {
        ...scenarioEffects("echo"),
        create,
        answer,
        cancel,
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
  return { store, create, answer, cancel }
}

it.each(["answer", "cancel"] as const)(
  "PT4: %s refuses a captured review sent to another conversation's tab",
  async (kind) => {
    const { store, create, answer, cancel } = fixture()
    await store.dispatch(
      controlConversation({
        id: "c1",
        control: { kind, target: captured(first), optionId: "allow" },
      }),
    )
    expect(create).not.toHaveBeenCalled()
    expect(answer).not.toHaveBeenCalled()
    expect(cancel).not.toHaveBeenCalled()
    expect(
      store.getState().conversation.conversations.every((tab) => !tab.controlPending),
    ).toBe(true)
  },
)

it.each(["answer", "cancel"] as const)(
  "PT3: %s rejects an edited target before preparing a conversation",
  async (kind) => {
    const { store, create, answer, cancel } = fixture()
    const target = {
      ...captured(first),
      executionId: "another-turn",
    } as PermissionTarget
    const result = await store.dispatch(
      controlConversation({ id: "c0", control: { kind, target, optionId: "allow" } }),
    )
    expect(controlConversation.rejected.match(result)).toBe(true)
    expect(create).not.toHaveBeenCalled()
    expect(answer).not.toHaveBeenCalled()
    expect(cancel).not.toHaveBeenCalled()
  },
)

it.each(["answer", "cancel"] as const)(
  "PT5: %s keeps its captured review while preparation waits and the active tab changes",
  async (kind) => {
    let release!: (value: { conversationId: string }) => void
    const gate = new Promise<{ conversationId: string }>((done) => {
      release = done
    })
    const create = vi.fn(() => gate)
    const { store, answer, cancel } = fixture(create)
    const target = captured(first)
    const pending = store.dispatch(
      controlConversation({ id: "c0", control: { kind, target, optionId: "allow" } }),
    )
    expect(create).toHaveBeenCalledWith(first, undefined)
    store.dispatch(setActive("c1"))
    release({ conversationId: first })
    await pending
    if (kind === "answer") expect(answer).toHaveBeenCalledWith(target, "allow")
    else expect(cancel).toHaveBeenCalledWith(target)
    expect(create).toHaveBeenCalledTimes(1)
    expect(store.getState().conversation.activeId).toBe("c1")
  },
)

it.each(["answer", "cancel"] as const)(
  "PT4: a delayed %s does nothing after restoration replaces the original local tab",
  async (kind) => {
    const { store, create, answer, cancel } = fixture()
    const target = captured(first)
    store.dispatch(
      restoreConversations({ tabs: [{ conversationId: second, title: "Restored" }] }),
    )
    expect(store.getState().conversation.conversations[0]?.id).not.toBe("c0")
    await store.dispatch(
      controlConversation({ id: "c0", control: { kind, target, optionId: "allow" } }),
    )
    expect(create).not.toHaveBeenCalled()
    expect(answer).not.toHaveBeenCalled()
    expect(cancel).not.toHaveBeenCalled()
  },
)
