// @vitest-environment jsdom
/** A run of steps: each one while its turn runs, one line that opens to them once it ended. */
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"
import type { StepPart } from "../../model/transcript"
import { SiteIconProvider, type SiteIcon } from "./site-icon"
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

const fetched: StepPart[] = [
  { kind: "step", step: "search", label: "Fetch", detail: "https://example.com/page" },
  { kind: "step", step: "read", label: "Read", detail: "transcript.tsx" },
]

it("asks the composed site icon for a step on a web page, with the kind's icon as its fallback", async () => {
  const asked: string[] = []
  const siteIcon: SiteIcon = ({ href, fallback }) => {
    asked.push(href)
    return <span data-site-icon={href}>{fallback}</span>
  }
  await act(async () =>
    root.render(
      <SiteIconProvider render={siteIcon}>
        <ToolSteps steps={fetched} />
      </SiteIconProvider>,
    ),
  )
  expect(asked).toEqual(["https://example.com/page"])
  const site = host.querySelector("[data-site-icon]")
  expect(site?.getAttribute("data-site-icon")).toBe("https://example.com/page")
  expect(site?.querySelector("svg")).not.toBeNull()
})

it("draws only the kind's icon, and fetches nothing, when no site icon is composed", async () => {
  await act(async () => root.render(<ToolSteps steps={fetched} />))
  expect(host.querySelectorAll(".workspace-steps li > svg")).toHaveLength(2)
  expect(host.querySelector("img")).toBeNull()
})
