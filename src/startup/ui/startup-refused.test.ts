// @vitest-environment jsdom
import * as React from "react"
import { createRoot } from "react-dom/client"
import { afterEach, expect, it, vi } from "vitest"
import { StartupRefused } from "./startup-refused"

afterEach(() => {
  vi.restoreAllMocks()
})

it("shows the calm screen, logs the reason, and restarts or quits from the icons", async () => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  vi.spyOn(console, "error").mockImplementation(() => {})
  const container = document.createElement("div")
  const root = createRoot(container)
  const tryAgain = vi.fn()
  const quit = vi.fn()
  const details = "settings could not be read: local storage must be private"
  await React.act(async () =>
    root.render(
      React.createElement(StartupRefused, {
        details,
        onTryAgain: tryAgain,
        onQuit: quit,
      }),
    ),
  )

  expect(container.querySelector("h1")).toBeNull()
  expect(container.querySelector("details")).toBeNull()
  expect(container.textContent).toContain("Nessa couldn’t start")
  expect(container.textContent).toContain("STARTUP_HOST")
  expect(container.textContent).not.toContain("local storage")
  expect(container.textContent).not.toContain("Try again")
  const logged = vi
    .mocked(console.error)
    .mock.calls.map((call) => call.map(String).join(" "))
    .join("\n")
  expect(logged).toContain("[nessa]")
  expect(logged).toContain("local storage must be private")

  const restart = container.querySelector<HTMLButtonElement>("[aria-label=Restart]")
  const quitButton = container.querySelector<HTMLButtonElement>("[aria-label=Quit]")
  expect(restart).toBeInstanceOf(HTMLButtonElement)
  expect(quitButton).toBeInstanceOf(HTMLButtonElement)
  await React.act(async () => restart?.click())
  expect(tryAgain).toHaveBeenCalledOnce()
  await React.act(async () => quitButton?.click())
  expect(quit).toHaveBeenCalledOnce()
  await React.act(async () => root.unmount())
})
