import { expect, it, vi } from "vitest"
import { makeStore } from "../../../store"
import { createDependencies } from "../../../composition/dependencies"
import { attachFiles, sendDraft } from "./slice"

it("rejects preview-file sends without contacting the backend or clearing the draft", async () => {
  const echo = vi.fn(async () => ({ text: "reply" }))
  const store = makeStore(createDependencies({ conversation: { echo } }))
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
    expect(result.payload).toEqual({ kind: "preview-only-files" })
    expect(store.getState().conversation).toEqual(before)
  }
  expect(echo).not.toHaveBeenCalled()
})
