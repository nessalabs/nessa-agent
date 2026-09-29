// @vitest-environment jsdom
/**
 * The thinking control: a chip naming the level, opening a popover whose
 * stops are a radio group the keyboard walks, which Escape or a press
 * elsewhere closes, with Fast mode a toggle of its own.
 */
import { act, useState } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"
import { thinkingLevels } from "../model/composer-options"
import { ThinkingControl } from "./thinking-control"

let host: HTMLDivElement
let root: Root

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  host = document.createElement("div")
  // The popover is drawn in the surface the chip sits in, for its tokens and theme.
  host.dataset.surface = "window"
  document.body.append(host)
  root = createRoot(host)
})

afterEach(async () => {
  await act(async () => root.unmount())
  host.remove()
})

const chosen: string[] = []

function Harness({
  levels = thinkingLevels,
  fast,
}: {
  levels?: typeof thinkingLevels
  fast?: boolean
}) {
  const [value, setValue] = useState("medium")
  const [pressed, setPressed] = useState(false)
  return (
    <ThinkingControl
      levels={levels}
      value={value}
      onValueChange={(next) => {
        chosen.push(next)
        setValue(next)
      }}
      fastMode={fast ? { pressed, onPressedChange: setPressed } : undefined}
    />
  )
}

const chip = () => {
  const button = host.querySelector<HTMLButtonElement>(".desktop-chip-thinking")
  if (!button) throw new Error("no thinking chip")
  return button
}
const popover = () => document.querySelector<HTMLElement>('[role="dialog"]')
const radios = () => [
  ...document.querySelectorAll<HTMLButtonElement>('[role="radiogroup"] [role="radio"]'),
]
const checked = () =>
  radios().find((radio) => radio.getAttribute("aria-checked") === "true")?.ariaLabel
const key = (target: Element, name: string) =>
  act(async () => {
    target.dispatchEvent(
      new KeyboardEvent("keydown", { key: name, bubbles: true, cancelable: true }),
    )
  })
const open = async () => act(async () => chip().click())

it("names the level on its chip, and opens onto the level chosen", async () => {
  await act(async () => root.render(<Harness />))
  expect(chip().getAttribute("aria-label")).toBe("Thinking level: Medium")
  expect(chip().getAttribute("aria-expanded")).toBe("false")
  await open()
  expect(chip().getAttribute("aria-expanded")).toBe("true")
  // Drawn inside the chip's surface, not at the page's end.
  expect(host.contains(popover())).toBe(true)
  expect(radios().map((radio) => radio.ariaLabel)).toEqual([
    "Low",
    "Medium",
    "High",
    "Max",
  ])
  expect(checked()).toBe("Medium")
  expect(document.activeElement).toBe(radios()[1])
  // Only the chosen stop is in the Tab order.
  expect(radios().map((radio) => radio.tabIndex)).toEqual([-1, 0, -1, -1])
})

it("walks the levels with the keys, and says each one's line", async () => {
  chosen.length = 0
  await act(async () => root.render(<Harness />))
  await open()
  await key(radios()[1], "ArrowRight")
  expect(checked()).toBe("High")
  expect(document.activeElement).toBe(radios()[2])
  await key(radios()[2], "End")
  expect(checked()).toBe("Max")
  // At the end, a further step is left alone.
  await key(radios()[3], "ArrowRight")
  await key(radios()[3], "Home")
  expect(checked()).toBe("Low")
  expect(chosen).toEqual(["high", "max", "low"])
  expect(chip().getAttribute("aria-label")).toBe("Thinking level: Low")
  const said = document.getElementById(radios()[0].getAttribute("aria-describedby") ?? "")
  expect(said?.textContent).toBe("Quick answers")
})

it("picks a level activated with no pointer, as a screen reader does", async () => {
  await act(async () => root.render(<Harness />))
  await open()
  await act(async () => radios()[3].click())
  expect(checked()).toBe("Max")
})

it("lights the stops up to the level, and marks the most a model will think", async () => {
  await act(async () => root.render(<Harness />))
  await open()
  const lit = () => radios().map((radio) => radio.hasAttribute("data-lit"))
  expect(lit()).toEqual([true, true, false, false])
  expect(popover()?.hasAttribute("data-utmost")).toBe(false)
  await key(radios()[1], "End")
  expect(lit()).toEqual([true, true, true, true])
  expect(popover()?.hasAttribute("data-utmost")).toBe(true)
})

it("cross-fades the words: what was shown stays, leaving, beneath the new", async () => {
  await act(async () => root.render(<Harness />))
  await open()
  const words = () =>
    [...document.querySelectorAll(".desktop-thinking-words")].map((element) => ({
      name: element.querySelector(".desktop-thinking-name")?.textContent,
      leaving: element.hasAttribute("data-leaving"),
    }))
  // Opening shows the level at rest, with nothing leaving.
  expect(words()).toEqual([{ name: "Medium", leaving: false }])
  await key(radios()[1], "ArrowRight")
  expect(words()).toEqual([
    { name: "Medium", leaving: true },
    { name: "High", leaving: false },
  ])
  expect(popover()?.hasAttribute("data-rising")).toBe(true)
  await key(radios()[2], "ArrowLeft")
  expect(popover()?.hasAttribute("data-rising")).toBe(false)
})

it("closes on Escape, giving the keyboard back to its chip, and takes the key", async () => {
  await act(async () => root.render(<Harness />))
  await open()
  let reachedWindow = false
  const onKey = (event: KeyboardEvent) => {
    reachedWindow = !event.defaultPrevented
  }
  window.addEventListener("keydown", onKey)
  await key(radios()[1], "Escape")
  window.removeEventListener("keydown", onKey)
  expect(popover()).toBeNull()
  expect(document.activeElement).toBe(chip())
  expect(reachedWindow).toBe(false)
})

it("leaves for its chip when Tab runs past either end", async () => {
  await act(async () => root.render(<Harness fast />))
  const tab = (target: Element | null, shiftKey: boolean) =>
    act(async () => {
      target?.dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "Tab",
          shiftKey,
          bubbles: true,
          cancelable: true,
        }),
      )
    })
  const fast = () => document.querySelector('[aria-label="Fast mode"]')
  await open()
  // Shift-Tab from the level goes back to Fast, inside it: the browser moves that, not the control.
  await tab(radios()[1], true)
  expect(popover()).not.toBeNull()
  // Tab from the level, its last part, leaves.
  await tab(radios()[1], false)
  expect(popover()).toBeNull()
  expect(document.activeElement).toBe(chip())
  // Shift-Tab from Fast, its first, leaves too.
  await open()
  await tab(fast(), true)
  expect(popover()).toBeNull()
  expect(document.activeElement).toBe(chip())
})

it("closes on a press anywhere else, and on its chip", async () => {
  await act(async () => root.render(<Harness />))
  await open()
  await act(async () => {
    document.body.dispatchEvent(new Event("pointerdown", { bubbles: true }))
  })
  expect(popover()).toBeNull()
  await open()
  expect(popover()).not.toBeNull()
  await open()
  expect(popover()).toBeNull()
})

it("offers Fast as a toggle of its own, shown on the chip while on", async () => {
  await act(async () => root.render(<Harness fast />))
  expect(chip().dataset.fast).toBe("off")
  await open()
  const fast = document.querySelector<HTMLButtonElement>('[aria-label="Fast mode"]')
  expect(fast?.getAttribute("aria-pressed")).toBe("false")
  await act(async () => fast?.click())
  expect(fast?.getAttribute("aria-pressed")).toBe("true")
  expect(chip().dataset.fast).toBe("on")
  // The level is not Fast's: it stays where it was.
  expect(checked()).toBe("Medium")
})

it("has no Fast where the model does not offer it", async () => {
  await act(async () => root.render(<Harness />))
  expect(chip().dataset.fast).toBeUndefined()
  expect(chip().querySelector(".desktop-fast-mark")).toBeNull()
  await open()
  expect(document.querySelector('[aria-label="Fast mode"]')).toBeNull()
})

it("is disabled, and cannot open, for a model that does not reason", async () => {
  await act(async () => root.render(<Harness levels={[]} />))
  expect(chip().disabled).toBe(true)
  expect(chip().getAttribute("aria-label")).toBe("Thinking levels unavailable")
  await open()
  expect(popover()).toBeNull()
})
