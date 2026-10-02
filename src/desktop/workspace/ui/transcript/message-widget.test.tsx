// @vitest-environment jsdom
/**
 * A widget in a message is a card of its own: another widget at the same
 * place in the message is drawn afresh, never handed the last one's view.
 */
import { act, useState } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it } from "vitest"
import {
  createWidgetRegistry,
  WidgetRegistryProvider,
  type NativeWidgetPlugin,
  type WidgetPlugin,
  type WidgetViewProps,
} from "../../../widgets"
import type { Message as MessageValue } from "../../model/transcript"
import { testStore } from "../../testing"
import { Message } from "./message"

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

/** A card that remembers whether it was pressed. */
function Card({ id }: WidgetViewProps) {
  const [pressed, setPressed] = useState(false)
  return (
    <button type="button" onClick={() => setPressed(true)}>
      {`${id}${pressed ? " pressed" : ""}`}
    </button>
  )
}

const plugin: NativeWidgetPlugin = {
  kind: "native",
  id: "cards",
  name: "Cards",
  useWidget: () => ({ kind: "ready", title: "A card" }),
  views: { pane: Card, inline: Card },
}

const reply = (id: string): MessageValue => ({
  id: "m",
  role: "agent",
  at: 1,
  parts: [
    { kind: "text", text: "Here:" },
    { kind: "widget", widget: { plugin: "cards", id } },
  ],
})

it("draws another widget at the same place in a message afresh", async () => {
  const store = testStore()
  const registry = createWidgetRegistry<WidgetPlugin>([plugin])
  const draw = (message: MessageValue) =>
    act(async () =>
      root.render(
        <Provider store={store}>
          <WidgetRegistryProvider registry={registry}>
            <Message sessionId="a" message={message} isNew={false} />
          </WidgetRegistryProvider>
        </Provider>,
      ),
    )
  await draw(reply("first"))
  await act(async () => host.querySelector("button")?.click())
  expect(host.querySelector("button")?.textContent).toBe("first pressed")
  await draw(reply("second"))
  expect(host.querySelector("button")?.textContent).toBe("second")
})
