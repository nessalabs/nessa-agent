import { expect, it } from "vitest"
import {
  validDraftAttachments,
  MAX_ATTACHMENT_BYTES,
  type FileAttachment,
} from "./attachments"

it("accepts empty files and exact file limit, rejects invalid byte sizes", () => {
  const file: FileAttachment = {
    type: "file",
    id: "a",
    name: "a.txt",
    mimeType: "text/plain",
    dataUrl: "data:text/plain;base64,",
    size: 0,
  }
  expect(validDraftAttachments([file])).toBe(true)
  expect(validDraftAttachments([{ ...file, size: MAX_ATTACHMENT_BYTES }])).toBe(true)
  for (const size of [-1, 0.5, Number.NaN, Infinity, MAX_ATTACHMENT_BYTES + 1]) {
    expect(validDraftAttachments([{ ...file, size }])).toBe(false)
  }
})
