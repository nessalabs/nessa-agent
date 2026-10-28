// @vitest-environment jsdom
/**
 * A widget in a message is a card of its own: another widget at the same
 * place in the message is drawn afresh, never handed the last one's view.
 */
import { act, useState } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it } from "vitest"
import { HeldFilesProvider, type HeldFileReader } from "../../../../conversation"
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
  views: { panel: Card, inline: Card },
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

function drawer() {
  const store = testStore()
  const registry = createWidgetRegistry<WidgetPlugin>([plugin])
  return (message: MessageValue) =>
    act(async () =>
      root.render(
        <Provider store={store}>
          <WidgetRegistryProvider registry={registry}>
            <Message sessionId="a" message={message} isNew={false} />
          </WidgetRegistryProvider>
        </Provider>,
      ),
    )
}

it("draws another widget at the same place in a message afresh", async () => {
  const draw = drawer()
  await draw(reply("first"))
  await act(async () => host.querySelector("button")?.click())
  expect(host.querySelector("button")?.textContent).toBe("first pressed")
  await draw(reply("second"))
  expect(host.querySelector("button")?.textContent).toBe("second")
})

for (const prefix of ["added", "removed"] as const)
  it(`keeps the same widget mounted when preceding text is ${prefix}`, async () => {
    const draw = drawer()
    await draw(reply("same"))
    await act(async () => host.querySelector("button")?.click())
    const held = host.querySelector("button")
    const message = reply("same")
    await draw({
      ...message,
      parts:
        prefix === "added"
          ? [{ kind: "text", text: "Earlier text" }, ...message.parts]
          : message.parts.slice(1),
    })
    expect(host.querySelector("button")).toBe(held)
    expect(host.querySelector("button")?.textContent).toBe("same pressed")
  })

it("keeps repeated widget references as distinct mounted siblings across text changes", async () => {
  const draw = drawer()
  const first = reply("same")
  const repeated: MessageValue = { ...first, parts: [...first.parts, first.parts[1]] }
  await draw(repeated)
  const held = [...host.querySelectorAll("button")]
  expect(held).toHaveLength(2)
  await act(async () => held[0].click())
  await draw({
    ...repeated,
    parts: [{ kind: "text", text: "Earlier" }, ...repeated.parts],
  })
  expect([...host.querySelectorAll("button")]).toEqual(held)
  expect(held.map((button) => button.textContent)).toEqual(["same pressed", "same"])
})

it("draws the files the agent published one after another as one list of cards, read from the session's conversation", async () => {
  URL.createObjectURL = (() => "blob:test/1") as typeof URL.createObjectURL
  URL.revokeObjectURL = (() => {}) as typeof URL.revokeObjectURL
  const asked: [string, string][] = []
  const read: HeldFileReader = async (conversationId, file) => {
    asked.push([conversationId, file.digest])
    return new Blob(["png"], { type: file.mimeType })
  }
  const digest = `sha256:${"a".repeat(64)}`
  const store = testStore()
  await act(async () =>
    root.render(
      <Provider store={store}>
        <HeldFilesProvider read={read} save={async () => "saved"}>
          <Message
            sessionId="a"
            isNew={false}
            message={{
              id: "m",
              role: "agent",
              at: 1,
              parts: [
                { kind: "text", text: "Charted." },
                {
                  kind: "file",
                  name: "chart.png",
                  digest,
                  mimeType: "image/png",
                  size: 3,
                },
                {
                  kind: "file",
                  name: "data.csv",
                  digest,
                  mimeType: "text/csv",
                  size: 3,
                },
              ],
            }}
          />
        </HeldFilesProvider>
      </Provider>,
    ),
  )
  await act(async () => {})
  expect(asked).toEqual([["a", digest]])
  expect(
    host.querySelector("[data-published-file='chart.png'] img")?.getAttribute("src"),
  ).toBe("blob:test/1")
  expect(host.querySelector("button[aria-label='Save chart.png']")).not.toBeNull()
  // Both in one list, not a list each.
  const lists = host.querySelectorAll("[data-published-files]")
  expect(lists).toHaveLength(1)
  expect(
    [...(lists[0]?.querySelectorAll("[data-published-file]") ?? [])].map((card) =>
      card.getAttribute("data-published-file"),
    ),
  ).toEqual(["chart.png", "data.csv"])
})
