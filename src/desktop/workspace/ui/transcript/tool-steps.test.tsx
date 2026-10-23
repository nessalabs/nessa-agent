// @vitest-environment jsdom
/** A run of steps as the kit's waterfall: open while its turn runs, folded to one line once it ended. */
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
  // The kit asks whether motion is reduced; jsdom has no media queries.
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

const T0 = Date.UTC(2026, 9, 23, 9, 0, 0)

const steps: StepPart[] = [
  {
    kind: "step",
    step: "search",
    label: "ToolSearch",
    startedAtMs: T0,
    endedAtMs: T0 + 300,
  },
  {
    kind: "step",
    step: "search",
    label: "Search",
    detail: '"scroll"',
    startedAtMs: T0 + 400,
    endedAtMs: T0 + 900,
  },
  {
    kind: "step",
    step: "read",
    label: "Read",
    detail: "transcript.tsx",
    startedAtMs: T0 + 1000,
  },
]

const line = () => host.querySelector<HTMLButtonElement>("[data-slot=tool-trace-line]")
const rows = () => [
  ...host.querySelectorAll<HTMLButtonElement>("[data-slot=tool-trace-step]"),
]
const body = () =>
  host.querySelector<HTMLElement>(`[id="${line()?.getAttribute("aria-controls")}"]`)

it("is open while its turn runs, its last call at work", async () => {
  await act(async () => root.render(<ToolSteps steps={steps} />))
  expect(line()?.textContent).toContain("Working")
  expect(line()?.getAttribute("aria-expanded")).toBe("true")
  expect(body()?.hasAttribute("inert")).toBe(false)
  expect(rows().map((row) => row.dataset.status)).toEqual([
    "completed",
    "completed",
    "running",
  ])
  expect(rows()[1]?.textContent).toContain('"scroll"')
})

it("folds once its turn ends, and then opens and closes as the person chooses", async () => {
  await act(async () => root.render(<ToolSteps steps={steps} />))
  await act(async () => root.render(<ToolSteps steps={steps} settled />))
  expect(line()?.textContent).toContain("Ran 3 tools")
  expect(line()?.getAttribute("aria-expanded")).toBe("false")
  expect(body()?.hasAttribute("inert")).toBe(true)
  // A call the turn ended under is finished, not still at work.
  expect(rows().map((row) => row.dataset.status)).toEqual([
    "completed",
    "completed",
    "completed",
  ])

  await act(async () => line()?.click())
  expect(line()?.getAttribute("aria-expanded")).toBe("true")
  expect(body()?.hasAttribute("inert")).toBe(false)
  // Settling again does not fold it under the person.
  await act(async () => root.render(<ToolSteps steps={[...steps]} settled />))
  expect(line()?.getAttribute("aria-expanded")).toBe("true")

  await act(async () => line()?.click())
  expect(line()?.getAttribute("aria-expanded")).toBe("false")
})

it("opens a call's card to what it printed, and keeps a gateway status it was given", async () => {
  const ran: StepPart[] = [
    {
      kind: "step",
      step: "run",
      label: "Run",
      detail: "pnpm test",
      id: "call-1",
      status: "failed",
      startedAtMs: T0,
      endedAtMs: T0 + 2000,
      input: '{"command":"pnpm test"}',
      output: "1 test failed\n",
    },
  ]
  await act(async () => root.render(<ToolSteps steps={ran} settled />))
  await act(async () => line()?.click())
  const [row] = rows()
  expect(row?.dataset.status).toBe("failed")
  await act(async () => row?.click())
  expect(row?.getAttribute("aria-expanded")).toBe("true")
  const card = host.querySelector(`[id="${row?.getAttribute("aria-controls")}"]`)
  expect(card?.textContent).toContain("1 test failed")
  // No diff, so its tabs are Input and Output, and Output is the one open.
  expect(card?.querySelectorAll("[role=tab]")).toHaveLength(2)
  expect(card?.querySelector("[role=tab][aria-selected=true]")?.textContent).toContain(
    "Output",
  )
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
        <ToolSteps steps={fetched} settled />
      </SiteIconProvider>,
    ),
  )
  expect(asked).toEqual(["https://example.com/page"])
  const site = host.querySelector("[data-site-icon]")
  expect(site?.getAttribute("data-site-icon")).toBe("https://example.com/page")
  expect(site?.querySelector("svg")).not.toBeNull()
})

it("draws only the kind's icon, and fetches nothing, when no site icon is composed", async () => {
  await act(async () => root.render(<ToolSteps steps={fetched} settled />))
  expect(rows().map((row) => row.querySelectorAll("svg").length)).toEqual([1, 1])
  expect(host.querySelector("img")).toBeNull()
})
