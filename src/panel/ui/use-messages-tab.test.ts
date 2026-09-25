// @vitest-environment jsdom
import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it, vi } from "vitest"

import { useMessagesTab } from "./use-messages-tab"

let container: HTMLDivElement
let root: Root
let move: ReturnType<typeof useMessagesTab>["move"] = () => {}

function Probe({ onLeave }: { onLeave: () => void }) {
  move = useMessagesTab(onLeave).move
  return null
}

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  container = document.createElement("div")
  root = createRoot(container)
})

afterEach(async () => {
  await React.act(async () => root.unmount())
})

it("lets go of what the list said when the tab is left, and only then", async () => {
  const onLeave = vi.fn()
  await React.act(async () =>
    root.render(
      // Development mode mounts, unmounts and mounts again: not a leave.
      React.createElement(
        React.StrictMode,
        null,
        React.createElement(Probe, { onLeave }),
      ),
    ),
  )
  await React.act(async () => move("show"))
  expect(onLeave).not.toHaveBeenCalled()
  await React.act(async () => move("leave"))
  expect(onLeave).toHaveBeenCalledOnce()
  await React.act(async () => move("show"))
  await React.act(async () => move("close"))
  expect(onLeave).toHaveBeenCalledTimes(2)
  // Leaving something already left is nothing new.
  await React.act(async () => move("leave"))
  expect(onLeave).toHaveBeenCalledTimes(2)
})
