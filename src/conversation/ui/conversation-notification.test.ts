// @vitest-environment jsdom
import { act, createElement } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { createDependencies } from "../../composition/dependencies"
import { makeStore } from "../../store"
import { scenarioEffects } from "../adapters/scenario/effects"
import { attachFiles, sendDraft, setDraft } from "../adapters/store/slice"
import { AttachmentResource, textContent, type MessageContent } from "../model"
import { ConversationNotification } from "./conversation-notification"

let root: Root
let host: HTMLDivElement
beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
})
afterEach(async () => {
  await act(async () => root.unmount())
  host.remove()
})

/**
 * Two messages refused, then the newer one sent again: the older is offered
 * with an empty draft, and its Send Again is pressed.
 */
type Store = ReturnType<typeof makeStore>

async function sendOlderAgain(first: (store: Store, id: string) => MessageContent) {
  const effects = scenarioEffects("echo")
  let refusals = 2
  const create = vi.fn(async (...args: Parameters<typeof effects.create>) => {
    if (refusals-- > 0) throw new Error("offline")
    return effects.create(...args)
  })
  const submit = vi.fn(effects.submit)
  const store = makeStore(
    createDependencies({ conversation: { ...effects, create, submit } }),
  )
  const tab = () => {
    const found = store.getState().conversation.conversations[0]
    if (!found) throw new Error("missing conversation")
    return found
  }
  const turn = (index: number) => {
    const found = tab().turns[index]
    if (!found) throw new Error(`missing turn ${index}`)
    return found
  }
  const id = tab().id
  await store.dispatch(sendDraft({ id, content: first(store, id) }))
  store.dispatch(setDraft({ id, draft: [] }))
  await store.dispatch(sendDraft({ id, content: textContent("second") }))
  const older = turn(0)
  const newer = turn(1)
  await store.dispatch(
    sendDraft({ id, content: textContent("second"), retryTurnId: newer.id }),
  )
  const olderNow = () => tab().turns.find((each) => each.id === older.id)
  expect(tab().draft).toEqual([])
  expect(olderNow()).toMatchObject({ receipt: "failed" })

  await act(async () =>
    root.render(
      createElement(Provider, {
        store,
        children: createElement(ConversationNotification, {
          conversation: tab(),
          connection: { phase: "ready", detail: "", retry: () => {} },
          gatewayAvailable: true,
        }),
      }),
    ),
  )
  const sendAgain = [...host.querySelectorAll("button")].find(
    (element) => element.textContent === "Send Again",
  )
  if (!sendAgain) throw new Error("missing Send Again")
  expect(sendAgain.disabled).toBe(false)
  await act(async () => sendAgain.click())
  await vi.waitFor(() => expect(olderNow()).not.toMatchObject({ receipt: "failed" }))
  return { store, submit, tab }
}

// docs/state/services/desktop/chat/recover-a-refused-or-unconfirmed-send.md,
// "Several refused messages", row 2: the older message is offered with an empty
// draft, and Send Again still sends it.
it("sends an older refused message as it was refused once a newer one was sent again", async () => {
  const { submit } = await sendOlderAgain(() => textContent("first"))
  expect(submit.mock.calls.at(-1)?.[0]).toMatchObject({ text: "first" })
})

// Row 8: a file that travels as a path has nothing to upload, so Send Again
// does not wait for it.
it("sends an older refused message that points at a file without waiting for an upload", async () => {
  const { submit } = await sendOlderAgain((store, id) => {
    store.dispatch(
      attachFiles({
        conversationId: id,
        files: [
          AttachmentResource.create({
            name: "report.pdf",
            mimeType: "application/pdf",
            size: 3,
            previewUrl: "blob:report",
            path: "/Users/ada/report.pdf",
          }),
        ],
      }),
    )
    return store.getState().conversation.conversations[0]?.draft ?? []
  })
  expect(submit.mock.calls.at(-1)?.[0]).toMatchObject({
    files: [{ path: "/Users/ada/report.pdf" }],
  })
})
