// @vitest-environment jsdom
/** A tool disclosure holds the transcript's end until its motion is over. */
import { afterEach, describe, expect, it, vi } from "vitest"
import { followTranscriptEnd, watchToolDisclosure } from "./tool-disclosure"

const frames = new Map<number, FrameRequestCallback>()
let nextFrame = 0
let timeout: TimerHandler | null = null

function flushFrames() {
  const due = [...frames.values()]
  frames.clear()
  for (const frame of due) frame(0)
}

afterEach(() => {
  frames.clear()
  nextFrame = 0
  timeout = null
  vi.restoreAllMocks()
  document.body.replaceChildren()
})

function scrollerWith(control: string, child = "span") {
  const scroller = document.createElement("div")
  scroller.className = "workspace-transcript"
  const button = document.createElement("button")
  button.type = "button"
  button.setAttribute("data-slot", control)
  const label = document.createElement(child)
  label.textContent = "Ran 1 tool"
  button.append(label)
  scroller.append(button)
  document.body.append(scroller)
  Object.defineProperty(scroller, "scrollHeight", { configurable: true, value: 400 })
  return { scroller, label }
}

function watch(scroller: HTMLElement) {
  vi.spyOn(window, "requestAnimationFrame").mockImplementation((callback) => {
    nextFrame += 1
    frames.set(nextFrame, callback)
    return nextFrame
  })
  vi.spyOn(window, "cancelAnimationFrame").mockImplementation((id) => {
    frames.delete(id)
  })
  vi.spyOn(window, "setTimeout").mockImplementation((callback) => {
    timeout = callback
    return 1
  })
  vi.spyOn(window, "clearTimeout").mockImplementation(() => {
    timeout = null
  })
  const released = vi.fn()
  const stop = watchToolDisclosure(scroller, released)
  return { released, stop }
}

describe("followTranscriptEnd", () => {
  it("moves to the end only while following and no disclosure is held", () => {
    const { scroller } = scrollerWith("tool-trace-line")
    followTranscriptEnd(scroller, true)
    expect(scroller.scrollTop).toBe(400)
    scroller.scrollTop = 40
    followTranscriptEnd(scroller, false)
    expect(scroller.scrollTop).toBe(40)
    scroller.setAttribute("data-disclosure-hold", "")
    followTranscriptEnd(scroller, true)
    expect(scroller.scrollTop).toBe(40)
  })
})

describe("watchToolDisclosure", () => {
  it.each(["tool-trace-line", "tool-trace-step", "tool-trace-earlier"])(
    "holds through a press on %s, including a child of the control, and lets go one frame after no motion",
    (slot) => {
      const { scroller, label } = scrollerWith(slot)
      const { released } = watch(scroller)
      label.dispatchEvent(new MouseEvent("click", { bubbles: true }))
      expect(scroller.hasAttribute("data-disclosure-hold")).toBe(true)
      flushFrames()
      expect(scroller.hasAttribute("data-disclosure-hold")).toBe(true)
      expect(released).not.toHaveBeenCalled()
      flushFrames()
      expect(scroller.hasAttribute("data-disclosure-hold")).toBe(false)
      expect(released).toHaveBeenCalledTimes(1)
    },
  )

  it("keeps the hold for the trace's motion, and through the frame after it", () => {
    const { scroller, label } = scrollerWith("tool-trace-line")
    scroller.style.setProperty("--nessa-motion-duration-normal", "200ms")
    const { released } = watch(scroller)
    label.dispatchEvent(new MouseEvent("click", { bubbles: true }))
    expect(frames.size).toBe(0)
    expect(timeout).not.toBeNull()
    const fire = timeout
    timeout = null
    if (typeof fire === "function") fire()
    expect(scroller.hasAttribute("data-disclosure-hold")).toBe(true)
    expect(released).not.toHaveBeenCalled()
    flushFrames()
    expect(scroller.hasAttribute("data-disclosure-hold")).toBe(false)
    expect(released).toHaveBeenCalledTimes(1)
  })

  it("restarts the hold when another control is pressed before the first lets go", () => {
    const { scroller, label } = scrollerWith("tool-trace-line")
    const row = document.createElement("button")
    row.type = "button"
    row.setAttribute("data-slot", "tool-trace-step")
    scroller.append(row)
    const { released } = watch(scroller)
    label.dispatchEvent(new MouseEvent("click", { bubbles: true }))
    flushFrames()
    row.dispatchEvent(new MouseEvent("click", { bubbles: true }))
    flushFrames()
    expect(released).not.toHaveBeenCalled()
    expect(scroller.hasAttribute("data-disclosure-hold")).toBe(true)
    flushFrames()
    expect(released).toHaveBeenCalledTimes(1)
  })

  it("ignores a press that is not a tool disclosure", () => {
    const { scroller } = scrollerWith("tool-trace-line")
    const other = document.createElement("button")
    other.type = "button"
    other.textContent = "Try Again"
    scroller.append(other)
    const { released } = watch(scroller)
    other.dispatchEvent(new MouseEvent("click", { bubbles: true }))
    expect(scroller.hasAttribute("data-disclosure-hold")).toBe(false)
    expect(frames.size).toBe(0)
    expect(released).not.toHaveBeenCalled()
  })
})
