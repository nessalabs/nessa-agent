import { expect, it } from "vitest"
import { conversation } from "../../model"
import type { ConversationView } from "../view"
import { applyView } from "./apply-view"

const failureNotice =
  "The agent provider reported an error: OpenCode's free tier can only be used from within OpenCode. The turn could not complete all required work."

const providerFailure = (revision: string): ConversationView => ({
  conversationId: "server",
  revision,
  truncated: false,
  queueComplete: true,
  messages: [
    {
      executionId: "execution",
      userText: "Hello",
      attachments: [],
      files: [],
      parts: [],
      status: "failed",
      error: failureNotice,
    },
  ],
  pending: [],
  permissions: [],
  tools: [],
  capabilities: {
    queue: true,
    steer: true,
    resume: false,
    permissions: true,
    imageInput: false,
    agentFeatures: {
      permissionDenial: "unknown",
      nativeHookSuppression: "unknown",
      compactionReporting: "unsupported_not_implemented",
      modelSwitchReporting: "unsupported_not_implemented",
      permissionDeferral: "unsupported_not_implemented",
      elicitationForwarding: "unknown",
      preToolPolicy: "unsupported_not_implemented",
      policyEndTurn: "unsupported_not_implemented",
      policyCloseSession: "unsupported_not_implemented",
      incomingElicitation: "unsupported_not_implemented",
    },
  },
})

const protocolFailure = (): ConversationView => {
  const view = providerFailure("terminal:protocol")
  return {
    ...view,
    messages: [
      {
        ...view.messages[0]!,
        parts: [{ offset: 0, kind: "tool", text: "", toolId: "tool", noticeId: "" }],
        error: "The turn could not complete all required work.",
      },
    ],
    tools: [
      {
        executionId: "execution",
        toolId: "tool",
        title: "Shell",
        status: "running",
        input: "",
        details: "partial output",
      },
    ],
  }
}

it("settles a provider prompt failure and repeated replacement views do not revive it", () => {
  const failed = applyView(conversation("tab"), providerFailure("terminal:1"))

  expect(failed.phase).toBe("idle")
  expect(failed.remote?.running).toBe(false)
  expect(failed.turns).toEqual([
    expect.objectContaining({
      from: "user",
      executionId: "execution",
      receipt: "delivered",
    }),
    expect.objectContaining({
      from: "assistant",
      executionId: "execution",
      status: failureNotice,
    }),
  ])

  const repeated = applyView(failed, providerFailure("terminal:2"))
  expect(repeated.phase).toBe("idle")
  expect(repeated.remote?.running).toBe(false)
  expect(
    repeated.turns.filter(
      (turn) => turn.from === "assistant" && turn.executionId === "execution",
    ),
  ).toHaveLength(1)
})

it("stops thinking after a mid-turn protocol failure without rewriting tool evidence", () => {
  const failed = applyView(conversation("tab"), protocolFailure())

  expect(failed.phase).toBe("idle")
  expect(failed.remote?.running).toBe(false)
  expect(failed.remote?.tools).toEqual([
    expect.objectContaining({ toolId: "tool", status: "running" }),
  ])
  expect(failed.turns[1]).toMatchObject({
    from: "assistant",
    thought: "",
    status: "The turn could not complete all required work.",
  })
})
