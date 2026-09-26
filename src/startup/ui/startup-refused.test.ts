// @vitest-environment jsdom
import * as React from "react"
import { createRoot } from "react-dom/client"
import { expect, it, vi } from "vitest"
import { StartupRefused } from "./startup-refused"

it("says Nessa could not start, offers Try again, and folds the reason away", async () => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  const container = document.createElement("div")
  const root = createRoot(container)
  const tryAgain = vi.fn()
  const quit = vi.fn()
  await React.act(async () =>
    root.render(
      React.createElement(StartupRefused, {
        details: "settings could not be read: local storage must be private",
        onTryAgain: tryAgain,
        onQuit: quit,
      }),
    ),
  )

  expect(container.querySelector("[role=alert] h1")?.textContent).toBe(
    "Nessa couldn’t start",
  )
  const details = container.querySelector("details")
  expect(details?.open).toBe(false)
  expect(details?.textContent).toContain("local storage must be private")
  const button = [...container.querySelectorAll("button")].find(
    (element) => element.textContent === "Try again",
  )
  await React.act(async () => button?.click())
  expect(tryAgain).toHaveBeenCalledOnce()
  // A refusal that trying again cannot clear still has a way out.
  const quitButton = [...container.querySelectorAll("button")].find(
    (element) => element.textContent === "Quit",
  )
  await React.act(async () => quitButton?.click())
  expect(quit).toHaveBeenCalledOnce()
  await React.act(async () => root.unmount())
})
