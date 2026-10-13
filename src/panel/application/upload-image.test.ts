/**
 * One image from attached to staged, driven through its ports.
 *
 * Every wait in `uploadImage` is a promise this file holds the other end of, so
 * "the tile was removed while the image was being hashed" is a line of test
 * rather than a timing. Nothing sleeps.
 */
import { describe, expect, it, vi } from "vitest"

import {
  MAX_UPLOADS_IN_FLIGHT,
  nextUploads,
  uploadImage,
  type UploadPorts,
} from "./upload-image"

const DIGEST = `sha256:${"ab".repeat(32)}`
const file = {
  conversationId: "c0",
  placementId: "f",
  resourceId: "bytes",
  mimeType: "image/heic",
} as const
/** A Blob of a given weight without allocating it: only `size` is read here. */
const blob = (size: number) => ({ size, type: "image/heic" }) as Blob

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((done, fail) => {
    resolve = done
    reject = fail
  })
  return { promise, resolve, reject }
}

/** A signal nobody aborts. */
const live = () => new AbortController().signal

/** Ports over a tiny resource store of one file, which a test can remove. */
function ports(original: Blob | undefined, overrides: Partial<UploadPorts> = {}) {
  let held = original
  const changes: unknown[] = []
  const all: UploadPorts = {
    digest: vi.fn(async () => DIGEST),
    bytes: () => held,
    change: (change) => {
      changes.push(change)
      return true
    },
    stage: vi.fn(async () => {}),
    ...overrides,
  }
  return { all, changes, remove: () => (held = undefined) }
}

it("marks the file uploading, then hashes and stages exactly the original bytes", async () => {
  // 18 MB of HEIC: nothing here scales, converts, or refuses it for its weight.
  const original = blob(18 * 1024 * 1024)
  const { all, changes } = ports(original)
  await uploadImage(file, all, live())
  expect(changes).toEqual([{ conversationId: "c0", placementId: "f", to: "uploading" }])
  expect(all.digest).toHaveBeenCalledExactlyOnceWith(original)
  expect(all.stage).toHaveBeenCalledExactlyOnceWith(
    {
      conversationId: "c0",
      placementId: "f",
      file: { digest: DIGEST, mimeType: "image/heic", size: 18 * 1024 * 1024 },
      bytes: original,
    },
    expect.any(AbortSignal),
  )
  expect(vi.mocked(all.stage).mock.calls[0]![0].bytes).toBe(original)
})

describe("a tile removed while its upload is in flight", () => {
  it("stops after hashing: the digest of a removed file is staged nowhere", async () => {
    const hashing = deferred<string>()
    const reached = deferred<void>()
    const { all, changes, remove } = ports(blob(40_000), {
      digest: () => {
        reached.resolve()
        return hashing.promise
      },
    })
    const running = uploadImage(file, all, live())
    await reached.promise
    remove()
    hashing.resolve(DIGEST)
    await running
    expect(all.stage).not.toHaveBeenCalled()
    // No state is written for a file that is no longer there.
    expect(changes).toEqual([{ conversationId: "c0", placementId: "f", to: "uploading" }])
  })

  it("stops quietly when hashing a removed file's bytes fails", async () => {
    const hashing = deferred<string>()
    const reached = deferred<void>()
    const { all, remove } = ports(blob(40_000), {
      digest: () => {
        reached.resolve()
        return hashing.promise
      },
    })
    const running = uploadImage(file, all, live())
    await reached.promise
    remove()
    hashing.reject(new Error("blob released"))
    await expect(running).resolves.toBeUndefined()
    expect(all.stage).not.toHaveBeenCalled()
  })
})

it.each([
  ["whose bytes are already gone", undefined],
  ["that is empty", blob(0)],
] as const)(
  "fails an image %s as unreadable without staging it",
  async (_name, original) => {
    const { all, changes } = ports(original)
    await uploadImage(file, all, live())
    expect(changes).toEqual([
      { conversationId: "c0", placementId: "f", to: "uploading" },
      { conversationId: "c0", placementId: "f", to: "failed", reason: "unreadable" },
    ])
    expect(all.digest).not.toHaveBeenCalled()
    expect(all.stage).not.toHaveBeenCalled()
  },
)

it("fails as unreadable, and never rejects, when hashing throws", async () => {
  const { all, changes } = ports(blob(40_000), {
    digest: () => Promise.reject(new Error("no subtle crypto")),
  })
  await expect(uploadImage(file, all, live())).resolves.toBeUndefined()
  expect(changes.at(-1)).toEqual({
    conversationId: "c0",
    placementId: "f",
    to: "failed",
    reason: "unreadable",
  })
  expect(all.stage).not.toHaveBeenCalled()
})

describe("how many uploads run at once", () => {
  const files = ["a", "b", "c", "d", "e", "f"].map((id) => ({ placementId: id }))

  it("starts three of six dropped images and leaves the rest waiting", () => {
    expect(MAX_UPLOADS_IN_FLIGHT).toBe(3)
    expect(nextUploads(files, new Set())).toEqual([
      { placementId: "a" },
      { placementId: "b" },
      { placementId: "c" },
    ])
  })

  it("starts the next one, in order, each time a slot frees", () => {
    const waiting = files.slice(3)
    expect(nextUploads(waiting, new Set(["a", "b", "c"]))).toEqual([])
    expect(nextUploads(waiting, new Set(["a", "c"]))).toEqual([{ placementId: "d" }])
    expect(nextUploads(waiting, new Set(["c"]))).toEqual([
      { placementId: "d" },
      { placementId: "e" },
    ])
    expect(nextUploads(waiting, new Set())).toEqual(waiting)
  })

  it("never starts a file that is already in flight, even if it still looks not started", () => {
    // The snapshot is older than the call that started it.
    expect(nextUploads(files.slice(0, 2), new Set(["a"]))).toEqual([{ placementId: "b" }])
    expect(nextUploads(files, new Set(["a", "b", "c", "x"]))).toEqual([])
  })
})

it("sends nothing for a file the conversation did not take to uploading", async () => {
  // A snapshot a render behind offered a file that has since gone, or that
  // another pass already started. Uploading it anyway would spend a gateway
  // slot on a result nobody records.
  const { all } = ports(blob(8), { change: () => false })
  await uploadImage(file, all, live())
  expect(all.digest).not.toHaveBeenCalled()
  expect(all.stage).not.toHaveBeenCalled()
})

it("stages nothing once the tile was taken away while its bytes were being hashed", async () => {
  const hashing = deferred<string>()
  const stopping = new AbortController()
  const { all, changes } = ports(blob(8), { digest: () => hashing.promise })
  const uploading = uploadImage(file, all, stopping.signal)
  stopping.abort()
  hashing.resolve(DIGEST)
  await uploading
  expect(all.stage).not.toHaveBeenCalled()
  expect(changes).toEqual([{ conversationId: "c0", placementId: "f", to: "uploading" }])
})

it("hands the signal that stops it to the transfer", async () => {
  const stopping = new AbortController()
  const { all } = ports(blob(8))
  await uploadImage(file, all, stopping.signal)
  expect(all.stage).toHaveBeenCalledExactlyOnceWith(expect.anything(), stopping.signal)
})

it.each(["success", "failure"] as const)(
  "UP2: %s after hashing preserves the originally captured parent and file",
  async (outcome) => {
    const hashing = deferred<string>()
    const { all, changes } = ports(blob(3), { digest: () => hashing.promise })
    const original: Parameters<typeof uploadImage>[0] = { ...file }
    const pending = uploadImage(original, all, live())
    original.conversationId = "c1"
    original.placementId = "other"
    if (outcome === "success") hashing.resolve(DIGEST)
    else hashing.reject(new Error("hash failed"))
    await pending
    expect(changes[0]).toEqual({
      conversationId: "c0",
      placementId: "f",
      to: "uploading",
    })
    if (outcome === "success")
      expect(all.stage).toHaveBeenCalledWith(
        expect.objectContaining({ conversationId: "c0", placementId: "f" }),
        expect.any(AbortSignal),
      )
    else
      expect(changes.at(-1)).toEqual({
        conversationId: "c0",
        placementId: "f",
        to: "failed",
        reason: "unreadable",
      })
  },
)

function uploadProgressTypeChecks() {
  // @ts-expect-error Progress cannot discard its enclosing conversation.
  const missingParent: Parameters<UploadPorts["change"]>[0] = {
    placementId: "f",
    to: "failed",
    reason: "unreadable",
  }
  return missingParent
}
void uploadProgressTypeChecks
