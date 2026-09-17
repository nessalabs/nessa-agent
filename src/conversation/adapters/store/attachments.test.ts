import { expect, it, vi } from "vitest"
import { makeStore } from "../../../store"
import { createDependencies } from "../../../composition/dependencies"
import { scenarioEffects } from "../scenario/effects"
import { attachFiles, sendDraft } from "./slice"

it("rejects preview-file sends without contacting the backend or clearing the draft", async () => {
  const send = vi.fn(scenarioEffects("echo").send)
  const store = makeStore(
    createDependencies({ conversation: { ...scenarioEffects("echo"), send } }),
  )
  const file = {
    type: "file" as const,
    id: "f",
    name: "a.txt",
    mimeType: "text/plain",
    size: 1,
    previewUrl: "blob:test-file",
  }
  store.dispatch(attachFiles({ files: [file], conversationId: "c0" }))
  const before = store.getState().conversation
  for (const content of [
    [file],
    [{ type: "text" as const, text: "hello" }, file],
    [{ type: "text" as const, text: "hello" }],
  ]) {
    const result = await store.dispatch(sendDraft({ content, id: "c0" }))
    expect(result.meta.requestStatus).toBe("rejected")
    expect(store.getState().conversation.conversations[0]!.draft).toEqual(
      before.conversations[0]!.draft,
    )
    expect(store.getState().conversation.conversations[0]!.error).toMatch(/preview-only/)
  }
  expect(send).not.toHaveBeenCalled()
})
