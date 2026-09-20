// @vitest-environment jsdom
/**
 * When an upload starts, driven through the mounted hook against the real store.
 *
 * The order of one upload is `upload-image.test.ts`'s; what the store does with
 * its result is `attachments.test.ts`'s. This is the part only React can run:
 * that attaching an image starts exactly one upload under Strict Mode, which
 * runs the effect twice; that a tile's state follows the gateway's answer; that
 * retry starts another; and that a tile removed mid-upload stays removed.
 */
import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it, vi } from "vitest"

// The conversation barrel also exports its components, which need the whole UI
// package resolved. Its `testing` entry has the same `isImageFile` and no
// component, so the barrel is mocked with that rather than with a copy of a rule.
vi.mock("../../conversation", () => import("../../conversation/testing"))

import { createDependencies } from "../../composition/dependencies"
import {
  attachFiles,
  AttachmentStagingError,
  removeFile,
  scenarioEffects,
  useConversation,
} from "../../conversation/testing"
import { makeStore } from "../../store"
import { useAttachmentUploads } from "./use-attachment-uploads"

const DIGEST = `sha256:${"ab".repeat(32)}`

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((done, fail) => {
    resolve = done
    reject = fail
  })
  return { promise, resolve, reject }
}

/** What the gateway stored: not the file that was attached, in any field. */
const stored = {
  digest: `sha256:${"ef".repeat(32)}`,
  mimeType: "image/jpeg" as const,
  size: 2,
}

let retry: (fileId: string) => void = () => {}
function Surface({
  resources,
}: {
  resources: Parameters<typeof useAttachmentUploads>[1]
}) {
  const uploads = useAttachmentUploads(useConversation(), resources, async () => DIGEST)
  retry = uploads.retry
  return null
}

let container: HTMLDivElement
let root: Root
// jsdom has no object URLs at all, so there is nothing to spy on: these stand in
// for the two functions and are taken away again after each test.
const objectUrls = URL as unknown as {
  createObjectURL?: (bytes: Blob) => string
  revokeObjectURL?: (url: string) => void
}

/** A mounted panel whose gateway's staging answer the test controls. */
async function mounted(
  stageAttachment: (...staged: unknown[]) => Promise<typeof stored>,
) {
  const stage = vi.fn(stageAttachment)
  const dependencies = createDependencies({
    conversation: { ...scenarioEffects("echo"), stageAttachment: stage },
  })
  const store = makeStore(dependencies)
  await React.act(async () => {
    root.render(
      React.createElement(
        React.StrictMode,
        null,
        React.createElement(Provider, {
          store,
          children: React.createElement(Surface, { resources: dependencies.attachments }),
        }),
      ),
    )
  })
  const attach = async (file: File) => {
    const [attachment] = dependencies.attachments.add([file])
    await React.act(async () => {
      store.dispatch(attachFiles({ files: [attachment!], conversationId: "c0" }))
    })
    return attachment!.id
  }
  const uploadOf = (id: string) => {
    const part = store
      .getState()
      .conversation.conversations[0]!.draft.find(
        (part) => part.type === "file" && part.id === id,
      )
    return part?.type === "file" ? part.upload : undefined
  }
  return { store, stage, attach, uploadOf, dependencies }
}

// A format no message names: it is uploaded all the same, and the gateway decides.
const heic = () => new File(["raw"], "finder.heic", { type: "image/heic" })

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  objectUrls.createObjectURL = vi.fn(() => "blob:test")
  objectUrls.revokeObjectURL = vi.fn()
  container = document.createElement("div")
  document.body.appendChild(container)
  root = createRoot(container)
})

afterEach(async () => {
  await React.act(async () => root.unmount())
  container.remove()
  delete objectUrls.createObjectURL
  delete objectUrls.revokeObjectURL
})

it("uploads an attached image's original bytes once, and records what the gateway stored", async () => {
  const panel = await mounted(async () => stored)
  const original = heic()
  const id = await panel.attach(original)
  expect(panel.stage).toHaveBeenCalledOnce()
  expect(panel.stage.mock.calls[0]).toEqual([
    panel.store.getState().conversation.conversations[0]!.serverConversationId,
    { digest: DIGEST, mimeType: "image/heic", size: 3 },
    original,
    expect.any(AbortSignal),
  ])
  // The returned reference, not the digest computed here.
  expect(panel.uploadOf(id)).toEqual({ status: "stored", image: stored })
})

it("uploads nothing for a file no message can carry", async () => {
  const panel = await mounted(async () => stored)
  const id = await panel.attach(
    new File(["%PDF"], "notes.pdf", { type: "application/pdf" }),
  )
  expect(panel.stage).not.toHaveBeenCalled()
  expect(panel.uploadOf(id)).toEqual({ status: "not-started" })
})

it("shows the upload in flight, then what the gateway said, then retries on request", async () => {
  const first = deferred<typeof stored>()
  const answers = [first.promise, Promise.resolve(stored)]
  const panel = await mounted(() => answers.shift()!)
  const id = await panel.attach(heic())
  expect(panel.uploadOf(id)).toEqual({ status: "uploading" })
  await React.act(async () => first.reject(new AttachmentStagingError("unavailable")))
  expect(panel.uploadOf(id)).toEqual({ status: "failed", reason: "unavailable" })
  expect(panel.stage).toHaveBeenCalledOnce()
  await React.act(async () => retry(id))
  expect(panel.stage).toHaveBeenCalledTimes(2)
  expect(panel.uploadOf(id)).toEqual({ status: "stored", image: stored })
})

it("leaves a tile removed mid-upload removed when the upload resolves", async () => {
  const gate = deferred<typeof stored>()
  const panel = await mounted(() => gate.promise)
  const id = await panel.attach(heic())
  expect(panel.uploadOf(id)).toEqual({ status: "uploading" })
  await React.act(async () => {
    panel.store.dispatch(removeFile(id))
  })
  expect(URL.revokeObjectURL).toHaveBeenCalledExactlyOnceWith("blob:test")
  await React.act(async () => gate.resolve(stored))
  expect(panel.store.getState().conversation.conversations[0]!.draft).toEqual([])
  expect(panel.uploadOf(id)).toBeUndefined()
  // Nothing was allocated again for it, and nothing started a second upload.
  expect(URL.createObjectURL).toHaveBeenCalledOnce()
  expect(panel.stage).toHaveBeenCalledOnce()
})

it("stops a removed tile's upload and gives its slot to the next image at once", async () => {
  // Four images, three slots. The signals are what the gateway adapter is given.
  const signals: AbortSignal[] = []
  const gates = Array.from({ length: 4 }, () => deferred<typeof stored>())
  const panel = await mounted((...staged: unknown[]) => {
    signals.push(staged[3] as AbortSignal)
    return gates[signals.length - 1]!.promise
  })
  const ids: string[] = []
  for (let index = 0; index < 4; index++)
    ids.push(
      await panel.attach(new File(["raw"], `${index}.heic`, { type: "image/heic" })),
    )
  expect(panel.stage).toHaveBeenCalledTimes(3)
  expect(panel.uploadOf(ids[3]!)?.status).toBe("not-started")

  // The first tile is taken away while its bytes are still going. Nothing about
  // that upload has finished: the fourth starts because the slot was given back.
  await React.act(async () => {
    panel.store.dispatch(removeFile(ids[0]!))
  })
  expect(signals[0]!.aborted).toBe(true)
  expect(signals.slice(1, 3).map((signal) => signal.aborted)).toEqual([false, false])
  expect(panel.stage).toHaveBeenCalledTimes(4)
  expect(panel.uploadOf(ids[3]!)?.status).toBe("uploading")

  // The stopped upload ending later takes nobody's slot and writes nothing.
  await React.act(async () => gates[0]!.reject(new AttachmentStagingError("interrupted")))
  expect(panel.uploadOf(ids[0]!)).toBeUndefined()
  expect(panel.stage).toHaveBeenCalledTimes(4)
  for (const gate of gates.slice(1)) await React.act(async () => gate.resolve(stored))
  expect(ids.slice(1).map((id) => panel.uploadOf(id)?.status)).toEqual([
    "stored",
    "stored",
    "stored",
  ])
})

it("keeps three uploads in flight and starts the next as each one finishes", async () => {
  // Six images dropped at once. The gateway takes four uploads at a time, so a
  // window that started all six would have the rest refused for want of room.
  const gates = Array.from({ length: 6 }, () => deferred<typeof stored>())
  let next = 0
  const panel = await mounted(() => gates[next++]!.promise)
  const ids: string[] = []
  for (let index = 0; index < 6; index++)
    ids.push(
      await panel.attach(new File(["raw"], `${index}.heic`, { type: "image/heic" })),
    )
  expect(panel.stage).toHaveBeenCalledTimes(3)
  expect(ids.map((id) => panel.uploadOf(id)?.status)).toEqual([
    "uploading",
    "uploading",
    "uploading",
    // In line, and shown as waiting on their tiles.
    "not-started",
    "not-started",
    "not-started",
  ])
  // One finishes; exactly one more starts. Nothing about the waiting images'
  // state changed, so only the upload finishing can be what started it.
  await React.act(async () => gates[0]!.resolve(stored))
  expect(panel.stage).toHaveBeenCalledTimes(4)
  expect(panel.uploadOf(ids[3]!)?.status).toBe("uploading")
  expect(panel.uploadOf(ids[4]!)?.status).toBe("not-started")
  // A failure frees a slot just as a success does.
  await React.act(async () => gates[1]!.reject(new AttachmentStagingError("busy")))
  expect(panel.stage).toHaveBeenCalledTimes(5)
  for (const gate of gates.slice(2)) await React.act(async () => gate.resolve(stored))
  expect(panel.stage).toHaveBeenCalledTimes(6)
  expect(ids.map((id) => panel.uploadOf(id)?.status)).toEqual([
    "stored",
    "failed",
    "stored",
    "stored",
    "stored",
    "stored",
  ])
})

it("does not spin on an image whose upload ends before React has rendered at all", async () => {
  // Its bytes are gone, so it fails in the same tick it starts, while the last
  // rendered draft still shows it as not started. Started again from that stale
  // picture it would fail the same way, at once, for ever.
  const panel = await mounted(async () => stored)
  const change = vi.spyOn(panel.store, "dispatch")
  const [attachment] = panel.dependencies.attachments.add([
    new File(["raw"], "gone.heic", { type: "image/heic" }),
  ])
  panel.dependencies.attachments.retain(new Set())
  await React.act(async () => {
    panel.store.dispatch(attachFiles({ files: [attachment!], conversationId: "c0" }))
  })
  expect(panel.uploadOf(attachment!.id)).toEqual({
    status: "failed",
    reason: "unreadable",
  })
  expect(panel.stage).not.toHaveBeenCalled()
  // Attach, uploading, failed — and then it stops.
  expect(change.mock.calls.length).toBeLessThan(10)
})
