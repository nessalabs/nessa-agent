// @vitest-environment jsdom
/**
 * The thinking control: a chip naming the level, opening a popover whose
 * slider the keyboard and the pointer move between the model's levels —
 * Ultra, past Max, only where the model has it — which Escape, Tab past its
 * ends or a press elsewhere closes, with Fast mode a toggle of its own.
 */
import { act, useState } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"
import { thinkingLevelsFor, type ThinkingLevel } from "../model/composer-options"
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

/** The levels a model publishing `effortLevels` is offered. */
const publishing = (effortLevels: string[]) =>
  thinkingLevelsFor({
    provider: "example",
    modelId: "example",
    displayName: "Example",
    reasoning: { effortLevels },
    fastMode: false,
    maxContextWindowTokens: 200_000,
  })
const upToMax = publishing(["low", "medium", "high", "max"])
// No catalogue model publishes a level past `max` today (ADR 302); this one
// stands for a provider that does, so the control's Ultra stays held.
const withUltra = publishing(["low", "medium", "high", "max", "ultra"])
const chosen: string[] = []

function Harness({
  levels = upToMax,
  fast,
  initial = "medium",
}: {
  levels?: readonly ThinkingLevel[]
  fast?: boolean
  initial?: string
}) {
  const [value, setValue] = useState(initial)
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
const slider = () => {
  const knob = document.querySelector<HTMLElement>('[role="slider"]')
  if (!knob) throw new Error("no slider")
  return knob
}
const said = () => slider().getAttribute("aria-valuetext")
const key = (target: Element | null, name: string, shiftKey = false) =>
  act(async () => {
    target?.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: name,
        shiftKey,
        bubbles: true,
        cancelable: true,
      }),
    )
  })
const open = async () => act(async () => chip().click())
const words = () =>
  [...document.querySelectorAll(".desktop-thinking-words")].map((element) => ({
    name: element.querySelector(".desktop-thinking-name")?.textContent,
    leaving: element.hasAttribute("data-leaving"),
    entering: element.hasAttribute("data-entering"),
  }))

it("names the level on its chip, and opens onto a slider at the level chosen", async () => {
  await act(async () => root.render(<Harness />))
  expect(chip().getAttribute("aria-label")).toBe("Thinking level: Medium")
  expect(chip().getAttribute("aria-expanded")).toBe("false")
  await open()
  expect(chip().getAttribute("aria-expanded")).toBe("true")
  // Drawn inside the chip's surface, not at the page's end.
  expect(host.contains(popover())).toBe(true)
  expect(slider().getAttribute("aria-valuemin")).toBe("0")
  expect(slider().getAttribute("aria-valuemax")).toBe("3")
  expect(slider().getAttribute("aria-valuenow")).toBe("1")
  expect(said()).toBe("Medium")
  expect(
    document.getElementById(slider().getAttribute("aria-describedby") ?? "")?.textContent,
  ).toBe("Balanced")
  expect(document.activeElement).toBe(slider())
})

it("moves between the levels with the keys", async () => {
  chosen.length = 0
  await act(async () => root.render(<Harness />))
  await open()
  await key(slider(), "ArrowRight")
  expect(said()).toBe("High")
  await key(slider(), "End")
  expect(said()).toBe("Max")
  // At the end, a further step is left alone.
  await key(slider(), "ArrowUp")
  await key(slider(), "Home")
  expect(said()).toBe("Low")
  await key(slider(), "PageUp")
  expect(said()).toBe("Medium")
  expect(chosen).toEqual(["high", "max", "low", "medium"])
  expect(chip().getAttribute("aria-label")).toBe("Thinking level: Medium")
  expect(document.activeElement).toBe(slider())
})

it("ends at Max for a model without Ultra, and past it at Ultra for one with it", async () => {
  await act(async () => root.render(<Harness />))
  await open()
  const segment = () => document.querySelector(".desktop-thinking-slider")
  expect(segment()?.hasAttribute("data-ultra")).toBe(false)
  await key(slider(), "End")
  expect(said()).toBe("Max")
  expect(popover()?.hasAttribute("data-utmost")).toBe(false)
  await act(async () => root.render(<Harness levels={withUltra} />))
  expect(segment()?.hasAttribute("data-ultra")).toBe(true)
  expect(slider().getAttribute("aria-valuemax")).toBe("4")
  await key(slider(), "End")
  expect(said()).toBe("Ultra")
  // Ultra's the one moment: the popover is marked for it.
  expect(popover()?.hasAttribute("data-utmost")).toBe(true)
})

it("shows a level carried from a model that has it as the nearest below it", async () => {
  await act(async () => root.render(<Harness initial="xhigh" />))
  expect(chip().getAttribute("aria-label")).toBe("Thinking level: High")
  // Below every level the model offers, it is the least of them.
  await act(async () => root.render(<Harness key="none" initial="none" />))
  expect(chip().getAttribute("aria-label")).toBe("Thinking level: Low")
})

it("cross-fades the words: what was shown stays, leaving, beneath the new", async () => {
  await act(async () => root.render(<Harness />))
  await open()
  // Opening shows the level at rest, with nothing leaving.
  expect(words()).toEqual([{ name: "Medium", leaving: false, entering: false }])
  await key(slider(), "ArrowRight")
  expect(words()).toEqual([
    { name: "Medium", leaving: true, entering: false },
    { name: "High", leaving: false, entering: true },
  ])
  expect(popover()?.hasAttribute("data-rising")).toBe(true)
  await key(slider(), "ArrowLeft")
  expect(popover()?.hasAttribute("data-rising")).toBe(false)
})

it("records Low chosen from the keys where a carried None is shown as Low", async () => {
  chosen.length = 0
  await act(async () => root.render(<Harness initial="none" />))
  await open()
  expect(said()).toBe("Low")
  await key(slider(), "Home")
  expect(chosen).toEqual(["low"])
})

it("reads a step as rising or falling by the model's own order, Ultra included", async () => {
  // Ultra has no words of its own in the control; its place past Max is the
  // model's order, and the step to it rises.
  await act(async () => root.render(<Harness levels={withUltra} initial="max" />))
  await open()
  await key(slider(), "ArrowRight")
  expect(said()).toBe("Ultra")
  expect(popover()?.hasAttribute("data-rising")).toBe(true)
  await key(slider(), "ArrowLeft")
  expect(said()).toBe("Max")
  expect(popover()?.hasAttribute("data-rising")).toBe(false)
  // Two names the control does not word, in the provider's order.
  await act(async () =>
    root.render(
      <Harness
        key="unworded"
        levels={publishing(["gentle", "fierce"])}
        initial="gentle"
      />,
    ),
  )
  await open()
  await key(slider(), "ArrowRight")
  expect(popover()?.hasAttribute("data-rising")).toBe(true)
})

it("opens at rest after a change: nothing fades, and Ultra's light does not run again", async () => {
  await act(async () => root.render(<Harness levels={withUltra} />))
  await open()
  await key(slider(), "End")
  expect(document.querySelector(".desktop-thinking-sheen")).not.toBeNull()
  await key(slider(), "Escape")
  await open()
  expect(words()).toEqual([{ name: "Ultra", leaving: false, entering: false }])
  expect(document.querySelector(".desktop-thinking-sheen")).toBeNull()
})

it("fades from no level it was not showing when the model changes while closed", async () => {
  function Switching({ levels }: { levels: readonly ThinkingLevel[] }) {
    return <ThinkingControl levels={levels} value="high" onValueChange={() => {}} />
  }
  await act(async () => root.render(<Switching levels={upToMax} />))
  // A model that does not reason, then one with Ultra, all while closed.
  await act(async () => root.render(<Switching levels={[]} />))
  await act(async () => root.render(<Switching levels={withUltra} />))
  await open()
  expect(words()).toEqual([{ name: "High", leaving: false, entering: false }])
})

it("stands the knob and fill at the level's fraction of the track", async () => {
  await act(async () => root.render(<Harness levels={withUltra} />))
  await open()
  const track = () =>
    document.querySelector<HTMLElement>(".desktop-thinking-slider")?.style
  expect(track()?.getPropertyValue("--position")).toBe("0.25")
  // Ultra's segment begins at Max, the level before it.
  expect(track()?.getPropertyValue("--ultra-from")).toBe("0.75")
  await key(slider(), "End")
  expect(track()?.getPropertyValue("--position")).toBe("1")
})

it("closes on Escape, giving the keyboard back to its chip, and takes the key", async () => {
  await act(async () => root.render(<Harness />))
  await open()
  let reachedWindow = false
  const onKey = (event: KeyboardEvent) => {
    reachedWindow = !event.defaultPrevented
  }
  window.addEventListener("keydown", onKey)
  await key(slider(), "Escape")
  window.removeEventListener("keydown", onKey)
  expect(popover()).toBeNull()
  expect(document.activeElement).toBe(chip())
  expect(reachedWindow).toBe(false)
})

it("leaves for its chip when Tab runs past either end", async () => {
  await act(async () => root.render(<Harness fast />))
  const fast = () => document.querySelector('[aria-label="Fast mode"]')
  await open()
  // Shift-Tab from the slider goes back to Fast, inside it: the browser moves that.
  await key(slider(), "Tab", true)
  expect(popover()).not.toBeNull()
  // Tab from the slider, its last part, leaves.
  await key(slider(), "Tab")
  expect(popover()).toBeNull()
  expect(document.activeElement).toBe(chip())
  // Shift-Tab from Fast, its first, leaves too.
  await open()
  await key(fast(), "Tab", true)
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
  expect(said()).toBe("Medium")
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
