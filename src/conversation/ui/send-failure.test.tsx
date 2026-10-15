// @vitest-environment jsdom
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { ClipboardProvider, SendFailure } from "./send-failure"

let root: Root
let host: HTMLDivElement
beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
})
afterEach(async () => {
  await act(async () => root.unmount())
  host.remove()
})

function button(label: string): HTMLButtonElement {
  const found = [...host.querySelectorAll("button")].find(
    (element) =>
      element.getAttribute("aria-label") === label || element.textContent === label,
  )
  if (!found) throw new Error(`missing button: ${label}`)
  return found
}

it("shows a compact error with real code and copies the complete diagnostic only on request", async () => {
  const write = vi.fn(async () => {})
  const retry = vi.fn()
  const discard = vi.fn()
  const details = "An unusually long provider diagnostic\nwith all the setup information."
  await act(async () =>
    root.render(
      <ClipboardProvider write={write}>
        <SendFailure
          details={details}
          code="agent_not_configured"
          onRetry={retry}
          onDiscard={discard}
        />
      </ClipboardProvider>,
    ),
  )
  expect(host.textContent).toContain("Error sending · agent_not_configured")
  expect(host.textContent).not.toContain(details)
  expect(write).not.toHaveBeenCalled()
  await act(async () => button("Copy error details").click())
  expect(write).toHaveBeenCalledExactlyOnceWith(`agent_not_configured\n${details}`)
  await act(async () => button("Send Again").click())
  await act(async () => button("Discard").click())
  expect(retry).toHaveBeenCalledTimes(1)
  expect(discard).toHaveBeenCalledTimes(1)
})

it("does not invent an error code, and reports a clipboard failure without losing recovery actions", async () => {
  const write = vi.fn(async () => {
    throw new Error("permission denied")
  })
  await act(async () =>
    root.render(
      <ClipboardProvider write={write}>
        <SendFailure details="offline" onRetry={() => {}} />
      </ClipboardProvider>,
    ),
  )
  expect(host.textContent).toContain("Error sending")
  expect(host.textContent).not.toContain(" · ")
  await act(async () => button("Copy error details").click())
  expect(host.querySelector('[aria-label="Copy failed; try again"]')).not.toBeNull()
  expect(host.textContent).toContain("Send Again")
})

it("keeps clipboard and retry disabled when unavailable but still permits local discard", async () => {
  await act(async () =>
    root.render(
      <SendFailure details="offline" pending onRetry={() => {}} onDiscard={() => {}} />,
    ),
  )
  expect(button("Copy error details").disabled).toBe(true)
  expect(button("Send Again").disabled).toBe(true)
  expect(button("Discard").disabled).toBe(false)
})
