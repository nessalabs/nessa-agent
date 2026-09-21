// @vitest-environment jsdom
/**
 * An image attached to an agent whose model takes none.
 *
 * What happened before: the image attached, its bytes went up in full, the
 * gateway refused them, and the tile turned red saying the upload failed. The
 * upload had not failed — it was never going to work, and that was knowable
 * before a single byte moved. A user on Codex hit exactly this by dragging an
 * image off a web page.
 *
 * Three states, and the third is the one worth being careful about.
 * `imageInput` is the gateway's answer for this conversation's model: `true`
 * takes images, `false` does not, and `undefined` means it has not answered
 * yet. Unknown is not a no. An image refused because the answer had not
 * arrived would be refused on a gateway that was about to say yes, which is
 * the mistake the send path has deliberately avoided since images shipped.
 */
import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it, vi } from "vitest"

vi.mock("../../conversation", () => import("../../conversation/testing"))
vi.mock("../../host", () => ({
  chooseAttachmentFiles: () => Promise.resolve(null),
  readAttachmentBytes: () => Promise.resolve(null),
  hasNativeHost: () => true,
  onAttachmentReadying: () => Promise.resolve(() => {}),
}))

import { createDependencies } from "../../composition/dependencies"
import {
  attachFiles,
  bindConversation,
  refreshConversation,
  scenarioEffects,
  useConversation,
  type ConversationView,
} from "../../conversation/testing"
import { makeStore } from "../../store"
import { useAttachmentUploads } from "./use-attachment-uploads"
import { useFileAttachments } from "./use-file-attachments"

const DIGEST = `sha256:${"ab".repeat(32)}`
const stored = { digest: `sha256:${"ef".repeat(32)}`, mimeType: "image/jpeg", size: 2 }

let hook: ReturnType<typeof useFileAttachments>
function Surface({ resources }: { resources: never }) {
  const chat = useConversation()
  hook = useFileAttachments(chat, resources)
  useAttachmentUploads(chat, resources, async () => DIGEST)
  return null
}

let container: HTMLDivElement
let root: Root
const objectUrls = URL as unknown as {
  createObjectURL?: (bytes: Blob) => string
  revokeObjectURL?: (url: string) => void
}

/** A panel whose gateway answers `imageInput` however the test says. */
async function mounted(imageInput: boolean | undefined) {
  const echo = scenarioEffects("echo")
  const stage = vi.fn(async () => stored)
  const dependencies = createDependencies({
    conversation: {
      ...echo,
      stageAttachment: stage as never,
      read: async (id: string) => {
        const view = (await echo.read(id)) as ConversationView
        return {
          ...view,
          capabilities: { ...view.capabilities, imageInput },
        } as ConversationView
      },
    } as never,
  })
  const store = makeStore(dependencies)
  await React.act(async () => {
    root.render(
      React.createElement(Provider, {
        store,
        children: React.createElement(Surface, {
          resources: dependencies.attachments as never,
        }),
      }),
    )
  })
  // The gateway's answer has to have arrived for any of this to be about it,
  // and a conversation is only read once it is bound to one on the gateway.
  await echo.create("c0")
  await React.act(async () => {
    store.dispatch(bindConversation({ id: "c0", serverId: "c0" }))
    await store.dispatch(refreshConversation("c0"))
  })
  const draft = () => store.getState().conversation.conversations[0]!.draft
  return { store, stage, draft, dependencies }
}

const screenshot = () => new File(["png"], "screenshot.png", { type: "image/png" })

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

it("refuses an image for a model that takes none, before spending an upload", async () => {
  const { stage, draft } = await mounted(false)

  await React.act(async () => hook.addFiles([screenshot()]))

  // Nothing on the draft, so no tile — and in particular no red one claiming
  // an upload failed when none was ever attempted.
  expect(draft()).toEqual([])
  expect(stage).not.toHaveBeenCalled()
  // And the reason is the one the draft notice already uses for this fact,
  // said now rather than after a wasted round trip.
  expect(hook.refusal).toEqual({ reason: "images-not-supported" })
})

it("takes the same image when the gateway has not answered yet", async () => {
  // The care this rule turns on. Refusing here would refuse an image on a
  // gateway that was about to say yes — and would keep refusing it.
  const { stage, draft } = await mounted(undefined)

  await React.act(async () => hook.addFiles([screenshot()]))

  expect(draft()).toHaveLength(1)
  expect(hook.refusal).toBe(null)
  await React.act(async () => {})
  expect(stage).toHaveBeenCalled()
})

it("takes it, and uploads it, for a model that does", async () => {
  const { stage, draft } = await mounted(true)

  await React.act(async () => hook.addFiles([screenshot()]))

  expect(draft()).toHaveLength(1)
  expect(hook.refusal).toBe(null)
  await React.act(async () => {})
  expect(stage).toHaveBeenCalled()
})

it("starts no upload for an image sitting on a draft the answer says no to", async () => {
  // The case attaching cannot catch. Attaching refuses an image outright for a
  // conversation that has already said no, so what is left is a file that got
  // onto a draft some other way — restored with the tab, or put there while
  // the answer was still unknown and still waiting for a slot when it came.
  // Spending its upload would cost the window's bytes and one of the gateway's
  // slots for a refusal that is already certain, and would land the tile in the
  // red that means something went wrong.
  const { store, stage, draft, dependencies } = await mounted(false)

  // Straight onto the draft, past the attach-time refusal, which is what makes
  // this about the upload driver rather than about attaching.
  const [attachment] = dependencies.attachments.add([screenshot()])
  await React.act(async () => {
    store.dispatch(attachFiles({ files: [attachment!], conversationId: "c0" }))
  })
  await React.act(async () => {})

  expect(draft()).toHaveLength(1)
  const file = draft()[0]
  expect(file?.type === "file" && file.upload.status).toBe("not-started")
  expect(stage).not.toHaveBeenCalled()
})

it("still uploads one sitting on a draft nobody has answered for", async () => {
  // The same shape with the third state, so the guard above cannot quietly
  // become "refuse unless told yes".
  const { store, stage, dependencies } = await mounted(undefined)

  const [attachment] = dependencies.attachments.add([screenshot()])
  await React.act(async () => {
    store.dispatch(attachFiles({ files: [attachment!], conversationId: "c0" }))
  })
  await React.act(async () => {})

  expect(stage).toHaveBeenCalled()
})
