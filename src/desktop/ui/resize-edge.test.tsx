// @vitest-environment jsdom
/**
 * A resize edge the keyboard can reach says where it stands, and between
 * what bounds, as a separator does.
 */
import { act } from "react"
import { createRoot } from "react-dom/client"
import { expect, it, vi } from "vitest"
import { ResizeEdge } from "./resize-edge"

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true })

it("says where a focusable resize edge stands, and between what bounds", async () => {
  const host = document.createElement("div")
  document.body.append(host)
  const root = createRoot(host)
  await act(async () =>
    root.render(
      <ResizeEdge
        label="Resize Sidebar"
        value={{ now: 240.4, min: 200, max: 320 }}
        onStart={() => {}}
        onMove={() => {}}
      />,
    ),
  )
  const edge = host.querySelector('[role="separator"]')
  expect(
    ["aria-valuenow", "aria-valuemin", "aria-valuemax"].map((name) =>
      edge?.getAttribute(name),
    ),
  ).toEqual(["240", "200", "320"])
  await act(async () => root.unmount())
  host.remove()
})

it("hovering a resize edge does not measure layout or start a resize", async () => {
  const host = document.createElement("div")
  document.body.append(host)
  const root = createRoot(host)
  const onStart = vi.fn()
  const onMove = vi.fn()
  try {
    await act(async () =>
      root.render(
        <ResizeEdge
          label="Resize Sidebar"
          value={{ now: 240, min: 200, max: 320 }}
          onStart={onStart}
          onMove={onMove}
        />,
      ),
    )
    const edge = host.querySelector<HTMLElement>('[role="separator"]')
    if (!edge) throw new Error("Resize edge did not render")
    const readGeometry = vi.spyOn(edge, "getBoundingClientRect")
    await act(async () => {
      edge.dispatchEvent(
        new MouseEvent("pointerover", { bubbles: true, clientX: 240, clientY: 100 }),
      )
      edge.dispatchEvent(
        new MouseEvent("pointermove", { bubbles: true, clientX: 240, clientY: 120 }),
      )
    })
    expect(readGeometry).not.toHaveBeenCalled()
    expect(onStart).not.toHaveBeenCalled()
    expect(onMove).not.toHaveBeenCalled()
    await act(async () =>
      edge.dispatchEvent(
        new KeyboardEvent("keydown", { bubbles: true, key: "ArrowRight" }),
      ),
    )
    expect(onStart).toHaveBeenCalledOnce()
    expect(onMove).toHaveBeenCalledExactlyOnceWith(16)
  } finally {
    await act(async () => root.unmount())
    host.remove()
  }
})
