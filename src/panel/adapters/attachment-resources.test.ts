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
  resources.retain({ needed: new Set([attachment!.resourceId]), previews: new Map() })
  expect(revoke).not.toHaveBeenCalled()
  resources.retain({ needed: new Set(), previews: new Map() })
  resources.retain({ needed: new Set(), previews: new Map() })
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
  expect(attachment).not.toHaveProperty("placementId")
  expect(attachment).not.toHaveProperty("upload")
  expect(resources.bytes(attachment!.resourceId)).toBe(file)
  resources.retain({ needed: new Set(), previews: new Map() })
  expect(resources.bytes(attachment!.resourceId)).toBeUndefined()
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
  const ids = resources.add(originals).map((file) => file.resourceId)
  expect(resources.canAdd(1)).toBe(false)
  // All four were sent and the gateway has them: drafts are empty.
  resources.retain({
    needed: new Set(),
    previews: new Map(ids.map((id) => [id, quarter])),
  })
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
  resources.retain({ needed: new Set([sending!.resourceId]), previews: new Map() })
  expect(resources.canAdd(1)).toBe(false)
  // A preview may become needed by another admission; classification is a snapshot.
  resources.retain({
    needed: new Set(),
    previews: new Map([[sending!.resourceId, MAX_SESSION_ATTACHMENT_BYTES]]),
  })
  resources.retain({ needed: new Set([sending!.resourceId]), previews: new Map() })
  expect(resources.canAdd(1)).toBe(false)
})

it("holds a window's worth of camera RAW files: 256 MiB", () => {
  expect(MAX_SESSION_ATTACHMENT_BYTES).toBe(256 * 1024 * 1024)
})

it("takes a chosen file by its path, holding none of it and spending no budget", () => {
  const create = vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:unused")
  const resources = createAttachmentResources()
  // Far past every byte bound this store has, and it does not matter: the
  // window is holding a path, not a video.
  const huge = 700 * 1024 * 1024
  const [attachment] = resources.addChosen([
    {
      path: "/Users/ada/holiday.mp4",
      name: "holiday.mp4",
      size: huge,
      mimeType: "video/mp4",
      ticket: "t1",
    },
  ])
  expect(attachment).toMatchObject({
    name: "holiday.mp4",
    size: huge,
    path: "/Users/ada/holiday.mp4",
    previewUrl: "",
  })
  // Nothing was read, so nothing was allocated and nothing can be read back.
  expect(create).not.toHaveBeenCalled()
  expect(resources.bytes(attachment!.resourceId)).toBeUndefined()
  expect(resources.canAdd(MAX_SESSION_ATTACHMENT_BYTES)).toBe(true)
})

it("gives a file with no bytes read no claim to an encoding", () => {
  const resources = createAttachmentResources()
  // The platform's own answer is carried through, in the same slot a dropped
  // file's `type` occupies, so one naming rule sees the same inputs whichever
  // route a file arrived by.
  const [typed] = resources.addChosen([
    {
      path: "/Users/ada/report.pdf",
      name: "report.pdf",
      size: 10,
      mimeType: "application/pdf",
      ticket: "t2",
    },
  ])
  expect(typed!.mimeType).toBe("application/pdf")
  // And a platform with no answer looks exactly like a browser with none, so
  // the extension fallback gets its turn at attach time rather than here.
  const [untyped] = resources.addChosen([
    { path: "/Users/ada/odd.bin", name: "odd.bin", size: 10, mimeType: "", ticket: "t3" },
  ])
  expect(untyped!.mimeType).toBe("application/octet-stream")
})
