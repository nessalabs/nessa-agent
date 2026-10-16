// @vitest-environment jsdom
/** A run of steps: each one while its turn runs, one line that opens to them once it ended. */
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"
import type { StepPart } from "../../model/transcript"
import { ToolSteps } from "./tool-steps"

let root: Root
let host: HTMLDivElement

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  // The kit's line asks whether motion is reduced; jsdom has no media queries.
  window.matchMedia ??= ((query: string) => ({
    matches: false,
    media: query,
    addEventListener() {},
    removeEventListener() {},
  })) as unknown as typeof window.matchMedia
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
})

afterEach(async () => {
  await act(async () => root.unmount())
  host.remove()
})

const steps: StepPart[] = [
  { kind: "step", step: "search", label: "ToolSearch" },
  { kind: "step", step: "search", label: "Search", detail: '"scroll"' },
  { kind: "step", step: "read", label: "Read", detail: "transcript.tsx" },
]

const labels = () =>
  [...host.querySelectorAll(".workspace-step-label")].map((label) => label.textContent)

it("shows each step while its turn runs", async () => {
  await act(async () => root.render(<ToolSteps steps={steps} />))
  expect(labels()).toEqual(["ToolSearch", "Search", "Read"])
  expect(host.querySelector("[data-slot=agent-activity-trigger]")).toBeNull()
})

it("folds a settled run into one line that opens to its steps in place and closes again", async () => {
  await act(async () => root.render(<ToolSteps steps={steps} settled />))
  const line = host.querySelector<HTMLButtonElement>("[data-slot=agent-activity-trigger]")
  expect(line?.textContent).toBe("Ran 3 tools · 1 read")
  expect(line?.getAttribute("aria-expanded")).toBe("false")
  expect(line?.hasAttribute("aria-haspopup")).toBe(false)
  expect(labels()).toEqual([])

  await act(async () => line?.click())
  expect(line?.getAttribute("aria-expanded")).toBe("true")
  const list = host.querySelector(".workspace-steps")
  expect(line?.getAttribute("aria-controls")).toBe(list?.id)
  expect(labels()).toEqual(["ToolSearch", "Search", "Read"])

  await act(async () => line?.click())
  expect(labels()).toEqual([])
  expect(line?.hasAttribute("aria-controls")).toBe(false)
})
