import { prepareSubmission } from "@nessa/client"
import { expect, it, vi } from "vitest"
import { makeStore } from "../../../store"
import { createDependencies } from "../../../composition/dependencies"
import { scenarioEffects } from "../scenario/effects"
import { gatewayEffects } from "../gateway/effects"
import { textContent } from "../../model"
import {
  bindConversation,
  followConversation,
  sendDraft,
  setDraft,
  submissionStarted,
  submissionFailed,
  submissionAccepted,
  failedTurnDiscarded,
  showError,
} from "./slice"
import { followByReading } from "../../testing"
import {
  ConversationErrorCode,
  NessaConversationMutationError,
  NessaRpcError,
  type NessaClient,
} from "@nessa/client"
import { conversationNotice } from "../../ui/notification"

it("a failed creation cannot make an unattempted message admission uncertain", async () => {
  const effects = scenarioEffects("echo")
  const send = vi.fn(effects.submit)
  const store = makeStore(
    createDependencies({
      conversation: {
        ...effects,
        submit: send,
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
  expect(conversationNotice(tab)?.retry).toMatchObject({ kind: "draft" })
})
/**
 * A store over the real gateway adapter, whose client refuses to open the
 * conversation with the gateway's own typed rejection.
 *
 * The real adapter, because the whole question is where the wire's word for a
 * failure becomes the panel's: a substitute that answered in the panel's words
 * already would be asserting its own fixture.
 */
function refusingCreation(code: string) {
  const client = {
    conversation: {
      create: () =>
        Promise.reject(
          new NessaConversationMutationError(
            "00000000-0000-4000-8000-000000000001",
            "action",
            undefined,
            new NessaRpcError(code, "agent_startup_deadline"),
            async () => undefined,
          ),
        ),
    },
  } as unknown as NessaClient
  return makeStore(
    createDependencies({
      conversation: gatewayEffects(
        () => client,
        () => Promise.reject(new Error("no wait expected")),
        () => 0,
      ),
    }),
  )
}

it("carries the gateway's startup deadline into the notice as the panel's own reason", async () => {
  const store = refusingCreation(ConversationErrorCode.AgentStartupDeadline)
  await store.dispatch(sendDraft({ content: textContent("first message") }))
  const tab = store.getState().conversation.conversations[0]!
  expect(tab.failure).toBe("agent-startup-deadline")
  expect(tab.turns[0]).toMatchObject({ receipt: "failed" })
  // The client's own sentence names the remedy at length; the panel keeps it.
  expect(tab.error).toMatch(/still starting and ran out of time/)
  expect(conversationNotice(tab)).toMatchObject({
    title: "Error sending",
    retry: { kind: "draft" },
  })
  // A retried send clears the rejection it described, reason and message together.
  store.dispatch(setDraft({ id: tab.id, draft: textContent("retry") }))
  store.dispatch(
    submissionStarted({
      conversationId: tab.id,
      content: textContent("retry"),
      prepared: prepareSubmission({
        conversationId: tab.serverConversationId!,
        executionId: "execution",
        requestId: "action",
        text: "retry",
        mode: "queued",
      }),
    }),
  )
  const retried = store.getState().conversation.conversations[0]!
  expect(retried.error).toBeUndefined()
  expect(retried.failure).toBeUndefined()
})
it("leaves a rejection this build has no word for untyped, and its sentence alone", async () => {
  // A code no build here knows, whose message is a code this one does. The
  // client hands back no typed code for it, and nothing downstream may read
  // one out of the text: the tab carries no reason at all.
  const store = refusingCreation("quantum_flux")
  await store.dispatch(sendDraft({ content: textContent("first message") }))
  const tab = store.getState().conversation.conversations[0]!
  expect(tab.failure).toBeUndefined()
  // Refused all the same: nothing was created, so nothing was sent.
  expect(tab.turns[0]).toMatchObject({ receipt: "failed" })
  expect(tab.draft).toEqual(textContent("first message"))
  expect(conversationNotice(tab)).toMatchObject({
    title: "Error sending",
    retry: { kind: "draft" },
  })
})
it("client loss after cached creation is known unsent rather than an uncertain admission", async () => {
  let client: NessaClient | null = {
    conversation: {
      create: async () => ({ conversationId: "00000000-0000-4000-8000-000000000001" }),
    },
  } as unknown as NessaClient
  const effects = gatewayEffects(
    () => client,
    () => Promise.reject(new Error("no wait expected")),
    () => 0,
  )
  await effects.create("00000000-0000-4000-8000-000000000001")
  client = null
  const store = makeStore(createDependencies({ conversation: effects }))
  store.dispatch(
    bindConversation({ id: "c0", serverId: "00000000-0000-4000-8000-000000000001" }),
  )
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
        follow: followByReading(async () => ({
          conversationId: "00000000-0000-4000-8000-000000000001",
          artifacts: [],
          notes: [],
          unreadable: [],
          unreadableTruncated: false,
          approvalMode: "ask",
          approvalModes: [
            {
              id: "ask",
              name: "Provider asks",
              description: "Provider asks where required.",
            },
          ],
          title: null,
          revision: "running",
          queueComplete: true,
          transcriptState: "complete",
          truncated: false,
          pending: [],
          reviewsWaiting: [],
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
              permissionDenial: "unknown",
              nativeHookSuppression: "unknown",
              nativeHooks: "off",
              compactionReporting: "unsupported_not_implemented",
              modelSwitchReporting: "unsupported_not_implemented",
              permissionDeferral: "unsupported_not_implemented",
              elicitationForwarding: "unknown",
              preToolPolicy: "unsupported_not_implemented",
              policyEndTurn: "unsupported",
              policyCloseSession: "unsupported",
              incomingElicitation: "unsupported",
            },
          },
          lifecycle: { phase: "attached" },
          messages: [
            {
              executionId: "active",
              userText: "first",
              attachments: [],
              files: [],
              parts: [
                { offset: 0, kind: "thought", text: "", toolId: "", noticeId: "" },
                { offset: 1, kind: "text", text: "", toolId: "", noticeId: "" },
              ],
              status: "running",
            },
          ],
        })),
      },
    }),
  )
  store.dispatch(
    bindConversation({ id: "c0", serverId: "00000000-0000-4000-8000-000000000001" }),
  )
  await store.dispatch(followConversation("c0"))
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
  const effects = gatewayEffects(
    () => client,
    () => Promise.reject(new Error("no wait expected")),
    () => 0,
  )
  await expect(
    Promise.resolve().then(() =>
      effects.submit(
        prepareSubmission({
          mode: "queued",
          conversationId: "00000000-0000-4000-8000-000000000001",
          executionId: "e",
          requestId: "a",
          text: "unsent",
          attachments: [],
          files: [],
        }),
      ),
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
  expect(conversationNotice(tab)?.retry).toMatchObject({ kind: "draft" })
})

it("explicit retries replace one refused bubble with fresh wire identities; independent sends stay distinct", async () => {
  const effects = scenarioEffects("echo")
  const create = vi.fn(async () => {
    throw new Error("offline")
  })
  const store = makeStore(createDependencies({ conversation: { ...effects, create } }))
  await store.dispatch(sendDraft({ content: textContent("hello") }))
  const first = store.getState().conversation.conversations[0]!
  const original = first.turns[0]!
  store.dispatch(setDraft({ id: first.id, draft: textContent("edited") }))
  await store.dispatch(
    sendDraft({ id: first.id, content: textContent("edited"), retryTurnId: original.id }),
  )
  const retried = store.getState().conversation.conversations[0]!
  expect(retried.turns).toHaveLength(1)
  expect(retried.turns[0]).toMatchObject({
    id: original.id,
    content: textContent("edited"),
    receipt: "failed",
  })
  if (original.from !== "user" || retried.turns[0]?.from !== "user")
    throw new Error("missing user turn")
  expect(retried.turns[0].executionId).not.toBe(original.executionId)
  expect(retried.turns[0].actionId).not.toBe(original.actionId)
  store.dispatch(
    submissionFailed({
      id: first.id,
      executionId: original.executionId!,
      message: "obsolete error",
      outcome: { kind: "refused", reupload: false },
    }),
  )
  expect(store.getState().conversation.conversations[0]).toBe(retried)
  await store.dispatch(sendDraft({ id: first.id, content: textContent("edited") }))
  const independent = store.getState().conversation.conversations[0]!
  expect(independent.turns).toHaveLength(2)
  expect(independent.turns[1]?.id).not.toBe(original.id)
  const calls = create.mock.calls.length
  await store.dispatch(
    sendDraft({
      id: first.id,
      content: textContent("edited"),
      retryTurnId: "another-tab-turn",
    }),
  )
  expect(create).toHaveBeenCalledTimes(calls)
  expect(store.getState().conversation.conversations[0]?.turns).toHaveLength(2)
})

it("retry and discard cannot replace an accepted message or discard an edited draft", async () => {
  const effects = scenarioEffects("echo")
  const create = vi.fn(async () => {
    throw new Error("offline")
  })
  const store = makeStore(createDependencies({ conversation: { ...effects, create } }))
  await store.dispatch(sendDraft({ content: textContent("hello") }))
  const first = store.getState().conversation.conversations[0]!
  const turn = first.turns[0]!
  if (turn.from !== "user") throw new Error("missing user turn")
  store.dispatch(setDraft({ id: first.id, draft: textContent("new work") }))
  store.dispatch(failedTurnDiscarded({ id: first.id, turnId: "foreign" }))
  expect(store.getState().conversation.conversations[0]?.turns).toHaveLength(1)
  store.dispatch(submissionAccepted({ id: first.id, executionId: turn.executionId! }))
  await store.dispatch(
    sendDraft({ id: first.id, content: textContent("new work"), retryTurnId: turn.id }),
  )
  store.dispatch(failedTurnDiscarded({ id: first.id, turnId: turn.id }))
  expect(create).toHaveBeenCalledTimes(1)
  expect(store.getState().conversation.conversations[0]?.turns).toHaveLength(1)
  expect(store.getState().conversation.conversations[0]?.draft).toEqual(
    textContent("new work"),
  )
})

it("discard removes only the refused bubble and preserves current draft work", async () => {
  const effects = scenarioEffects("echo")
  const store = makeStore(
    createDependencies({
      conversation: {
        ...effects,
        create: async () => {
          throw new Error("offline")
        },
      },
    }),
  )
  await store.dispatch(sendDraft({ content: textContent("hello") }))
  const first = store.getState().conversation.conversations[0]!
  store.dispatch(setDraft({ id: first.id, draft: textContent("new work") }))
  store.dispatch(failedTurnDiscarded({ id: first.id, turnId: first.turns[0]!.id }))
  expect(store.getState().conversation.conversations[0]?.turns).toEqual([])
  expect(store.getState().conversation.conversations[0]?.draft).toEqual(
    textContent("new work"),
  )
})

it.each([
  // docs/state/services/desktop/chat/recover-a-refused-or-unconfirmed-send.md,
  // "Several refused messages", rows 4 and 5, and row 7: a control failing in
  // the refusal's own words is still the control's.
  "Close failed",
  "offline",
])(
  "a control that fails after a refusal keeps its notice (%s), and discarding the refusal does not clear it",
  async (message) => {
    const effects = scenarioEffects("echo")
    const store = makeStore(
      createDependencies({
        conversation: {
          ...effects,
          create: async () => {
            throw new Error("offline")
          },
        },
      }),
    )
    await store.dispatch(sendDraft({ content: textContent("hello") }))
    const first = store.getState().conversation.conversations[0]!
    store.dispatch(showError({ id: first.id, message }))
    const tab = () => store.getState().conversation.conversations[0]!
    expect(conversationNotice(tab())).toMatchObject({
      title: "Conversation needs attention",
      description: message,
    })
    store.dispatch(failedTurnDiscarded({ id: first.id, turnId: first.turns[0]!.id }))
    expect(tab().turns).toEqual([])
    expect(conversationNotice(tab())).toMatchObject({
      title: "Conversation needs attention",
      description: message,
    })
  },
)

it("discarding the current refusal offers the older one with its own error", async () => {
  // docs/state/services/desktop/chat/recover-a-refused-or-unconfirmed-send.md,
  // "Several refused messages", row 6.
  const effects = scenarioEffects("echo")
  const errors = ["offline", "busy"]
  const store = makeStore(
    createDependencies({
      conversation: {
        ...effects,
        create: async () => {
          throw new Error(errors.shift() ?? "unexpected")
        },
      },
    }),
  )
  await store.dispatch(sendDraft({ content: textContent("first") }))
  const id = store.getState().conversation.conversations[0]!.id
  await store.dispatch(sendDraft({ id, content: textContent("second") }))
  const tab = () => store.getState().conversation.conversations[0]!
  const [older, newer] = tab().turns
  expect(conversationNotice(tab())?.retry).toEqual({ kind: "draft", turnId: newer!.id })
  store.dispatch(failedTurnDiscarded({ id, turnId: newer!.id }))
  expect(conversationNotice(tab())).toEqual({
    title: "Error sending",
    description: tab().error,
    retry: { kind: "draft", turnId: older!.id },
  })
  expect(older).toMatchObject({ receipt: "failed", error: tab().error })
})

it("retains the default agent actually requested and the original diagnostic for setup and copying", async () => {
  const failure = new NessaConversationMutationError(
    "c",
    "request",
    undefined,
    new NessaRpcError(
      ConversationErrorCode.AgentNotConfigured,
      "provider launch diagnostic",
    ),
    async () => undefined,
  )
  let selected = "claude"
  const create = vi.fn(async () => {
    throw failure
  })
  const client = { conversation: { create } } as unknown as NessaClient
  const effects = gatewayEffects(
    () => client,
    async () => {},
    () => 0,
    async () => selected,
  )
  const store = makeStore(createDependencies({ conversation: effects }))
  const sending = store.dispatch(sendDraft({ content: textContent("hello") }))
  selected = "codex"
  await sending
  expect(create.mock.calls).toHaveLength(1)
  const tab = store.getState().conversation.conversations[0]!
  expect(tab.turns[0]).toMatchObject({
    receipt: "failed",
    diagnostic: {
      agent: "claude",
      code: ConversationErrorCode.AgentNotConfigured,
      message: failure.message,
    },
  })
  expect(tab.selection).toBeUndefined()
})
