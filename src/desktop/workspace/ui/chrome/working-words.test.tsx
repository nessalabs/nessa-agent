// @vitest-environment jsdom
import { act } from "react"
import { createRoot } from "react-dom/client"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { SidebarRow } from "../source-list/sidebar-row"
import { workingHoldMs, workingShowAfterMs } from "./use-settled-working"
import { WorkingWords } from "./working-words"

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

let host: HTMLDivElement
let root: ReturnType<typeof createRoot>
beforeEach(() => {
  vi.useFakeTimers()
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
})
afterEach(() => {
  act(() => root.unmount())
  host.remove()
  vi.useRealTimers()
})

const render = (node: React.ReactElement) => act(() => root.render(node))
const wait = (ms: number) => act(() => vi.advanceTimersByTime(ms))
const shimmer = () => host.querySelector(".workspace-shimmer[data-working]")

it("shimmers the words once the work lasts, with no turning arc beside them", () => {
  render(<WorkingWords working>Working</WorkingWords>)
  expect(shimmer()).toBeNull()
  wait(workingShowAfterMs)
  expect(shimmer()?.textContent).toBe("Working")
  expect(host.querySelector(".workspace-status")).toBeNull()
  expect(host.textContent).toBe("Working")
})

it("never shimmers for work that ends at once, and holds a shown shimmer a moment", () => {
  render(<WorkingWords working>Chat</WorkingWords>)
  wait(workingShowAfterMs - 50)
  render(<WorkingWords working={false}>Chat</WorkingWords>)
  wait(workingShowAfterMs)
  expect(shimmer()).toBeNull()
  render(<WorkingWords working>Chat</WorkingWords>)
  wait(workingShowAfterMs)
  render(<WorkingWords working={false}>Chat</WorkingWords>)
  wait(workingHoldMs - 50)
  expect(shimmer()).not.toBeNull()
  wait(50)
  expect(shimmer()).toBeNull()
})

it("says Running to a reader only when asked", () => {
  render(
    <WorkingWords working spoken>
      A chat
    </WorkingWords>,
  )
  wait(workingShowAfterMs)
  expect(host.querySelector(".workspace-spoken")?.textContent).toBe(" Running")
})

it("shows a running channel by its name, not a glyph at its end", () => {
  render(
    <SidebarRow
      icon="channel"
      label="general"
      active={false}
      current={false}
      running
      onClick={() => {}}
    />,
  )
  wait(workingShowAfterMs)
  expect(shimmer()?.textContent).toBe("general")
  expect(host.querySelector('.workspace-status[data-status="running"]')).toBeNull()
  expect(host.querySelector("[data-trailing]")).toBeNull()
  expect(host.querySelector("[aria-label]")?.getAttribute("aria-label")).toBe(
    "general Running",
  )
})
