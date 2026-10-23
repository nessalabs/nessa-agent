// @vitest-environment jsdom
import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import type { useConversation } from "../../conversation"
import { useComposer } from "./use-composer"

function deferred() {
  let resolve!: (taken: boolean) => void
  const promise = new Promise<boolean>((done) => {
    resolve = done
  })
  return { promise, resolve }
}
let root: Root
let container: HTMLDivElement
let composer!: ReturnType<typeof useComposer>
const sent = vi.fn()
function Surface({
  id,
  text,
  submit,
}: {
  id: string
  text: string
  submit: () => Promise<boolean>
}) {
  const chat = {
    active: { id, draft: text ? [{ type: "text", text }] : [] },
    submit,
    setDraft: vi.fn(),
  } as unknown as ReturnType<typeof useConversation>
  composer = useComposer(chat, () => false, sent)
  return (
    <form onSubmit={composer.submit}>
      <button type="submit">Send</button>
      <output>{String(composer.expanded)}</output>
    </form>
  )
}
async function render(id: string, text: string, submit: () => Promise<boolean>) {
  await React.act(async () =>
    root.render(<Surface id={id} text={text} submit={submit} />),
  )
}
const expand = () => React.act(async () => composer.changeExpanded(true, "control"))
const send = () =>
  React.act(async () => {
    const button = container.querySelector("button")
    if (!button) throw new Error("Missing Send button")
    button.click()
  })
beforeEach(() => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true })
  sent.mockClear()
  container = document.createElement("div")
  document.body.appendChild(container)
  root = createRoot(container)
})
afterEach(async () => {
  await React.act(async () => root.unmount())
  container.remove()
})

it("does not collapse tab B when tab A's pending send is followed by clearing B's draft", async () => {
  const pending = deferred(),
    submit = vi.fn(() => pending.promise)
  await render("A", "send A", submit)
  await expand()
  await send()
  await render("B", "draft B", submit)
  await expand()
  expect(composer.expanded).toBe(true)
  await render("B", "", submit)
  expect(composer.expanded).toBe(true)
  await React.act(async () => pending.resolve(true))
  expect(sent).toHaveBeenCalledExactlyOnceWith("A")
  expect(composer.expanded).toBe(true)
})

it("keeps a newer send's collapse observation when an older send is refused", async () => {
  const first = deferred(),
    next = deferred()
  const submit = vi
    .fn()
    .mockReturnValueOnce(first.promise)
    .mockReturnValueOnce(next.promise)
  await render("A", "send A", submit)
  await send()
  await render("B", "send B", submit)
  await expand()
  await send()
  await React.act(async () => first.resolve(false))
  await render("B", "", submit)
  expect(composer.expanded).toBe(false)
  await React.act(async () => next.resolve(true))
  expect(sent).toHaveBeenCalledExactlyOnceWith("B")
})

it("keeps a refused send's editor open when its draft is cleared later", async () => {
  const pending = deferred(),
    submit = vi.fn(() => pending.promise)
  await render("A", "kept draft", submit)
  await expand()
  await send()
  await React.act(async () => pending.resolve(false))
  await render("A", "", submit)
  expect(composer.expanded).toBe(true)
  expect(sent).not.toHaveBeenCalled()
})
