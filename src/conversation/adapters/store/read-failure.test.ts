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
  questions: [],
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

it("shows no code for a gateway outage, and promises no recovery it cannot make", async () => {
  for (const code of [
    ConversationErrorCode.TemporarilyUnavailable,
    // A read waits for the conversation's agent to open, so a restored tab
    // polling a cold gateway meets this one routinely — and so does one whose
    // conversation the gateway has stopped serving until it restarts. The
    // gateway sends the same code for both, so this says only what holds of
    // both: the view is stale, and the panel is still asking.
    ConversationErrorCode.AgentStartupDeadline,
  ]) {
    const tab = await refreshed(new NessaRpcError(code, code))
    expect(tab.readError).toBe("unavailable")
    const notice = conversationNotice(tab)
    expect(notice).toMatchObject({
      title: "Conversation not refreshed",
      retry: { kind: "refresh" },
    })
    expect(notice?.description).not.toMatch(/catch(es)? up|in a moment|shortly|soon/)
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
  expect(store.getState().conversation.conversations[0]!.readError).toBe("unavailable")
  await store.dispatch(refreshConversation("c0"))
  const tab = store.getState().conversation.conversations[0]!
  expect(tab.readError).toBeUndefined()
  expect(conversationNotice(tab)).toBeNull()
})

/** A close whose acknowledgement is lost, with the refresh behind it failing too. */
function closedWithFailingRead(readCause: unknown) {
  const client = {
    conversation: {
      create: async () => ({ conversationId: "server" }),
      close: () => Promise.reject(new Error("acknowledgement lost")),
      read: () => Promise.reject(readCause),
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
  return store
    .dispatch(controlConversation({ id: "c0", control: { kind: "close" } }))
    .then(() => store.getState().conversation.conversations[0]!)
}

it("keeps a failed command and a failed read as two facts that cannot contradict", async () => {
  // Both at once, in their own fields and their own vocabularies. An ordinary
  // stale view adds nothing to the control's own sentence, so the sentence
  // somebody is waiting for is the one shown, and the read never rewrites it.
  const tab = await closedWithFailingRead(new Error("socket closed"))
  expect(tab.failure).toBeUndefined()
  expect(tab.error).toBe("acknowledgement lost")
  expect(tab.readError).toBe("unavailable")
  expect(conversationNotice(tab)).toEqual({
    title: "Conversation needs attention",
    description: "acknowledgement lost",
    retry: { kind: "refresh" },
  })
})

it("never offers a refresh for a conversation the gateway has stopped serving", async () => {
  // The same lost acknowledgement, over a conversation whose configuration the
  // gateway no longer has. Its sentence invites a refresh, and the refresh is
  // exactly what can no longer succeed — so the permanent fact wins the notice,
  // and the retry goes with it. Both facts are still on the tab.
  const tab = await closedWithFailingRead(
    new NessaRpcError(
      ConversationErrorCode.ConversationConfigurationChanged,
      "setup changed",
    ),
  )
  expect(tab.error).toBe("acknowledgement lost")
  expect(tab.readError).toBe("configuration-changed")
  expect(conversationNotice(tab)).toEqual({
    title: "Conversation setup changed",
    description:
      "This chat uses a different agent configuration. Start a new conversation with the current setup.",
    retry: null,
  })
})

it("reports what a read failure was translated from, including a code it could not name", async () => {
  // The tab keeps the word; the console keeps the cause. Without this the one
  // case with no other way out — a gateway answering about a different
  // conversation — reads as an ordinary stale view and the evidence is gone.
  const warn = vi.spyOn(console, "warn").mockImplementation(() => {})
  try {
    const unnamed = new NessaRpcError("conversation_quiesced", "the gateway said so")
    await refreshed(unnamed)
    expect(warn).toHaveBeenLastCalledWith(
      "[nessa] a conversation was not refreshed",
      "unavailable",
      expect.objectContaining({ cause: unnamed }),
    )
    // The gateway serving another conversation's data: not an adapter failure
    // at all, so the word is all the tab has and the sentence lives only here.
    const store = reading(async () => view("somebody-else"))
    await store.dispatch(refreshConversation("c0"))
    expect(store.getState().conversation.conversations[0]!.readError).toBe("unavailable")
    expect(warn).toHaveBeenLastCalledWith(
      "[nessa] a conversation was not refreshed",
      "unavailable",
      expect.objectContaining({
        message: "Gateway returned a different conversation identity.",
      }),
    )
  } finally {
    warn.mockRestore()
  }
})
