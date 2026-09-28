// @vitest-environment jsdom
/**
 * The composer's Return: a press sends what is typed; a held Return sends it
 * once. Its repeats — carried in from an approval answered by the same press,
 * whose card went and left the caret here — send nothing.
 */
import { act } from "react"
import { createRoot } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"
import { Composer } from "./composer"

let host: HTMLDivElement

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  host = document.createElement("div")
  document.body.append(host)
})

afterEach(() => host.remove())

it("sends on a press of Return, and nothing on its repeats", async () => {
  const sent: string[] = []
  const root = createRoot(host)
  await act(async () =>
    root.render(
      <Composer
        page={false}
        onPageChange={() => {}}
        onSend={(text) => sent.push(text)}
        text="ship it"
        onTextChange={() => {}}
      />,
    ),
  )
  const field = host.querySelector("textarea")
  if (!field) throw new Error("no textarea")
  const enter = (repeat: boolean) =>
    act(async () => {
      field.dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "Enter",
          code: "Enter",
          repeat,
          bubbles: true,
          cancelable: true,
        }),
      )
    })
  await enter(true)
  expect(sent).toEqual([])
  await enter(false)
  expect(sent).toEqual(["ship it"])
  await act(async () => root.unmount())
})
