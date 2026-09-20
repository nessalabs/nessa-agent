import { expect, it } from "vitest"
import { makeStore } from "../../../store"
import { createDependencies } from "../../../composition/dependencies"
import { ControlFailedError, SubmissionRefusedError } from "../../application/ports"
import { textContent } from "../../model"
import { conversationNotice } from "../../ui/notification"
import { scenarioEffects } from "../scenario/effects"
import { bindConversation, sendDraft, stopGenerating } from "./slice"

/**
 * What a control's failure becomes on the tab.
 *
 * The gateway adapter is the only translator — `gateway/effects.test.ts` holds
 * it to that — so what is under test here is the half after it: the reason a
 * control failed reaches the tab in the panel's own words, the sentence is
 * chosen from that reason and not from the message, and a failure with no
 * translated reason still leaves the control's own cleanup and refresh alone.
 */
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

it("says a close whose cleanup failed in the panel's words, by its reason", async () => {
  // Not a refusal: the conversation did close, and only letting go of the files
  // it held did not. The panel has its own sentence for that; the gateway's
  // "did not return a trustworthy acknowledgement" is not it.
  const failed = new ControlFailedError("attachment-cleanup-unavailable")
  failed.message = "Conversation control did not return a trustworthy acknowledgement"
  const tab = await stopAfterFailing(failed)
  expect(tab.failure).toBe("attachment-cleanup-unavailable")
  expect(tab.error).toMatch(/could not release the images/)
  expect(tab.error).not.toBe(failed.message)
  expect(conversationNotice(tab)).toMatchObject({
    title: "Conversation needs attention",
    retry: { kind: "refresh" },
  })
  // The close is over either way: nothing is left pending or mid-cancellation.
  expect(tab.controlPending).toBe(false)
  expect(tab.cancellationStatus).toBeUndefined()
})

it("says a control that found the agent still starting so, keeping the client's sentence", async () => {
  const failed = new ControlFailedError("agent-startup-deadline")
  failed.message = "The agent was still starting and ran out of time."
  const tab = await stopAfterFailing(failed)
  expect(tab.failure).toBe("agent-startup-deadline")
  // No panel sentence for this one: the client's names the remedy at length.
  expect(tab.error).toBe(failed.message)
  expect(conversationNotice(tab)).toEqual({
    title: "Agent was still starting",
    description: failed.message,
    retry: { kind: "refresh" },
  })
})

it("carries a refused creation's reason through the control that asked for it", async () => {
  // A control opens the conversation first, so a cold agent stops it here, with
  // nothing sent and no failed turn. The refusal is a message's — nothing was
  // taken — and the notice must still name the cause.
  const effects = scenarioEffects("echo")
  const refused = new SubmissionRefusedError("agent-startup-deadline")
  refused.message = "The agent was still starting and ran out of time."
  const store = makeStore(
    createDependencies({
      conversation: { ...effects, create: () => Promise.reject(refused) },
    }),
  )
  // Bound but never sent into, so no failed turn can be what answers below.
  store.dispatch(bindConversation({ id: "c0", serverId: "server" }))
  await store.dispatch(stopGenerating({ conversationId: "c0" }))
  const tab = store.getState().conversation.conversations[0]!
  expect(tab.turns).toEqual([])
  expect(tab.failure).toBe("agent-startup-deadline")
  expect(conversationNotice(tab)).toEqual({
    title: "Agent was still starting",
    description: refused.message,
    retry: { kind: "refresh" },
  })
})

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
          if (refuse) throw new ControlFailedError("attachment-cleanup-unavailable")
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
