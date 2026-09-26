// @vitest-environment jsdom
import * as React from "react"
import { createRoot } from "react-dom/client"
import { expect, it, vi } from "vitest"

const startup = vi.hoisted(() => ({
  status: undefined as { revision: number; state: string } | undefined,
}))
vi.mock("../../startup", () => ({
  useGatewayStartup: () => ({ status: startup.status, retry: () => undefined }),
}))

import { usePanelStartup } from "./use-panel-startup"

it("retries a failed session once, when the host says the gateway became ready", async () => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  const retry = vi.fn()
  function Probe({ phase }: { phase: "error" | "ready" }) {
    usePanelStartup({ phase, retry })
    return null
  }
  const root = createRoot(document.createElement("div"))
  const render = (phase: "error" | "ready") =>
    React.act(async () => root.render(React.createElement(Probe, { phase })))

  startup.status = { revision: 1, state: "failed" }
  await render("error")
  expect(retry).not.toHaveBeenCalled()

  startup.status = { revision: 2, state: "ready" }
  await render("error")
  expect(retry).toHaveBeenCalledOnce()

  // Still ready, still failing: that failure is the session's own now, and it
  // keeps its own notice and Retry rather than being retried on every render.
  await render("error")
  await render("error")
  expect(retry).toHaveBeenCalledOnce()

  startup.status = { revision: 3, state: "starting" }
  await render("ready")
  startup.status = { revision: 4, state: "ready" }
  await render("ready")
  expect(retry).toHaveBeenCalledOnce()
  await React.act(async () => root.unmount())
})

it("retries once when the session fails after the gateway became ready", async () => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  const retry = vi.fn()
  function Probe({ phase }: { phase: "connecting" | "error" }) {
    usePanelStartup({ phase, retry })
    return null
  }
  const root = createRoot(document.createElement("div"))
  const render = (phase: "connecting" | "error") =>
    React.act(async () => root.render(React.createElement(Probe, { phase })))

  startup.status = { revision: 1, state: "starting" }
  await render("connecting")
  startup.status = { revision: 2, state: "ready" }
  await render("connecting")
  expect(retry).not.toHaveBeenCalled()
  // The attempt that was in flight when the gateway became ready fails.
  await render("error")
  expect(retry).toHaveBeenCalledOnce()
  await render("connecting")
  await render("error")
  expect(retry).toHaveBeenCalledOnce()
  await React.act(async () => root.unmount())
})

it("owes no retry once the session has connected after the gateway became ready", async () => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  const retry = vi.fn()
  function Probe({ phase }: { phase: "connecting" | "ready" | "error" }) {
    usePanelStartup({ phase, retry })
    return null
  }
  const root = createRoot(document.createElement("div"))
  const render = (phase: "connecting" | "ready" | "error") =>
    React.act(async () => root.render(React.createElement(Probe, { phase })))

  startup.status = { revision: 1, state: "starting" }
  await render("connecting")
  startup.status = { revision: 2, state: "ready" }
  await render("connecting")
  await render("ready")
  // Much later, an ordinary session failure against a ready gateway.
  await render("error")
  expect(retry).not.toHaveBeenCalled()
  await React.act(async () => root.unmount())
})
