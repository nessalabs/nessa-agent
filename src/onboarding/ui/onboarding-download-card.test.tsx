// @vitest-environment jsdom
import * as React from "react"
import { createRoot } from "react-dom/client"
import { expect, it, vi } from "vitest"
import { Onboarding } from "./onboarding"
import type { InstallationOutcome } from "../application/agent-installations"
import {
  beginOnboarding,
  recordReadiness,
  startAgentChoice,
  type OnboardingState,
} from "../model/onboarding"

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true })

const asking = startAgentChoice(beginOnboarding())

function installer(installed = false) {
  let finish!: (outcome: InstallationOutcome) => void
  return {
    finish: (outcome: InstallationOutcome) => finish(outcome),
    source: {
      subscribe: () => () => {},
      offers: vi
        .fn()
        .mockResolvedValue([{ agent: "codex", archiveBytes: 44_200_000, installed }]),
      install: vi.fn(
        () =>
          new Promise<InstallationOutcome>((done) => {
            finish = done
          }),
      ),
    },
  }
}

async function picker(
  state: OnboardingState,
  installations: ReturnType<typeof installer>["source"],
  onRecheck = () => {},
) {
  const container = document.createElement("div")
  // In the document, so the focus can be asked about.
  document.body.append(container)
  const root = createRoot(container)
  const render = (next: OnboardingState, checking = false) =>
    React.act(async () =>
      root.render(
        <Onboarding
          state={next}
          checking={checking}
          installations={installations}
          gatewayStartup={{ revision: 0, state: "unmanaged" }}
          platform="apple"
          onBegin={() => {}}
          onChoose={() => {}}
          onConfirm={() => {}}
          onFinish={() => {}}
          onRecheck={onRecheck}
          onRetryGateway={() => {}}
        />,
      ),
    )
  await render(state)
  const unmount = async () => {
    await React.act(async () => root.unmount())
    container.remove()
  }
  return { container, render, unmount }
}

const card = (container: HTMLElement) =>
  container.querySelector('[data-agent-download="codex"]')
const codexRow = (container: HTMLElement) =>
  [...container.querySelectorAll("button[aria-pressed]")].find((row) =>
    row.textContent?.includes("Codex"),
  )

it("shows an agent that can be downloaded as a card in its place, and nothing else about downloads", async () => {
  const { source } = installer()
  const state = recordReadiness(asking, { claude: "ready", codex: "not-installed" })
  const { container, unmount } = await picker(state, source)
  expect(card(container)?.getAttribute("aria-label")).toBe("Download Codex")
  expect(codexRow(container)).toBeUndefined()
  // Claude, installed, is just a row to pick.
  expect(container.querySelector('[data-agent-download="claude"]')).toBeNull()
  for (const words of ["Check downloads", "Download only", "Installed"])
    expect(container.textContent).not.toContain(words)
  await unmount()
})

it("downloads from the card, then rechecks so the card becomes the agent's row", async () => {
  const pending = installer()
  const onRecheck = vi.fn()
  const state = recordReadiness(asking, { claude: "ready", codex: "not-installed" })
  const { container, render, unmount } = await picker(state, pending.source, onRecheck)
  const download = card(container)?.querySelector("button")
  download?.focus()
  expect(download?.getAttribute("aria-label")).toBe("Download Codex · 45 MB")
  await React.act(async () => {
    download?.click()
    download?.click()
  })
  expect(pending.source.install).toHaveBeenCalledExactlyOnceWith("codex")
  // The same button, still focusable, now the download's spinner.
  expect(card(container)?.querySelector("button")).toBe(download)
  expect(download?.getAttribute("aria-label")).toBe("Downloading and verifying Codex")
  expect(download?.getAttribute("aria-disabled")).toBe("true")
  pending.source.offers.mockResolvedValue([
    { agent: "codex", archiveBytes: 44_200_000, installed: true },
  ])
  await React.act(async () =>
    pending.finish({ status: "installed", cleanupPending: false }),
  )
  expect(onRecheck).toHaveBeenCalledTimes(1)
  // Installed, and held as a card while the check that follows runs.
  await render(state, true)
  expect(card(container)?.getAttribute("data-download-state")).toBe("installed")
  await render(recordReadiness(asking, { claude: "ready", codex: "ready" }))
  expect(card(container)).toBeNull()
  expect(codexRow(container)?.getAttribute("aria-disabled")).toBeNull()
  // The focus the card's button held is the row's now.
  expect(document.activeElement).toBe(codexRow(container))
  await unmount()
})

it("gives the row back when the check after an install still cannot find the agent", async () => {
  const pending = installer()
  const state = recordReadiness(asking, { claude: "ready", codex: "not-installed" })
  const { container, render, unmount } = await picker(state, pending.source)
  await React.act(async () => card(container)?.querySelector("button")?.click())
  pending.source.offers.mockResolvedValue([
    { agent: "codex", archiveBytes: 44_200_000, installed: true },
  ])
  await React.act(async () =>
    pending.finish({ status: "installed", cleanupPending: false }),
  )
  // Held while no check has run yet, and while it runs.
  expect(card(container)?.getAttribute("data-download-state")).toBe("installed")
  await render(state, true)
  expect(card(container)?.getAttribute("data-download-state")).toBe("installed")
  await render(state, false)
  expect(card(container)).toBeNull()
  expect(codexRow(container)?.getAttribute("aria-label")).toBe("Codex, not installed")
  await unmount()
})

it("holds the agent's place as a card while downloads are still being read", async () => {
  const { source } = installer()
  source.offers.mockReturnValue(new Promise(() => {}))
  const state = recordReadiness(asking, { claude: "ready", codex: "not-installed" })
  const { container, unmount } = await picker(state, source)
  expect(card(container)?.getAttribute("data-download-state")).toBe("reading")
  expect(codexRow(container)).toBeUndefined()
  await unmount()
})

it("keeps a failed download's card, saying why, with a way to check again when the attempt may still run", async () => {
  const pending = installer()
  const state = recordReadiness(asking, { claude: "ready", codex: "not-installed" })
  const { container, unmount } = await picker(state, pending.source)
  await React.act(async () => card(container)?.querySelector("button")?.click())
  await React.act(async () =>
    pending.finish({ status: "failed", reason: "not-confirmed" }),
  )
  expect(card(container)?.textContent).toContain("Not confirmed")
  const again = card(container)?.querySelector("button")
  expect(again?.textContent).toBe("Check again")
  await React.act(async () => again?.click())
  // The fresh read offers the download again.
  expect(card(container)?.querySelector("button")?.getAttribute("aria-label")).toBe(
    "Download Codex · 45 MB",
  )
  await unmount()
})

it("gives the row back once a fresh read no longer offers the agent", async () => {
  const pending = installer()
  const state = recordReadiness(asking, { claude: "ready", codex: "not-installed" })
  const { container, unmount } = await picker(state, pending.source)
  await React.act(async () => card(container)?.querySelector("button")?.click())
  await React.act(async () => pending.finish({ status: "failed", reason: "busy" }))
  pending.source.offers.mockResolvedValue([])
  await React.act(async () => card(container)?.querySelector("button")?.click())
  expect(card(container)).toBeNull()
  expect(codexRow(container)?.getAttribute("aria-label")).toBe("Codex, not installed")
  await unmount()
})

it("says downloads could not be read, with a way to read them again", async () => {
  const { source } = installer()
  source.offers.mockRejectedValueOnce(new Error("connecting"))
  const state = recordReadiness(asking, { claude: "ready", codex: "not-installed" })
  const { container, unmount } = await picker(state, source)
  expect(card(container)?.textContent).toContain("Downloads could not be checked.")
  await React.act(async () => card(container)?.querySelector("button")?.click())
  expect(card(container)?.querySelector("button")?.getAttribute("aria-label")).toBe(
    "Download Codex · 45 MB",
  )
  await unmount()
})

it("lets go of an installed card when no readiness check ever runs", async () => {
  const pending = installer()
  const state = recordReadiness(asking, { claude: "ready", codex: "not-installed" })
  const { container, unmount } = await picker(state, pending.source)
  await React.act(async () => card(container)?.querySelector("button")?.click())
  pending.source.offers.mockResolvedValue([
    { agent: "codex", archiveBytes: 44_200_000, installed: true },
  ])
  vi.useFakeTimers()
  try {
    await React.act(async () =>
      pending.finish({ status: "installed", cleanupPending: false }),
    )
    expect(card(container)?.getAttribute("data-download-state")).toBe("installed")
    await React.act(async () => vi.advanceTimersByTime(5_000))
    expect(card(container)).toBeNull()
    expect(codexRow(container)?.getAttribute("aria-label")).toBe("Codex, not installed")
  } finally {
    vi.useRealTimers()
  }
  await unmount()
})
