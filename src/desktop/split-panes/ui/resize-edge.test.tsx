// @vitest-environment jsdom
/**
 * A resize edge the keyboard can reach says where it stands, and between
 * what bounds, as a separator does.
 */
import { act } from "react"
import { createRoot } from "react-dom/client"
import { expect, it } from "vitest"
import { ResizeEdge } from "./resize-edge"

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
