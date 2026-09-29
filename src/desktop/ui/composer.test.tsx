// @vitest-environment jsdom
/**
 * The composer shows the model its owner gives it. Its Return: a press sends
 * what is typed; a held Return sends it once. Its repeats — carried in from an approval answered by the same press,
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
        model={undefined}
        onModelChange={() => {}}
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

it("shows the model it is given, and only that", async () => {
  const root = createRoot(host)
  const render = (model: { provider: string; modelId: string } | undefined) =>
    act(async () =>
      root.render(
        <Composer
          page={false}
          onPageChange={() => {}}
          text=""
          onTextChange={() => {}}
          model={model}
          onModelChange={() => {}}
        />,
      ),
    )
  const shown = () => host.querySelector(".desktop-chip-model")?.textContent ?? ""
  await render({ provider: "anthropic", modelId: "claude-opus-5" })
  expect(shown()).toContain("Claude Opus 5")
  await render({ provider: "openai", modelId: "gpt-6-astra" })
  expect(shown()).toContain("GPT-6 Astra")
  // A model the catalogue does not have is shown as none chosen, never as another.
  await render({ provider: "openai", modelId: "not-in-the-catalogue" })
  expect(shown()).toContain("Choose model")
  await act(async () => root.unmount())
})
