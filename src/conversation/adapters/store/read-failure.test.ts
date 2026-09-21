import { ConversationErrorCode, NessaRpcError, type NessaClient } from "@nessa/client"
import { expect, it, vi } from "vitest"
import { makeStore } from "../../../store"
import { createDependencies } from "../../../composition/dependencies"
import type { ConversationView } from "../../application/view"
import { conversationNotice } from "../../ui/notification"
import { gatewayEffects } from "../gateway/effects"
import { bindConversation, controlConversation, refreshConversation } from "./slice"

/**
 * What a failed read becomes on the tab, and what the panel then says.
 *
 * The whole path, through the real adapter: the gateway's code, the client's
 * `NessaRpcError`, the translation, the store, and the notice. The gateway sends
 * the code as the message as well, which is the coincidence the panel used to
 * rest on — so every message here is deliberately *not* the code, and a notice
 * that still appears is one that came from the typed answer.
 */
const view = (conversationId: string): ConversationView => ({
  conversationId,
  revision: "1",
  messages: [],
  pending: [],
  permissions: [],
  tools: [],
  capabilities: {
    queue: true,
    steer: true,
    resume: true,
    permissions: true,
    imageInput: false,
  },
  truncated: false,
  queueComplete: true,
})

function reading(read: () => Promise<ConversationView>) {
  const client = { conversation: { read } } as unknown as NessaClient
  const store = makeStore(
    createDependencies({
      conversation: gatewayEffects(
        () => client,
        () => Promise.reject(new Error("no wait expected")),
      ),
    }),
  )
  store.dispatch(bindConversation({ id: "c0", serverId: "server" }))
  return store
}

async function refreshed(cause: unknown) {
  const store = reading(() => Promise.reject(cause))
  await store.dispatch(refreshConversation("c0"))
  return store.getState().conversation.conversations[0]!
}

it("still explains a configuration change when the gateway sends a sentence, not its code", async () => {
  // The failure this panel used to recognise only because the gateway happened
  // to repeat the code as the message. A gateway writing prose there must not
  // take the notice away.
  const tab = await refreshed(
    new NessaRpcError(
      ConversationErrorCode.ConversationConfigurationChanged,
      "This conversation was created against a different agent configuration.",
    ),
  )
  expect(tab.readError).toBe("configuration-changed")
  expect(conversationNotice(tab)).toEqual({
    title: "Conversation setup changed",
    description:
      "This chat uses a different agent configuration. Start a new conversation with the current setup.",
    retry: null,
  })
})

it("tells somebody whose gateway is briefly busy to wait, rather than showing them a code", async () => {
  for (const code of [
    ConversationErrorCode.TemporarilyUnavailable,
    // A read waits for the conversation's agent to open, so a restored tab
    // polling a cold gateway meets this one routinely.
    ConversationErrorCode.AgentStartupDeadline,
  ]) {
    const tab = await refreshed(new NessaRpcError(code, code))
    expect(tab.readError).toBe("busy")
    const notice = conversationNotice(tab)
    expect(notice).toMatchObject({
      title: "Waiting for the gateway",
      retry: { kind: "refresh" },
    })
    // The gateway's word for it reaches nobody.
    expect(JSON.stringify(notice)).not.toContain(code)
  }
})

it("does not reinterpret a code this build has never heard of, or claim a cause for it", async () => {
  const tab = await refreshed(
    // A code from a newer gateway, whose message happens to be a code this
    // build *does* know. Neither may decide anything.
    new NessaRpcError("conversation_quiesced", "conversation_configuration_changed"),
  )
  expect(tab.readError).toBe("unavailable")
  const notice = conversationNotice(tab)
  expect(notice).toMatchObject({
    title: "Conversation not refreshed",
    retry: { kind: "refresh" },
  })
  expect(notice?.description).toBe(
    "Nessa could not read this conversation from the gateway, so what is shown may be out of date. It keeps trying.",
  )
  expect(JSON.stringify(notice)).not.toContain("conversation_")
})

it("says the same for a read that never reached a gateway answer", async () => {
  // A dropped connection has no code at all, and nothing is invented for it.
  const tab = await refreshed(new Error("socket closed"))
  expect(tab.readError).toBe("unavailable")
  expect(conversationNotice(tab)?.description).not.toContain("socket closed")
})

it("lets the next view clear the read failure, notice and all", async () => {
  const read = vi
    .fn()
    .mockRejectedValueOnce(
      new NessaRpcError(ConversationErrorCode.TemporarilyUnavailable, "busy right now"),
    )
    .mockResolvedValueOnce(view("server"))
  const store = reading(read as () => Promise<ConversationView>)
  await store.dispatch(refreshConversation("c0"))
  expect(store.getState().conversation.conversations[0]!.readError).toBe("busy")
  await store.dispatch(refreshConversation("c0"))
  const tab = store.getState().conversation.conversations[0]!
  expect(tab.readError).toBeUndefined()
  expect(conversationNotice(tab)).toBeNull()
})

it("keeps a failed command and a failed read as two facts that cannot contradict", async () => {
  // A control fails, and the refresh it runs afterwards fails too. Both are
  // recorded, in their own fields and their own vocabularies; the notice shows
  // the one somebody asked for, and the read's word never rewrites it.
  const client = {
    conversation: {
      create: async () => ({ conversationId: "server" }),
      close: () => Promise.reject(new Error("acknowledgement lost")),
      read: () =>
        Promise.reject(
          new NessaRpcError(
            ConversationErrorCode.ConversationConfigurationChanged,
            "setup changed",
          ),
        ),
    },
  } as unknown as NessaClient
  const store = makeStore(
    createDependencies({
      conversation: gatewayEffects(
        () => client,
        () => Promise.reject(new Error("no wait expected")),
      ),
    }),
  )
  store.dispatch(bindConversation({ id: "c0", serverId: "server" }))
  await store.dispatch(controlConversation({ id: "c0", control: { kind: "close" } }))
  const tab = store.getState().conversation.conversations[0]!
  expect(tab.failure).toBeUndefined()
  expect(tab.error).toBe("acknowledgement lost")
  expect(tab.readError).toBe("configuration-changed")
  // The control's own sentence, with the refresh it invites — not the read's
  // "start a new conversation", which would withdraw the retry the lost
  // acknowledgement is exactly what needs.
  expect(conversationNotice(tab)).toEqual({
    title: "Conversation needs attention",
    description: "acknowledgement lost",
    retry: { kind: "refresh" },
  })
})
