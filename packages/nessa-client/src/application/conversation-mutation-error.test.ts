import { expect, it } from "vitest"
import {
  ConversationErrorCode,
  NessaConversationControlError,
  NessaConversationMutationError,
  NessaRpcError,
} from "../index.js"

/** The rows in docs/design/pre-save-revoked-refusal-860.md. */
const rows = [
  {
    wire: "revoked",
    code: ConversationErrorCode.Revoked,
    uncertain: false,
    message:
      "Access changed before the message was admitted, so nothing was saved. Sending it again needs a new message.",
  },
  {
    wire: "revoked_after_save",
    code: ConversationErrorCode.RevokedAfterSave,
    uncertain: true,
    message:
      "The message was saved, then refused because its access changed before it reached the agent. Sending it again needs a new message.",
  },
  {
    wire: "unavailable_after_save",
    code: ConversationErrorCode.UnavailableAfterSave,
    uncertain: true,
    message:
      "The message was saved, then could not be checked because its access record could not be read. Sending it again needs a new message.",
  },
] as const

it("explains a pre-save revoked refusal apart from both refusals after save", async () => {
  const sentences = new Set<string>()
  for (const row of rows) {
    let retried = 0
    const error = new NessaConversationMutationError(
      "c",
      "r",
      "x",
      new NessaRpcError(row.wire, "access changed"),
      async () => {
        retried += 1
        return row.wire
      },
    )
    expect(error.code).toBe(row.code)
    expect(error.uncertain).toBe(row.uncertain)
    expect(error.message).toBe(row.message)
    expect(error.conversationId).toBe("c")
    expect(error.requestId).toBe("r")
    expect(error.executionId).toBe("x")
    expect(error.cause).toMatchObject({ code: row.wire, message: "access changed" })
    expect(await error.retry()).toBe(row.wire)
    expect(retried).toBe(1)
    expect(error.conversationId).toBe("c")
    expect(error.requestId).toBe("r")
    expect(error.executionId).toBe("x")
    sentences.add(error.message)

    const control = new NessaConversationControlError(
      "c",
      "r",
      "x",
      new NessaRpcError(row.wire, "access changed"),
    )
    expect(control.code).toBe(row.code)
    expect(control.uncertain).toBe(row.uncertain)
    expect(control.message).toBe(row.message)
    expect(control.conversationId).toBe("c")
    expect(control.requestId).toBe("r")
    expect(control.executionId).toBe("x")
  }
  expect(sentences.size).toBe(rows.length)

  const unknown = new NessaConversationMutationError(
    "c",
    "r",
    "x",
    new NessaRpcError("invented_code", "access changed"),
    async () => "no",
  )
  expect(unknown.code).toBeUndefined()
  expect(unknown.uncertain).toBe(true)
  expect(unknown.message).toBe("Conversation command failed")
})
