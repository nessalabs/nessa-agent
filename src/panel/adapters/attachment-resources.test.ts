import { afterEach, expect, it, vi } from "vitest"
import {
  createAttachmentResources,
  MAX_SESSION_ATTACHMENT_BYTES,
} from "./attachment-resources"

afterEach(() => vi.restoreAllMocks())

it("keeps binary bytes behind short URLs without reading or encoding them", () => {
  const create = vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:local-file")
  const resources = createAttachmentResources()
  const file = new File(["contents"], "note.txt", { type: "text/plain" })
  const [attachment] = resources.add([file])
  expect(create).toHaveBeenCalledWith(file)
  expect(attachment).toMatchObject({
    name: "note.txt",
    size: 8,
    previewUrl: "blob:local-file",
  })
  expect(JSON.stringify(attachment)).not.toContain("contents")
  expect(JSON.stringify(attachment)).not.toContain("base64")
})

it("bounds all retained conversations and reclaims the budget exactly once", () => {
  const create = vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:full")
  const revoke = vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => {})
  const resources = createAttachmentResources()
  // Metadata-only fixture avoids allocating100MiB just to test admission.
  const file = { name: "full.bin", type: "", size: MAX_SESSION_ATTACHMENT_BYTES } as File
  const [attachment] = resources.add([file])
  expect(resources.canAdd(1)).toBe(false)
  expect(() => resources.add([new File(["x"], "extra.txt")])).toThrow()
  expect(create).toHaveBeenCalledTimes(1)
  resources.retain(new Set([attachment!.id]))
  expect(revoke).not.toHaveBeenCalled()
  resources.retain(new Set())
  resources.retain(new Set())
  expect(revoke).toHaveBeenCalledExactlyOnceWith("blob:full")
  expect(resources.canAdd(MAX_SESSION_ATTACHMENT_BYTES)).toBe(true)
})

it("rolls back a partial URL allocation failure while preserving existing previews", () => {
  const create = vi
    .spyOn(URL, "createObjectURL")
    .mockReturnValueOnce("blob:keep")
    .mockReturnValueOnce("blob:rollback")
    .mockImplementationOnce(() => {
      throw new Error("allocation failed")
    })
  const revoke = vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => {})
  const resources = createAttachmentResources()
  const file = new File(["x"], "one.txt")
  resources.add([file])
  expect(() => resources.add([file, file])).toThrow("allocation failed")
  expect(create).toHaveBeenCalledTimes(3)
  expect(revoke).toHaveBeenCalledExactlyOnceWith("blob:rollback")
  expect(resources.canAdd(MAX_SESSION_ATTACHMENT_BYTES - 1)).toBe(true)
})
