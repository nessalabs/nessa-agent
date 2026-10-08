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

it("does not measure an empty draft, closes its page, and still measures nonempty drafts", async () => {
  const root = createRoot(host)
  const changes: boolean[] = []
  const sheet = document.createElement("style")
  sheet.textContent = ".desktop-composer textarea { line-height: 24px; padding: 0; }"
  document.head.append(sheet)
  const before = Object.getOwnPropertyDescriptor(
    HTMLTextAreaElement.prototype,
    "scrollHeight",
  )
  let reads = 0
  Object.defineProperty(HTMLTextAreaElement.prototype, "scrollHeight", {
    configurable: true,
    get() {
      reads++
      return 8 * 24
    },
  })
  const render = (page: boolean, text: string) =>
    act(async () =>
      root.render(
        <Composer
          page={page}
          onPageChange={(next) => changes.push(next)}
          text={text}
          onTextChange={() => {}}
          placeholder={"A wrapping placeholder ".repeat(20)}
          model={undefined}
          onModelChange={() => {}}
        />,
      ),
    )
  try {
    await render(false, "")
    expect(reads).toBe(0)
    expect(changes).toEqual([])
    await render(true, "")
    expect(reads).toBe(0)
    expect(changes).toEqual([false])
    await render(false, Array.from({ length: 8 }, () => "draft line").join("\n"))
    expect(reads).toBeGreaterThan(0)
    expect(changes).toEqual([false, true])
  } finally {
    await act(async () => root.unmount())
    sheet.remove()
    if (before)
      Object.defineProperty(HTMLTextAreaElement.prototype, "scrollHeight", before)
    else Reflect.deleteProperty(HTMLTextAreaElement.prototype, "scrollHeight")
  }
})

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

it("offers Fast and the levels only where the catalogue records them for the model", async () => {
  const root = createRoot(host)
  const render = (modelId: string) =>
    act(async () =>
      root.render(
        <Composer
          page={false}
          onPageChange={() => {}}
          text=""
          onTextChange={() => {}}
          model={{ provider: "anthropic", modelId }}
          onModelChange={() => {}}
        />,
      ),
    )
  const chip = () => host.querySelector<HTMLButtonElement>(".desktop-chip-thinking")
  // Claude Opus 5 publishes Fast; Claude Sonnet 5 does not; Claude Haiku 4.5
  // reasons with no level recorded (crates/nessa-sdk/data/models.json).
  await render("claude-opus-5")
  expect(chip()?.dataset.fast).toBe("off")
  expect(chip()?.disabled).toBe(false)
  await render("claude-sonnet-5")
  expect(chip()?.dataset.fast).toBeUndefined()
  expect(chip()?.disabled).toBe(false)
  await render("claude-haiku-4-5-20251001")
  expect(chip()?.disabled).toBe(true)
  await act(async () => root.unmount())
})
