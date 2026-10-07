// @vitest-environment jsdom
import { act } from "react"
import { createRoot } from "react-dom/client"
import { expect, it } from "vitest"
import { HistoryButtons } from "./history-buttons"

it("rests while there is no history, saying what it will do and doing nothing", async () => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  const host = document.createElement("div")
  document.body.append(host)
  const root = createRoot(host)
  const went: string[] = []
  await act(async () => root.render(<HistoryButtons />))
  const [back, forward] = host.querySelectorAll("button")
  expect([back.getAttribute("aria-label"), forward.getAttribute("aria-label")]).toEqual([
    "Go Back (⌘[)",
    "Go Forward (⌘])",
  ])
  expect(back.getAttribute("aria-disabled")).toBe("true")
  expect(back.dataset.tooltipShortcut).toBe("⌘[")
  back.click()
  // The seam: given history, they walk it.
  await act(async () =>
    root.render(<HistoryButtons canGoBack onBack={() => went.push("back")} />),
  )
  host.querySelector<HTMLButtonElement>('[data-history="back"]')?.click()
  host.querySelector<HTMLButtonElement>('[data-history="forward"]')?.click()
  expect(went).toEqual(["back"])
  await act(async () => root.unmount())
  host.remove()
})
