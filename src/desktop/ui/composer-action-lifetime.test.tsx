// @vitest-environment jsdom
/** An activation belonging to the old control cannot become its opposite act. */
import { act, StrictMode } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"
import { Composer } from "./composer"

let host: HTMLDivElement
let root: Root
let sent: string[]
let stops: number

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
  sent = []
  stops = 0
})

afterEach(async () => {
  await act(async () => root.unmount())
  host.remove()
})

const draft = "SYNTHETIC unsent draft"

async function render(running: boolean) {
  await act(async () =>
    root.render(
      <StrictMode>
        <Composer
          page={false}
          onPageChange={() => {}}
          text={draft}
          onTextChange={() => {}}
          model={undefined}
          onModelChange={() => {}}
          onSend={(text) => sent.push(text)}
          onStop={running ? { stop: () => stops++ } : undefined}
        />
      </StrictMode>,
    ),
  )
}

function control(): HTMLButtonElement {
  const button = host.querySelector<HTMLButtonElement>(".desktop-send")
  if (!button) throw new Error("missing composer control")
  return button
}

it("does not turn an old Send activation into Stop", async () => {
  await render(false)
  const send = control()
  await render(true)
  await act(async () => send.click())
  expect(stops).toBe(0)
  expect(sent).toEqual([])
  expect(host.querySelector("textarea")?.value).toBe(draft)

  await act(async () => control().click())
  expect(stops).toBe(1)
  expect(sent).toEqual([])
})

it("does not turn an old Stop activation into a submitted draft", async () => {
  await render(true)
  const stop = control()
  await render(false)
  await act(async () => stop.click())
  expect(sent).toEqual([])
  expect(stops).toBe(0)
  expect(host.querySelector("textarea")?.value).toBe(draft)

  await act(async () => control().click())
  expect(sent).toEqual([draft])
  expect(stops).toBe(0)
})
