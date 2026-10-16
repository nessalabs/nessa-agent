// @vitest-environment jsdom
import { act, createElement } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { createDependencies } from "../../composition/dependencies"
import { makeStore } from "../../store"
import { scenarioEffects } from "../adapters/scenario/effects"
import { sendDraft } from "../adapters/store/slice"
import { textContent } from "../model"
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

it("sends an older refused message as it was refused once a newer one was sent again", async () => {
  // docs/state/services/desktop/chat/recover-a-refused-or-unconfirmed-send.md,
  // "Several refused messages", row 2: the older message is offered with an
  // empty draft, and Send Again still sends it.
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
  await store.dispatch(sendDraft({ content: textContent("first") }))
  const id = tab().id
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
  expect(submit.mock.calls.at(-1)?.[0]).toMatchObject({ text: "first" })
})
