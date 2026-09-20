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

it("hands back the bytes it retains, and nothing once they are released", () => {
  vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:held")
  vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => {})
  const resources = createAttachmentResources()
  const file = new File(["contents"], "note.txt", { type: "text/plain" })
  const [attachment] = resources.add([file])
  expect(attachment!.upload).toEqual({ status: "not-started" })
  expect(resources.bytes(attachment!.id)).toBe(file)
  resources.retain(new Set())
  expect(resources.bytes(attachment!.id)).toBeUndefined()
})

it("stops counting a file against attaching once its message has been taken, and still keeps it", () => {
  let index = 0
  vi.spyOn(URL, "createObjectURL").mockImplementation(() => `blob:${++index}`)
  const revoke = vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => {})
  const resources = createAttachmentResources()
  // Metadata-only fixtures: a window's worth of originals without allocating it.
  const quarter = MAX_SESSION_ATTACHMENT_BYTES / 4
  const originals = [1, 2, 3, 4].map(
    (name) => ({ name: `${name}.cr3`, type: "", size: quarter }) as File,
  )
  const ids = resources.add(originals).map((file) => file.id)
  expect(resources.canAdd(1)).toBe(false)
  // All four were sent and the gateway has them: drafts are empty.
  resources.retain(new Set(ids), new Set(ids))
  expect(revoke).not.toHaveBeenCalled()
  expect(resources.bytes(ids[0]!)).toBe(originals[0])
  // The whole budget is there for attaching again.
  expect(resources.canAdd(MAX_SESSION_ATTACHMENT_BYTES)).toBe(true)
  expect(resources.canAdd(MAX_SESSION_ATTACHMENT_BYTES + 1)).toBe(false)
  expect(() =>
    resources.add([{ name: "next.cr3", type: "", size: quarter } as File]),
  ).not.toThrow()
})

it("keeps counting a file whose message may yet come back to the draft", () => {
  vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:held")
  vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => {})
  const resources = createAttachmentResources()
  const [sending] = resources.add([
    { name: "a.png", type: "image/png", size: MAX_SESSION_ATTACHMENT_BYTES } as File,
  ])
  // Retained because a turn holds it, but that turn is not taken yet.
  resources.retain(new Set([sending!.id]))
  expect(resources.canAdd(1)).toBe(false)
  // Once sent, always sent: a later pass that does not repeat it changes nothing.
  resources.retain(new Set([sending!.id]), new Set([sending!.id]))
  resources.retain(new Set([sending!.id]))
  expect(resources.canAdd(MAX_SESSION_ATTACHMENT_BYTES)).toBe(true)
})

it("holds a window's worth of camera RAW files: 256 MiB", () => {
  expect(MAX_SESSION_ATTACHMENT_BYTES).toBe(256 * 1024 * 1024)
})
