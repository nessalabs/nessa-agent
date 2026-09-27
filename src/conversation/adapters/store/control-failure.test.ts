import {
  ConversationErrorCode,
  NessaConversationControlError,
  NessaConversationMutationError,
  NessaRpcError,
  type NessaClient,
} from "@nessa/client"
import { expect, it } from "vitest"
import { makeStore } from "../../../store"
import { createDependencies } from "../../../composition/dependencies"
import {
  ControlFailedError,
  SubmissionRefusedError,
  type ControlOutcome,
} from "../../application/ports"
import { textContent, type CommandFailure } from "../../model"
import { conversationNotice } from "../../ui/notification"
import { gatewayEffects } from "../gateway/effects"
import { scenarioEffects } from "../scenario/effects"
import { bindConversation, controlConversation, sendDraft, stopGenerating } from "./slice"

/**
 * What a control's failure becomes on the tab, and what the panel then says.
 *
 * The client has one sentence for every failed control — "Conversation control
 * did not return a trustworthy acknowledgement" — which names neither the
 * command nor its cause, and calls the outcome unknown even where the gateway
 * refused the control outright. So these check both halves: the reason reaches
 * the tab in the panel's own words, and the sentence shown is one that is true
 * of what actually happened.
 */
const CLIENT_CONSTANT =
  "Conversation control did not return a trustworthy acknowledgement"

async function stopAfterFailing(error: unknown) {
  const effects = scenarioEffects("echo")
  const store = makeStore(
    createDependencies({
      conversation: { ...effects, close: () => Promise.reject(error) },
    }),
  )
  await store.dispatch(sendDraft({ content: textContent("hello") }))
  await store.dispatch(stopGenerating({ conversationId: "c0" }))
  return store.getState().conversation.conversations[0]!
}

/** As the gateway adapter builds one: the reason, the outcome, the client's text. */
function controlFailure(reason: CommandFailure | undefined, outcome: ControlOutcome) {
  const failed = new ControlFailedError(reason, outcome)
  failed.message = CLIENT_CONSTANT
  return failed
}

it("says a close whose cleanup failed in the panel's words, by its reason", async () => {
  // Not a refusal: the conversation did close, and only letting go of the files
  // it held did not. The client's constant would call that unknown.
  const tab = await stopAfterFailing(
    controlFailure("attachment-cleanup-unavailable", "unknown"),
  )
  expect(tab.failure).toBe("attachment-cleanup-unavailable")
  expect(tab.error).toMatch(/could not release the images/)
  expect(tab.error).not.toBe(CLIENT_CONSTANT)
  expect(conversationNotice(tab)).toMatchObject({
    title: "Conversation needs attention",
    retry: { kind: "refresh" },
  })
  // The close is over either way: nothing is left pending or mid-cancellation.
  expect(tab.controlPending).toBe(false)
  expect(tab.cancellationStatus).toBeUndefined()
})

it("tells somebody whose control the gateway refused that nothing was done", async () => {
  // The client records these as decided before anything was applied, so saying
  // the acknowledgement could not be trusted would claim the opposite. Every
  // reason answers, including none at all: the outcome is what is being said.
  const startup = await stopAfterFailing(
    controlFailure("agent-startup-deadline", "refused"),
  )
  expect(startup.error).toMatch(/nothing was done/)
  expect(startup.error).not.toBe(CLIENT_CONSTANT)
  expect(conversationNotice(startup)).toEqual({
    title: "Agent was still starting",
    description: startup.error,
    retry: { kind: "refresh" },
  })
  for (const reason of [
    "invalid-request",
    "conversation-not-found",
    "conversation-capacity",
    "agent-not-configured",
    "image-input-unsupported",
    "attachment-not-found",
    "attachment-unavailable",
    "conversation-state-unreadable",
    undefined,
  ] as const) {
    const tab = await stopAfterFailing(controlFailure(reason, "refused"))
    expect(tab.failure).toBe(reason)
    expect(tab.error, `${reason ?? "no reason"} was refused`).toMatch(/nothing was done/)
    expect(tab.error).not.toBe(CLIENT_CONSTANT)
  }
})

it("tells somebody whose choice the gateway recorded that it need not be made again", async () => {
  // The opposite certainty, and the one a flag could not carry: the review's
  // option was consumed, so the choice took effect even though the command
  // failed. Calling that untrustworthy would understate what the gateway said.
  const tab = await stopAfterFailing(controlFailure(undefined, "applied"))
  expect(tab.failure).toBeUndefined()
  expect(tab.error).toMatch(/nothing to answer again/)
  expect(tab.error).not.toBe(CLIENT_CONSTANT)
})

it("keeps the client's sentence where the outcome really is open", async () => {
  // The same reasons, with the gateway saying nothing about what became of the
  // command. "Not a trustworthy acknowledgement" is then exactly right, so it
  // stands — the reason alone never licenses either of the other sentences.
  for (const reason of [
    "agent-startup-deadline",
    "invalid-request",
    "conversation-not-found",
  ] as const) {
    const tab = await stopAfterFailing(controlFailure(reason, "unknown"))
    expect(tab.failure).toBe(reason)
    expect(tab.error).toBe(CLIENT_CONSTANT)
  }
})

it("carries a refused creation's reason through the control that asked for it", async () => {
  // A control opens the conversation first, so a cold agent stops it here, with
  // nothing sent and no failed turn. The whole path runs, through the real
  // adapter: the gateway's code, the client's error, the translation, the notice.
  const client = {
    conversation: {
      create: () =>
        Promise.reject(
          new NessaConversationMutationError(
            "server",
            "action",
            undefined,
            new NessaRpcError(
              ConversationErrorCode.AgentStartupDeadline,
              "agent_startup_deadline",
            ),
            async () => undefined,
          ),
        ),
      // The refresh the control runs before reporting has nothing to read.
      read: () => Promise.reject(new Error("no conversation")),
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
  // Bound but never sent into, so no failed turn can be what answers below.
  store.dispatch(bindConversation({ id: "c0", serverId: "server" }))
  await store.dispatch(stopGenerating({ conversationId: "c0" }))
  const tab = store.getState().conversation.conversations[0]!
  expect(tab.turns).toEqual([])
  expect(tab.failure).toBe("agent-startup-deadline")
  // Creation is a message's refusal, and for this one the client does have a
  // sentence of its own, which names the remedy at length. It is kept.
  expect(tab.error).toMatch(/still starting and ran out of time/)
  expect(conversationNotice(tab)).toEqual({
    title: "Agent was still starting",
    description: tab.error,
    retry: { kind: "refresh" },
  })
})

it.each([
  // The gateway attaches the review's state to an ordinary diagnostic code, so
  // the case that matters most is the one with no word for the reason at all.
  ["audit_unavailable", undefined],
  ["conversation_not_found", "conversation-not-found"],
] as const)(
  "never tells somebody the gateway left a review pending that it could not be trusted, under %s",
  async (code, reason) => {
    // The whole path: the gateway's answer, the client's typed error with its
    // selection state, the adapter's translation, the store, and the notice.
    const client = {
      conversation: {
        create: async () => ({ conversationId: "server" }),
        // Exactly what the client's own `mutate` builds for a failed permission
        // answer: a control error flagged as one, so it reads the gateway's
        // selection state out of the details.
        answer: () =>
          Promise.reject(
            new NessaConversationControlError(
              "server",
              "action",
              "execution",
              new NessaRpcError(code, "answer failed", { selectionState: "pending" }),
              true,
            ),
          ),
        read: () => Promise.reject(new Error("no conversation")),
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
    await store.dispatch(
      controlConversation({
        id: "c0",
        control: {
          kind: "answer",
          executionId: "execution",
          permissionId: "permission",
          optionId: "deny",
        },
      }),
    )
    const tab = store.getState().conversation.conversations[0]!
    expect(tab.failure).toBe(reason)
    // A pending review is the gateway saying the option was not taken. Whether
    // this build has a word for the code beside it changes nothing about that.
    expect(tab.error).toMatch(/nothing was done/)
    expect(tab.error).not.toBe(CLIENT_CONSTANT)
    expect(conversationNotice(tab)).toMatchObject({
      title: "Conversation needs attention",
      description: tab.error,
    })
  },
)

it("leaves an unacknowledged control untyped: a lost answer names no reason", async () => {
  const tab = await stopAfterFailing(new Error("connection lost"))
  expect(tab.failure).toBeUndefined()
  expect(tab.error).toBe("connection lost")
  expect(conversationNotice(tab)).toMatchObject({
    title: "Conversation needs attention",
    description: "connection lost",
    retry: { kind: "refresh" },
  })
})

it("clears the previous failure when the next control starts, reason and sentence together", async () => {
  const effects = scenarioEffects("echo")
  let refuse = true
  const store = makeStore(
    createDependencies({
      conversation: {
        ...effects,
        close: async (id: string) => {
          if (refuse) throw controlFailure("attachment-cleanup-unavailable", "unknown")
          await effects.close(id)
        },
      },
    }),
  )
  await store.dispatch(sendDraft({ content: textContent("hello") }))
  await store.dispatch(stopGenerating({ conversationId: "c0" }))
  expect(store.getState().conversation.conversations[0]!.failure).toBe(
    "attachment-cleanup-unavailable",
  )
  refuse = false
  await store.dispatch(stopGenerating({ conversationId: "c0" }))
  const tab = store.getState().conversation.conversations[0]!
  expect(tab.failure).toBeUndefined()
  expect(tab.error).toBeUndefined()
  expect(conversationNotice(tab)).toBeNull()
})

it("does not tell a refused retry its message is back in the draft, because it is not", async () => {
  // A retry re-sends a turn whose delivery was unknown. Unlike `sendDraft` it
  // acts on nothing when that is refused — the turn keeps its receipt and the
  // draft is untouched — so a message's sentences, every one of which promises
  // the draft is back with its images, would describe what did not happen.
  const effects = scenarioEffects("echo")
  let attempts = 0
  const store = makeStore(
    createDependencies({
      conversation: {
        ...effects,
        send: () => {
          attempts += 1
          // First the acknowledgement is lost, which is what leaves a turn to
          // retry at all. Then the gateway refuses the retry outright.
          if (attempts === 1) return Promise.reject(new Error("acknowledgement lost"))
          const refused = new SubmissionRefusedError("conversation-capacity")
          refused.message = "Conversation command failed"
          return Promise.reject(refused)
        },
      },
    }),
  )
  await store.dispatch(sendDraft({ content: textContent("hello") }))
  const turn = store.getState().conversation.conversations[0]!.turns[0]!
  expect(turn).toMatchObject({ receipt: "unknown" })
  if (turn.from !== "user" || !turn.executionId) throw new Error("no submission identity")
  await store.dispatch(
    controlConversation({
      id: "c0",
      control: { kind: "retry", executionId: turn.executionId },
    }),
  )
  expect(attempts).toBe(2)
  const tab = store.getState().conversation.conversations[0]!
  expect(tab.failure).toBe("conversation-capacity")
  expect(tab.error).not.toMatch(/back in the draft/)
  expect(tab.draft).toEqual([])
  // The turn's own receipt is what says whether it went, and it still says
  // unknown, because nothing about the refused retry changed that.
  expect(tab.turns[0]).toMatchObject({ receipt: "unknown" })
  expect(conversationNotice(tab)).toMatchObject({ title: "Delivery unknown" })
})
