// @vitest-environment jsdom
import * as React from "react"
import { createRoot } from "react-dom/client"
import { expect, it, vi } from "vitest"
import { AgentDownloads } from "./agent-downloads"
import type { InstallationOutcome } from "../application/agent-installations"

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true })

it("lists each agent with a one-word status, coalesces clicks, and refreshes after confirmed install", async () => {
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
  const onChanged = vi.fn()
  await React.act(async () =>
    root.render(<AgentDownloads source={source} onChanged={onChanged} />),
  )
  const download = requiredElement(
    container,
    'button[aria-label="Download Claude · 87 MB"]',
  )
  // The row says one word; the size is the button's tooltip and name.
  expect(download.textContent).toBe("Download")
  expect(download.title).toBe("87 MB")
  await React.act(async () => {
    download.click()
    download.click()
  })
  expect(source.install).toHaveBeenCalledTimes(1)
  // Downloading, and nothing more: no paragraph, size or second button.
  const row = requiredElement(container, '[data-agent-download="claude"]')
  expect(row.textContent).toBe("ClaudeDownloading")
  const progress = requiredElement(container, '[role="progressbar"]')
  expect(progress.getAttribute("aria-label")).toBe("Downloading Claude")
  expect(progress.hasAttribute("aria-valuenow")).toBe(false)
  expect(container.querySelectorAll("button")).toHaveLength(0)
  expect(requiredElement(container, '[role="status"]').textContent).toBe(
    "Downloading Claude",
  )
  source.offers.mockResolvedValue([
    { agent: "claude", archiveBytes: 86_966_377, installed: true },
  ])
  await React.act(async () => resolve({ status: "installed", cleanupPending: false }))
  expect(container.querySelector('[role="progressbar"]')).toBeNull()
  expect(row.textContent).toBe("ClaudeInstalled")
  expect(onChanged).toHaveBeenCalledTimes(1)
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
  const onChanged = vi.fn()
  await React.act(async () =>
    root.render(<AgentDownloads source={source} onChanged={onChanged} />),
  )
  await React.act(async () => requiredElement(container, "button").click())
  await React.act(async () => root.unmount())
  await React.act(async () => resolve({ status: "installed", cleanupPending: false }))
  expect(onChanged).not.toHaveBeenCalled()
  expect(source.install).toHaveBeenCalledTimes(1)
})

it.each(["download", "refused", "verification", "busy", "not-confirmed"] as const)(
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
    expect(container.querySelector('[role="progressbar"]')).toBeNull()
    expect(status).not.toContain("Installed.")
    expect(status?.length).toBeGreaterThan(10)
    if (reason === "refused") {
      expect(status).toContain("release server refused")
      expect(status).not.toMatch(/connection|try again/i)
    }
    if (reason === "download") expect(status).toContain("Check your connection")
    // Where the download itself failed, the row offers Retry with the reason
    // as its tooltip; where it may still be running, a fresh check comes first.
    if (["download", "refused", "verification"].includes(reason)) {
      const retry = requiredElement(container, 'button[aria-label^="Retry Claude"]')
      expect(retry.textContent).toBe("Retry")
      expect(retry.title).toBe(status)
    } else {
      expect(container.querySelector("[data-agent-download]")).toBeNull()
      expect(requiredElement(container, "button").textContent).toBe("Check again")
    }
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
  expect(container.textContent).toContain("Can’t check downloads")
  expect(requiredElement(container, "button").textContent).toBe("Check again")
  source.offers.mockResolvedValue([
    { agent: "claude", archiveBytes: 1, installed: false },
  ])
  await React.act(async () => notify())
  expect(
    container.querySelector('button[aria-label="Download Claude · 1 MB"]'),
  ).not.toBeNull()
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
  const onChanged = vi.fn()
  await React.act(async () =>
    root.render(<AgentDownloads source={first} onChanged={onChanged} />),
  )
  await React.act(async () => requiredElement(container, "button").click())
  await React.act(async () =>
    root.render(<AgentDownloads source={next} onChanged={onChanged} />),
  )
  await React.act(async () => complete({ status: "installed", cleanupPending: false }))
  expect(container.textContent).not.toContain("Installed.")
  expect(onChanged).not.toHaveBeenCalled()
  await React.act(async () => root.unmount())
})

it.each(["busy", "not-confirmed"] as const)(
  "does not let a pre-outcome offer restore Download after %s",
  async (reason) => {
    const container = document.createElement("div")
    const root = createRoot(container)
    let publish = () => {}
    let offersReady!: (
      offers: { agent: "claude"; archiveBytes: number; installed: boolean }[],
    ) => void
    let complete!: (outcome: InstallationOutcome) => void
    const offer = { agent: "claude" as const, archiveBytes: 1, installed: false }
    const source = {
      subscribe: (listener: () => void) => {
        publish = listener
        return () => {}
      },
      offers: vi
        .fn()
        .mockResolvedValueOnce([offer])
        .mockImplementationOnce(
          () =>
            new Promise((resolve) => {
              offersReady = resolve
            }),
        )
        .mockResolvedValue([offer]),
      install: vi.fn(
        () =>
          new Promise<InstallationOutcome>((resolve) => {
            complete = resolve
          }),
      ),
    }
    try {
      await React.act(async () => root.render(<AgentDownloads source={source} />))
      await React.act(async () => requiredElement(container, "button").click())
      // A connection change starts another offers read while installation is pending.
      await React.act(async () => publish())
      await React.act(async () => complete({ status: "failed", reason }))
      expect(container.querySelector('button[aria-label^="Download"]')).toBeNull()
      await React.act(async () => offersReady([offer]))
      expect(container.querySelector('button[aria-label^="Download"]')).toBeNull()
      expect(source.install).toHaveBeenCalledOnce()
      // A read explicitly started after uncertainty remains a supported recovery.
      await React.act(async () => requiredElement(container, "button").click())
      expect(source.offers).toHaveBeenCalledTimes(3)
      expect(container.querySelector('button[aria-label^="Download"]')).not.toBeNull()
    } finally {
      await React.act(async () => root.unmount())
    }
  },
)
