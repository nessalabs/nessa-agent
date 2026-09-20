// @vitest-environment jsdom
/**
 * What pressing send does when the draft cannot go, driven through the form.
 *
 * `sendDraft` refusing a draft whose files cannot go, each with its reason, is
 * tested against the store in `attachments.test.ts`. This is the half that test
 * cannot see: that the composer's submit reaches it. It once returned early for
 * a draft holding files, so the reason was never shown and the panel looked
 * broken.
 */
import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import type { ChatComposerEditorHandle } from "@nessa-ui/react/chat-composer-editor"

// The conversation barrel also exports its components, which need the whole UI
// package resolved. The composer takes two pure functions from it.
vi.mock("../../conversation", () => import("../../conversation/ui/composer-content"))

import { createDependencies } from "../../composition/dependencies"
import { useConversation } from "../../conversation/ui/use-conversation"
import { scenarioEffects } from "../../conversation/adapters/scenario/effects"
import { attachFiles } from "../../conversation/adapters/store/slice"
import { sessionReady } from "../../session/adapters/store/slice"
import { makeStore } from "../../store"
import { useComposer } from "./use-composer"

const file = {
  type: "file" as const,
  id: "f",
  name: "finder.png",
  mimeType: "image/png",
  size: 1,
  previewUrl: "blob:test-file",
  upload: { status: "not-started" as const },
}

/** An editor holding "hello"; file tiles live in the draft, never in the editor. */
const editor: ChatComposerEditorHandle = {
  focus: () => {},
  clear: () => {},
  insertChip: () => {},
  insertText: () => {},
  getContent: () => ({ text: "hello", parts: [{ type: "text", text: "hello" }] }),
}

function Surface({ declines }: { declines: (conversationId: string) => boolean }) {
  const { setComposerRef, submit } = useComposer(useConversation(), declines)
  React.useEffect(() => setComposerRef(editor), [setComposerRef])
  return React.createElement(
    "form",
    { onSubmit: submit },
    React.createElement("button", { type: "submit" }, "Send"),
  )
}

let container: HTMLDivElement
let root: Root

async function pressSend(
  store: ReturnType<typeof makeStore>,
  declines: (conversationId: string) => boolean,
) {
  await React.act(async () => {
    root.render(
      React.createElement(Provider, {
        store,
        children: React.createElement(Surface, { declines }),
      }),
    )
  })
  await React.act(async () => {
    container.querySelector("button")!.click()
  })
}

function storeWithAttachedFile() {
  const send = vi.fn(scenarioEffects("echo").send)
  const store = makeStore(
    createDependencies({ conversation: { ...scenarioEffects("echo"), send } }),
  )
  // Connected, as the panel was: a submit with no gateway never reaches
  // `sendDraft`. Only the phase and the presence of a hello are read.
  store.dispatch(
    sessionReady({ hello: {}, health: {} } as Parameters<typeof sessionReady>[0]),
  )
  store.dispatch(attachFiles({ files: [file], conversationId: "c0" }))
  return { send, store }
}

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  container = document.createElement("div")
  document.body.appendChild(container)
  root = createRoot(container)
})

afterEach(async () => {
  await React.act(async () => root.unmount())
  container.remove()
})

it("shows why a draft whose image has not uploaded did not send, and keeps the draft", async () => {
  const { send, store } = storeWithAttachedFile()
  await pressSend(store, () => false)
  const conversation = store.getState().conversation.conversations[0]!
  expect(conversation.error).toMatch(/still uploading/)
  expect(conversation.draft).toContainEqual(file)
  expect(conversation.turns).toEqual([])
  expect(send).not.toHaveBeenCalled()
})

it("asks the panel to explain a read still in flight, and submits nothing", async () => {
  const { send, store } = storeWithAttachedFile()
  const declines = vi.fn(() => true)
  await pressSend(store, declines)
  expect(declines).toHaveBeenCalledWith("c0")
  // Declined here, so the conversation carries no refusal of its own.
  expect(store.getState().conversation.conversations[0]!.error).toBeUndefined()
  expect(send).not.toHaveBeenCalled()
})

it("shows why a draft holding a file that is not an image did not send", async () => {
  const { send, store } = storeWithAttachedFile()
  const notes = { ...file, id: "n", name: "notes.pdf", mimeType: "application/pdf" }
  store.dispatch(attachFiles({ files: [notes], conversationId: "c0" }))
  await pressSend(store, () => false)
  const conversation = store.getState().conversation.conversations[0]!
  expect(conversation.error).toMatch(/notes\.pdf.*cannot be sent/)
  expect(conversation.draft).toContainEqual(notes)
  expect(conversation.turns).toEqual([])
  expect(send).not.toHaveBeenCalled()
})
