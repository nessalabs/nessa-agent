import { expect, it, vi } from "vitest"
import { makeStore } from "../../../store"
import { createDependencies } from "../../../composition/dependencies"
import { scenarioEffects } from "../scenario/effects"
import { gatewayEffects } from "../gateway/effects"
import { textContent } from "../../model"
import {
  bindConversation,
  refreshConversation,
  sendDraft,
  setDraft,
  submissionStarted,
} from "./slice"
import {
  ConversationErrorCode,
  NessaConversationMutationError,
  NessaRpcError,
  type NessaClient,
} from "@nessa/client"
import { conversationNotice } from "../../ui/notification"

it("a failed creation cannot make an unattempted message admission uncertain", async () => {
  const effects = scenarioEffects("echo")
  const send = vi.fn(effects.send)
  const store = makeStore(
    createDependencies({
      conversation: {
        ...effects,
        send,
        create: async () => {
          throw new Error("creation acknowledgement lost")
        },
      },
    }),
  )
  await store.dispatch(sendDraft({ content: textContent("unsent") }))
  expect(send).not.toHaveBeenCalled()
  const tab = store.getState().conversation.conversations[0]!
  expect(tab.turns[0]).toMatchObject({
    receipt: "failed",
    content: textContent("unsent"),
  })
  expect(tab.phase).toBe("idle")
  expect(tab.draft).toEqual(textContent("unsent"))
  expect(tab.draftReset).toBe(1)
  expect(conversationNotice(tab)?.retry).toEqual({ kind: "draft" })
})
it("carries the gateway startup-deadline code into the notice, not just its text", async () => {
  const effects = scenarioEffects("echo")
  const store = makeStore(
    createDependencies({
      conversation: {
        ...effects,
        create: async () => {
          throw new NessaConversationMutationError(
            "server",
            "action",
            undefined,
            new NessaRpcError(
              ConversationErrorCode.AgentStartupDeadline,
              "agent_startup_deadline",
            ),
            async () => undefined,
          )
        },
      },
    }),
  )
  await store.dispatch(sendDraft({ content: textContent("first message") }))
  const tab = store.getState().conversation.conversations[0]!
  expect(tab.errorCode).toBe(ConversationErrorCode.AgentStartupDeadline)
  expect(tab.turns[0]).toMatchObject({ receipt: "failed" })
  expect(conversationNotice(tab)).toMatchObject({
    title: "Agent was still starting",
    retry: { kind: "draft" },
  })
  // A retried send clears the rejection it described, code and message together.
  store.dispatch(setDraft({ id: tab.id, draft: textContent("retry") }))
  store.dispatch(
    submissionStarted({
      conversationId: tab.id,
      executionId: "execution",
      actionId: "action",
      content: textContent("retry"),
      mode: "queued",
    }),
  )
  const retried = store.getState().conversation.conversations[0]!
  expect(retried.error).toBeUndefined()
  expect(retried.errorCode).toBeUndefined()
})
it("client loss after cached creation is known unsent rather than an uncertain admission", async () => {
  let client: NessaClient | null = {
    conversation: { create: async () => ({ conversationId: "server" }) },
  } as unknown as NessaClient
  const effects = gatewayEffects(() => client)
  await effects.create("server")
  client = null
  const store = makeStore(createDependencies({ conversation: effects }))
  store.dispatch(bindConversation({ id: "c0", serverId: "server" }))
  await store.dispatch(sendDraft({ content: textContent("offline") }))
  const tab = store.getState().conversation.conversations[0]!
  expect(tab.turns[0]).toMatchObject({
    receipt: "failed",
    content: textContent("offline"),
  })
  expect(tab.phase).toBe("idle")
})
it("known-unsent follow-up does not settle the earlier running invocation before its first chunk", async () => {
  const effects = scenarioEffects("echo")
  const store = makeStore(
    createDependencies({
      conversation: {
        ...effects,
        create: async () => {
          throw new Error("offline")
        },
        read: async () => ({
          conversationId: "server",
          revision: "running",
          queueComplete: true,
          truncated: false,
          pending: [],
          permissions: [],
          tools: [],
          capabilities: { queue: true, steer: true, resume: true, permissions: true },
          messages: [
            {
              executionId: "active",
              userText: "first",
              parts: [
                { offset: 0, kind: "thought", text: "", toolId: "" },
                { offset: 1, kind: "text", text: "", toolId: "" },
              ],
              status: "running",
            },
          ],
        }),
      },
    }),
  )
  store.dispatch(bindConversation({ id: "c0", serverId: "server" }))
  await store.dispatch(refreshConversation("c0"))
  await store.dispatch(sendDraft({ content: textContent("follow-up") }))
  const tab = store.getState().conversation.conversations[0]!
  expect(tab.turns[1]).toMatchObject({ receipt: "failed" })
  expect(tab.remote?.running).toBe(true)
  expect(tab.phase).toBe("thinking")
})

it("a reconnecting client rejects locally without attempting a message RPC", async () => {
  const send = vi.fn()
  const client = {
    connectionState: { status: "reconnecting" },
    conversation: { send },
  } as unknown as NessaClient
  const effects = gatewayEffects(() => client)
  await expect(
    Promise.resolve().then(() =>
      effects.send({
        conversationId: "server",
        executionId: "e",
        actionId: "a",
        text: "unsent",
      }),
    ),
  ).rejects.toThrow("was not sent")
  expect(send).not.toHaveBeenCalled()
})

it("late known-unsent failure preserves a newer draft instead of restoring over it", async () => {
  const effects = scenarioEffects("echo")
  let reject!: (error: Error) => void
  const creating = new Promise<{ conversationId: string }>((_, fail) => {
    reject = fail
  })
  const store = makeStore(
    createDependencies({ conversation: { ...effects, create: () => creating } }),
  )
  const sending = store.dispatch(sendDraft({ content: textContent("original") }))
  store.dispatch(setDraft({ id: "c0", draft: textContent("newer draft") }))
  reject(new Error("offline"))
  await sending
  const tab = store.getState().conversation.conversations[0]!
  expect(tab.draft).toEqual(textContent("newer draft"))
  expect(tab.draftReset).toBeUndefined()
  expect(tab.turns[0]).toMatchObject({
    receipt: "failed",
    content: textContent("original"),
  })
  expect(conversationNotice(tab)?.retry).toEqual({ kind: "draft" })
})
