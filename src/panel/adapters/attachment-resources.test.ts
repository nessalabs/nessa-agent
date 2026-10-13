import { AttachmentResource } from "../../conversation/testing"
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
  expect(create).toHaveBeenCalledWith(resources.bytes(attachment!))
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
  // Native bytes own the budget, rather than a caller-supplied weight.
  const file = new File([new ArrayBuffer(MAX_SESSION_ATTACHMENT_BYTES)], "full.bin")
  const [attachment] = resources.add([file])
  expect(resources.canAdd(1)).toBe(false)
  expect(() => resources.add([new File(["x"], "extra.txt")])).toThrow()
  expect(create).toHaveBeenCalledTimes(1)
  resources.retain({ needed: new Set([attachment!]), previews: new Set() })
  expect(revoke).not.toHaveBeenCalled()
  resources.retain({ needed: new Set(), previews: new Set() })
  resources.retain({ needed: new Set(), previews: new Set() })
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
  expect(resources.bytes(attachment!)?.size).toBe(file.size)
  resources.retain({ needed: new Set(), previews: new Set() })
  expect(resources.bytes(attachment!)).toBeUndefined()
})

it("stops counting a file against attaching once its message has been taken, and still keeps it", () => {
  let index = 0
  vi.spyOn(URL, "createObjectURL").mockImplementation(() => `blob:${++index}`)
  const revoke = vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => {})
  const resources = createAttachmentResources()
  // A window's worth of native originals, measured by their resource values.
  const quarter = MAX_SESSION_ATTACHMENT_BYTES / 4
  const originals = [1, 2, 3, 4].map(
    (name) => new File([new ArrayBuffer(quarter)], `${name}.cr3`),
  )
  const ids = resources.add(originals)
  expect(resources.canAdd(1)).toBe(false)
  // All four were sent and the gateway has them: drafts are empty.
  resources.retain({
    needed: new Set(),
    previews: new Set(ids),
  })
  expect(revoke).not.toHaveBeenCalled()
  expect(resources.bytes(ids[0]!)?.size).toBe(originals[0]!.size)
  // The whole budget is there for attaching again.
  expect(resources.canAdd(MAX_SESSION_ATTACHMENT_BYTES)).toBe(true)
  expect(resources.canAdd(MAX_SESSION_ATTACHMENT_BYTES + 1)).toBe(false)
  expect(() =>
    resources.add([new File([new ArrayBuffer(quarter)], "next.cr3")]),
  ).not.toThrow()
})

it("keeps counting a file whose message may yet come back to the draft", () => {
  vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:held")
  vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => {})
  const resources = createAttachmentResources()
  const [sending] = resources.add([
    new File([new ArrayBuffer(MAX_SESSION_ATTACHMENT_BYTES)], "a.png", {
      type: "image/png",
    }),
  ])
  // Retained because a turn holds it, but that turn is not taken yet.
  resources.retain({ needed: new Set([sending!]), previews: new Set() })
  expect(resources.canAdd(1)).toBe(false)
  // A preview may become needed by another admission; classification is a snapshot.
  resources.retain({
    needed: new Set(),
    previews: new Set([sending!]),
  })
  resources.retain({ needed: new Set([sending!]), previews: new Set() })
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
  expect(resources.bytes(attachment!)).toBeUndefined()
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

it("AL17/AL23: native byte snapshots ignore File shadow hooks and resource copies address no existing bytes", async () => {
  vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:native")
  vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => {})
  const resources = createAttachmentResources()
  const file = new File([Uint8Array.from([1, 2, 3])], "image.png", { type: "image/png" })
  let shadow = 1
  Object.defineProperty(file, "size", { get: () => shadow })
  Object.defineProperty(file, "type", { get: () => "text/plain" })
  file.arrayBuffer = async () => new ArrayBuffer(1)
  const [resource] = resources.add([file])
  expect(resource!.size).toBe(3)
  expect(resource!.mimeType).toBe("image/png")
  expect(resources.bytes(resource!)?.type).toBe("image/png")
  shadow = 0
  expect(resources.bytes(resource!)?.size).toBe(3)
  expect(Reflect.defineProperty(resources.bytes(resource!)!, "size", { value: 1 })).toBe(
    false,
  )
  expect(resources.bytes(resource!)?.size).toBe(3)
  expect([...new Uint8Array(await resources.bytes(resource!)!.arrayBuffer())]).toEqual([
    1, 2, 3,
  ])
  const copy = { ...resource!, size: 1 }
  expect(resources.bytes(copy)).toBeUndefined()
  const minted = AttachmentResource.create(copy)
  expect(resources.bytes(minted)).toBeUndefined()
  resources.retain({ needed: new Set([minted]), previews: new Set() })
  expect(resources.bytes(resource!)).toBeUndefined()
  expect(URL.revokeObjectURL).toHaveBeenCalledExactlyOnceWith("blob:native")
})

it("AL23: retained native SVG bytes preserve their content and media type", async () => {
  vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:svg")
  const resources = createAttachmentResources()
  const text =
    '<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><rect width="4" height="4" fill="red"/></svg>'
  const [resource] = resources.add([
    new File([text], "native.svg", { type: "image/svg+xml" }),
  ])
  expect(resource!.mimeType).toBe("image/svg+xml")
  expect(resources.bytes(resource!)?.type).toBe("image/svg+xml")
  expect(await resources.bytes(resource!)!.text()).toBe(text)
})
