// @vitest-environment jsdom
import { act } from "react"
import { createRoot } from "react-dom/client"
import { afterEach, expect, it, vi } from "vitest"
import { WindowControlsProvider, useWindowControlsLead } from "./window-controls"

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

afterEach(() => {
  vi.restoreAllMocks()
})

function Leads({ by }: { by: number }) {
  useWindowControlsLead(by)
  return null
}

it("tells the host each place the first column moves to, and the window's edge once the view is gone", async () => {
  const asked: number[] = []
  const lead = (by: number) => {
    asked.push(by)
    return Promise.resolve()
  }
  const host = document.createElement("div")
  const root = createRoot(host)
  const render = (by: number) =>
    act(async () =>
      root.render(
        <WindowControlsProvider lead={lead}>
          <Leads by={by} />
        </WindowControlsProvider>,
      ),
    )

  await render(0)
  await render(48)
  // The same place again is no new ask.
  await render(48)
  await render(0)
  await render(48)
  expect(asked).toEqual([0, 48, 0, 48])

  await act(async () => root.unmount())
  expect(asked).toEqual([0, 48, 0, 48, 0])
})

it("says when the host could not move them, and asks nothing with no host", async () => {
  const error = vi.spyOn(console, "error").mockImplementation(() => {})
  const host = document.createElement("div")
  const root = createRoot(host)
  await act(async () =>
    root.render(
      <WindowControlsProvider lead={() => Promise.reject(new Error("no window"))}>
        <Leads by={48} />
      </WindowControlsProvider>,
    ),
  )
  expect(error).toHaveBeenCalledWith(
    "[nessa] could not move the window's controls",
    expect.any(Error),
  )

  // Outside the desktop host (tests, the browser preview) there is no one to tell.
  await act(async () => root.render(<Leads by={48} />))
  await act(async () => root.unmount())
})
