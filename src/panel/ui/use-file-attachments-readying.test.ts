// @vitest-environment jsdom
/**
 * The tile for a file that is not on this disk yet.
 *
 * A file a cloud service is keeping has to be fetched before the agent can be
 * pointed at it, and that takes up to forty-five seconds. The answer arrives at
 * the end, so without a tile the panel says nothing at all while it happens —
 * and silence reads as broken. The host says which file needs a moment as soon
 * as it knows; this is what the panel does with that.
 *
 * Nothing here can produce a real placeholder. What it can do is deliver
 * exactly what the host emits, which has its own test on the other side
 * (`the_panel_is_told_before_the_wait_and_again_when_it_ends`).
 */
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import * as React from "react"
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"

const { onAttachmentReadying, chooseAttachmentFiles, readAttachmentBytes } = vi.hoisted(
  () => ({
    onAttachmentReadying: vi.fn(),
    chooseAttachmentFiles: vi.fn(),
    readAttachmentBytes: vi.fn(),
  }),
)
vi.mock("../../host", () => ({
  onAttachmentReadying,
  chooseAttachmentFiles,
  readAttachmentBytes,
  hasNativeHost: () => true,
}))
vi.mock("../../conversation", () => import("../../conversation/testing"))

import { openConversation, useConversation } from "../../conversation/testing"
import { createDependencies } from "../../composition/dependencies"
import { makeStore } from "../../store"
import { Provider } from "react-redux"
import { scenarioEffects } from "../../conversation/adapters/scenario/effects"
import type { ConversationEffects } from "../../conversation/application/ports"
import { useFileAttachments } from "./use-file-attachments"
import { createAttachmentResources } from "../adapters/attachment-resources"

/** What the host said. Captured so a test can speak as the host. */
let say: (file: { name: string; readying: boolean }) => void
let hook: ReturnType<typeof useFileAttachments>
let chat: ReturnType<typeof useConversation>
let container: HTMLElement
let root: Root
let store: ReturnType<typeof makeStore>

function Surface() {
  chat = useConversation()
  hook = useFileAttachments(chat, createAttachmentResources())
  return null
}

const names = () => hook.pendingFiles.map((file) => file.name)

beforeEach(async () => {
  onAttachmentReadying.mockReset()
  onAttachmentReadying.mockImplementation(
    (handler: (file: { name: string; readying: boolean }) => void) => {
      say = handler
      return Promise.resolve(() => {})
    },
  )
  container = document.createElement("div")
  document.body.append(container)
  store = makeStore(
    createDependencies({
      conversation: scenarioEffects("echo") as unknown as ConversationEffects,
    }),
  )
  root = createRoot(container)
  await act(async () => {
    root.render(
      React.createElement(Provider, {
        store,
        children: React.createElement(Surface),
      }),
    )
  })
})

afterEach(async () => {
  await act(async () => root.unmount())
  container.remove()
})

it("shows a tile for the file that needs a moment, named, as soon as it is told", async () => {
  expect(names()).toEqual([])

  await act(async () => say({ name: "amica-document 2.pdf", readying: true }))

  // Named, not a count: somebody who dropped five needs to know which one.
  expect(names()).toEqual(["amica-document 2.pdf"])
  // And it is not on the draft — it is a file on its way, not an attachment.
  expect(hook.files).toEqual([])
})

it("keeps the tiles apart when several files are being readied at once", async () => {
  await act(async () => {
    say({ name: "first.pdf", readying: true })
    say({ name: "second.pdf", readying: true })
  })
  expect(names()).toEqual(["first.pdf", "second.pdf"])

  // One settling takes its own tile and leaves the other alone. A count could
  // not have done this.
  await act(async () => say({ name: "first.pdf", readying: false }))

  expect(names()).toEqual(["second.pdf"])
})

it("takes the tile away whatever the outcome was, so none is left spinning", async () => {
  // The host says the same thing when a file arrives and when it never will:
  // the sentence for one that is not coming is the refusal's, and a tile still
  // spinning beside that refusal would be the panel disagreeing with itself.
  for (const outcome of ["arrived", "refused"]) {
    await act(async () => say({ name: `${outcome}.pdf`, readying: true }))
    expect(names()).toEqual([`${outcome}.pdf`])

    await act(async () => say({ name: `${outcome}.pdf`, readying: false }))

    expect(names(), outcome).toEqual([])
  }
})

it("refuses a send while a file is still being made ready", async () => {
  expect(hook.isPending(chat.active.id)).toBe(false)

  await act(async () => say({ name: "amica-document 2.pdf", readying: true }))

  // The composer asks this before it lets a draft go, and answers with the
  // words it already has. Sending now would send a message naming a file whose
  // path is not yet worth anything.
  expect(hook.isPending(chat.active.id)).toBe(true)

  await act(async () => say({ name: "amica-document 2.pdf", readying: false }))
  expect(hook.isPending(chat.active.id)).toBe(false)
})

it("leaves the tile with the conversation it was attached to, not the one on screen", async () => {
  const first = chat.active.id
  await act(async () => say({ name: "amica-document 2.pdf", readying: true }))
  expect(names()).toEqual(["amica-document 2.pdf"])

  // A new tab while the file is still coming. The tile belongs to the draft it
  // was dropped on, so this one shows nothing — and the other one still waits.
  await act(async () => {
    store.dispatch(openConversation())
  })

  expect(chat.active.id).not.toBe(first)
  expect(names()).toEqual([])
  expect(hook.isPending(first)).toBe(true)
  expect(hook.isPending(chat.active.id)).toBe(false)

  // And it settles onto the conversation it belonged to rather than the one in
  // front of somebody now.
  await act(async () => say({ name: "amica-document 2.pdf", readying: false }))
  expect(hook.isPending(first)).toBe(false)
})

it("stops listening when the panel goes, so a late answer reaches nothing", async () => {
  await act(async () => say({ name: "amica-document 2.pdf", readying: true }))

  await act(async () => root.unmount())

  // The subscription is torn down, and an answer arriving after it must not
  // set state on a panel that is gone.
  expect(() => say({ name: "amica-document 2.pdf", readying: false })).not.toThrow()
  // Remount for the shared teardown.
  root = createRoot(container)
  await act(async () => {
    root.render(
      React.createElement(Provider, {
        store,
        children: React.createElement(Surface),
      }),
    )
  })
})
