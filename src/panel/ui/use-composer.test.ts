// @vitest-environment jsdom
/**
 * What pressing send does when the draft cannot go, driven through the form.
 *
 * `sendDraft` refusing a draft whose files cannot go, each with its reason, is
 * tested against the store in `attachments.test.ts`. This is the half that test
 * cannot see: that the composer's submit reaches it. A submit that returns
 * early for a draft holding files shows no reason at all, and the panel looks
 * broken while behaving exactly as its store says.
 */
import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import type { ChatComposerEditorHandle } from "@nessa-ui/react/chat-composer-editor"

// The conversation barrel also exports its components, which need the whole UI
// package resolved. Its `testing` entry is the same pure functions with no
// component among them, so the barrel is mocked with that: one definition.
vi.mock("../../conversation", () => import("../../conversation/testing"))

import { createDependencies } from "../../composition/dependencies"
import { attachFiles, scenarioEffects, useConversation } from "../../conversation/testing"
import { sessionReady } from "../../session/testing"
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

const sent = vi.fn()

function Surface({
  declines,
  mounted = editor,
}: {
  declines: (conversationId: string) => boolean
  mounted?: ChatComposerEditorHandle | null
}) {
  const { setComposerRef, submit } = useComposer(useConversation(), declines, sent)
  React.useEffect(() => setComposerRef(mounted), [setComposerRef, mounted])
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
  mounted?: ChatComposerEditorHandle | null,
) {
  await React.act(async () => {
    root.render(
      React.createElement(Provider, {
        store,
        children: React.createElement(Surface, { declines, mounted }),
      }),
    )
  })
  await React.act(async () => {
    container.querySelector("button")!.click()
  })
}

function storeWithAttachedFile(connected = true) {
  const send = vi.fn(scenarioEffects("echo").send)
  const store = makeStore(
    createDependencies({ conversation: { ...scenarioEffects("echo"), send } }),
  )
  // Connected, as the panel usually is. Only the phase and the presence of a
  // hello are read.
  if (connected)
    store.dispatch(
      sessionReady({ hello: {}, health: {} } as Parameters<typeof sessionReady>[0]),
    )
  store.dispatch(attachFiles({ files: [file], conversationId: "c0" }))
  return { send, store }
}

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  sent.mockClear()
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
  // Nothing left the composer, so nothing downstream is told that it did. The
  // panel puts down what it was saying about the draft's files on this, and a
  // draft that is still here has not answered anything.
  expect(sent).not.toHaveBeenCalled()
})

it("names the conversation a draft has actually gone from, and only then", async () => {
  const store = makeStore(createDependencies({ conversation: scenarioEffects("echo") }))
  store.dispatch(
    sessionReady({ hello: {}, health: {} } as Parameters<typeof sessionReady>[0]),
  )
  await pressSend(store, () => false)
  expect(store.getState().conversation.conversations[0]!.draft).toEqual([])
  expect(sent).toHaveBeenCalledExactlyOnceWith("c0")
})

it("does not say a draft has gone when the panel itself turned the send away", async () => {
  const { store } = storeWithAttachedFile()
  await pressSend(store, () => true)
  expect(sent).not.toHaveBeenCalled()
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

it("says why nothing was sent when there is no session yet, and keeps the draft", async () => {
  // Idle, connecting, or ready without a hello: the notification above the
  // composer says nothing for these, so a silent return here was a dead button.
  const { send, store } = storeWithAttachedFile(false)
  await pressSend(store, () => false)
  const conversation = store.getState().conversation.conversations[0]!
  expect(conversation.error).toMatch(
    /Not connected to the gateway yet.*draft has been kept/,
  )
  expect(conversation.draft).toContainEqual(file)
  expect(conversation.turns).toEqual([])
  expect(send).not.toHaveBeenCalled()
})

it("still reaches sendDraft, with the draft's own prose, when the editor is between mounts", async () => {
  const send = vi.fn(scenarioEffects("echo").send)
  const store = makeStore(
    createDependencies({ conversation: { ...scenarioEffects("echo"), send } }),
  )
  store.dispatch(
    sessionReady({ hello: {}, health: {} } as Parameters<typeof sessionReady>[0]),
  )
  const { setDraft } = await import("../../conversation/testing")
  store.dispatch(
    setDraft({ draft: [{ type: "text", text: "typed before the remount" }] }),
  )
  await pressSend(store, () => false, null)
  expect(send).toHaveBeenCalledExactlyOnceWith(
    expect.objectContaining({ text: "typed before the remount" }),
  )
})
