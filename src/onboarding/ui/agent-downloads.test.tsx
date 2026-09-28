// @vitest-environment jsdom
import * as React from "react"
import { createRoot } from "react-dom/client"
import { expect, it, vi } from "vitest"
import { AgentDownloads } from "./agent-downloads"
import type { InstallationOutcome } from "../application/agent-installations"

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true })

it("offers measured downloads, coalesces clicks, and refreshes after confirmed install", async () => {
  const container = document.createElement("div")
  const root = createRoot(container)
  let resolve!: (outcome: InstallationOutcome) => void
  const source = {
    subscribe: () => () => {},
    offers: vi
      .fn()
      .mockResolvedValue([
        { agent: "claude", archiveBytes: 86_966_377, installed: false },
      ]),
    install: vi.fn(
      () =>
        new Promise<InstallationOutcome>((done) => {
          resolve = done
        }),
    ),
  }
  const onInstalled = vi.fn()
  await React.act(async () =>
    root.render(<AgentDownloads source={source} onInstalled={onInstalled} />),
  )
  const download = [...container.querySelectorAll("button")].find((button) =>
    button.textContent?.includes("Download ·"),
  )
  if (!download) throw new Error("Missing download button")
  expect(download.textContent).toContain("87 MB")
  await React.act(async () => {
    download.click()
    download.click()
  })
  expect(source.install).toHaveBeenCalledTimes(1)
  expect(container.textContent).toContain("installation will continue")
  source.offers.mockResolvedValue([
    { agent: "claude", archiveBytes: 86_966_377, installed: true },
  ])
  await React.act(async () => resolve({ status: "installed", cleanupPending: false }))
  expect(container.textContent).toContain("Installed")
  expect(onInstalled).toHaveBeenCalledTimes(1)
  await React.act(async () => root.unmount())
})

it("closing the view does not cancel a download or publish a late result to another view", async () => {
  const container = document.createElement("div")
  const root = createRoot(container)
  let resolve!: (outcome: InstallationOutcome) => void
  const source = {
    subscribe: () => () => {},
    offers: vi
      .fn()
      .mockResolvedValue([
        { agent: "codex", archiveBytes: 116_501_639, installed: false },
      ]),
    install: vi.fn(
      () =>
        new Promise<InstallationOutcome>((done) => {
          resolve = done
        }),
    ),
  }
  const onInstalled = vi.fn()
  await React.act(async () =>
    root.render(<AgentDownloads source={source} onInstalled={onInstalled} />),
  )
  await React.act(async () => requiredElement(container, "button").click())
  await React.act(async () => root.unmount())
  await React.act(async () => resolve({ status: "installed", cleanupPending: false }))
  expect(onInstalled).not.toHaveBeenCalled()
  expect(source.install).toHaveBeenCalledTimes(1)
})

it.each(["download", "verification", "busy", "not-confirmed"] as const)(
  "reports %s without claiming success",
  async (reason) => {
    const container = document.createElement("div")
    const root = createRoot(container)
    const source = {
      subscribe: () => () => {},
      offers: vi
        .fn()
        .mockResolvedValue([{ agent: "claude", archiveBytes: 1, installed: false }]),
      install: vi.fn(async () => ({ status: "failed" as const, reason })),
    }
    await React.act(async () => root.render(<AgentDownloads source={source} />))
    await React.act(async () => requiredElement(container, "button").click())
    const status = requiredElement(container, '[role="status"]').textContent
    expect(status).not.toContain("Installed.")
    expect(status?.length).toBeGreaterThan(10)
    await React.act(async () => root.unmount())
  },
)

function requiredElement(container: HTMLElement, selector: string): HTMLElement {
  const element = container.querySelector<HTMLElement>(selector)
  if (!element) throw new Error(`Missing ${selector}`)
  return element
}

it("refreshes offers when setup's authenticated session becomes available", async () => {
  const container = document.createElement("div")
  const root = createRoot(container)
  let notify = () => {}
  const unsubscribe = vi.fn()
  const source = {
    subscribe: (listener: () => void) => {
      notify = listener
      return unsubscribe
    },
    offers: vi.fn().mockRejectedValue(new Error("connecting")),
    install: vi.fn(),
  }
  await React.act(async () => root.render(<AgentDownloads source={source} />))
  expect(container.textContent).toContain("Cannot check downloads")
  source.offers.mockResolvedValue([
    { agent: "claude", archiveBytes: 1, installed: false },
  ])
  await React.act(async () => notify())
  expect(container.textContent).toContain("Download · 1 MB")
  await React.act(async () => root.unmount())
  expect(unsubscribe).toHaveBeenCalledTimes(1)
})

it("ignores an old source's completion after the view receives a new source", async () => {
  const container = document.createElement("div")
  const root = createRoot(container)
  let complete: (outcome: InstallationOutcome) => void = () => {}
  const first = {
    subscribe: () => () => {},
    offers: vi
      .fn()
      .mockResolvedValue([{ agent: "claude", archiveBytes: 1, installed: false }]),
    install: () =>
      new Promise<InstallationOutcome>((resolve) => {
        complete = resolve
      }),
  }
  const next = {
    subscribe: () => () => {},
    offers: vi
      .fn()
      .mockResolvedValue([{ agent: "codex", archiveBytes: 2, installed: false }]),
    install: vi.fn(),
  }
  const onInstalled = vi.fn()
  await React.act(async () =>
    root.render(<AgentDownloads source={first} onInstalled={onInstalled} />),
  )
  await React.act(async () => requiredElement(container, "button").click())
  await React.act(async () =>
    root.render(<AgentDownloads source={next} onInstalled={onInstalled} />),
  )
  await React.act(async () => complete({ status: "installed", cleanupPending: false }))
  expect(container.textContent).not.toContain("Installed.")
  expect(onInstalled).not.toHaveBeenCalled()
  await React.act(async () => root.unmount())
})
