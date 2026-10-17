// @vitest-environment jsdom
import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { AgentApiKeySaveUncertain, type AgentReadinessAnswer } from "../application/ports"
import type { InstallationOutcome } from "../application/agent-installations"
import {
  AgentSetupProvider,
  SelectedAgentSetup,
  type AgentSetupSource,
} from "./selected-agent-setup"

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true })
let root: Root
let container: HTMLDivElement
beforeEach(() => {
  container = document.createElement("div")
  document.body.append(container)
  root = createRoot(container)
})
afterEach(async () => {
  await React.act(async () => root.unmount())
  container.remove()
})
function source(
  readiness:
    | "ready"
    | "not-installed"
    | "needs-authentication"
    | "not-configured" = "not-installed",
) {
  return {
    readiness: {
      read: vi.fn(async (): Promise<AgentReadinessAnswer> => ({
        ok: true,
        agents: { claude: readiness, codex: "not-installed" },
      })),
    },
    installations: {
      subscribe: () => () => {},
      offers: vi.fn(async () => [
        { agent: "claude" as const, archiveBytes: 12_000_000, installed: false },
        { agent: "codex" as const, archiveBytes: 24_000_000, installed: false },
      ]),
      install: vi.fn(async (): Promise<InstallationOutcome> => ({
        status: "installed",
        cleanupPending: false,
      })),
    },
    signIn: vi.fn(async () => {}),
    loginAvailable: vi.fn(async () => true),
  }
}
async function render(
  value: AgentSetupSource | undefined,
  agent = "claude",
  recovery = true,
) {
  await React.act(async () =>
    root.render(
      <AgentSetupProvider source={value}>
        <SelectedAgentSetup agent={agent} recovery={recovery} />
      </AgentSetupProvider>,
    ),
  )
}
function button(text: string) {
  const found = [...container.querySelectorAll("button")].find((item) =>
    item.textContent?.includes(text),
  )
  if (!found) throw new Error(`Missing ${text}`)
  return found
}

it("offers only the captured missing agent and makes installation an explicit action", async () => {
  const value = source()
  await render(value)
  expect(container.textContent).toContain("Download Claude · 12 MB")
  expect(container.textContent).not.toContain("Codex")
  expect(value.installations.install).not.toHaveBeenCalled()
  await React.act(async () => button("Download Claude").click())
  expect(value.installations.install).toHaveBeenCalledExactlyOnceWith("claude")
})

it("keeps progress visible and coalesces duplicate clicks while physical installation waits", async () => {
  const value = source()
  let finish!: (value: InstallationOutcome) => void
  value.installations.install.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve
      }),
  )
  await render(value)
  const download = button("Download Claude")
  await React.act(async () => {
    download.click()
    download.click()
  })
  expect(value.installations.install).toHaveBeenCalledTimes(1)
  expect(container.textContent).toContain("Downloading and verifying Claude")
  expect(button("Downloading").disabled).toBe(true)
  expect(container.textContent).toContain("Archive size · 12 MB")
  expect(
    container.querySelector('[role="progressbar"]')?.hasAttribute("aria-valuenow"),
  ).toBe(false)
  await React.act(async () => finish({ status: "installed", cleanupPending: false }))
  expect(container.querySelector('[role="progressbar"]')).toBeNull()
})

it.each(["busy", "not-confirmed"] as const)(
  "requires a new check after %s instead of replaying installation",
  async (reason) => {
    const value = source()
    value.installations.install.mockResolvedValue({ status: "failed", reason })
    await render(value)
    await React.act(async () => button("Download Claude").click())
    expect(
      [...container.querySelectorAll("button")].some((item) =>
        item.textContent?.includes("Download Claude"),
      ),
    ).toBe(false)
    expect(value.installations.install).toHaveBeenCalledTimes(1)
    await React.act(async () => button("Retry").click())
    expect(button("Download Claude")).toBeDefined()
    expect(value.installations.install).toHaveBeenCalledTimes(1)
  },
)

it("does not infer installation need from an absent managed record when readiness is ready or not configured", async () => {
  const value = source("ready")
  await render(value)
  expect(container.textContent).toContain("Ready.")
  expect(container.textContent).not.toContain("Download Claude")
  await render(source("not-configured"))
  expect(container.textContent).toContain(
    "Downloading a runtime alone will not change that",
  )
  expect(container.textContent).not.toContain("Download Claude")
  expect(value.installations.install).not.toHaveBeenCalled()
})

it("keeps authentication separate from installation and login launch acknowledgement", async () => {
  const value = source("needs-authentication")
  await render(value)
  expect(container.textContent).not.toContain("Download Claude")
  await React.act(async () => button("Sign in to Claude").click())
  expect(value.signIn).toHaveBeenCalledExactlyOnceWith("claude")
  expect(container.textContent).toContain("Sign in to your agent account next")
  expect(value.installations.install).not.toHaveBeenCalled()
})

it("does not turn unknown readiness into a missing runtime or show native setup without composition", async () => {
  const value = source()
  value.readiness.read.mockResolvedValue({ ok: false, reason: "unreachable" })
  await render(value)
  expect(container.textContent).toContain("could not check this agent")
  expect(container.textContent).not.toContain("Download Claude")
  await render(undefined)
  expect(container.textContent).toBe("")
})

it("continues first-use download through sign-in and retires the ready hint", async () => {
  const value = source()
  await render(value, "claude", false)
  expect(container.textContent).toContain("Download Claude")
  value.readiness.read.mockResolvedValue({
    ok: true,
    agents: { claude: "needs-authentication" },
  })
  await React.act(async () => button("Download Claude").click())
  expect(container.textContent).toContain("Sign in to Claude")
  expect(value.signIn).not.toHaveBeenCalled()
  value.readiness.read.mockResolvedValue({ ok: true, agents: { claude: "ready" } })
  await React.act(async () => button("Sign in to Claude").click())
  expect(value.signIn).toHaveBeenCalledExactlyOnceWith("claude")
  expect(container.textContent).toBe("")
})

it("retains a first-use failed installation when the next readiness read is unknown", async () => {
  const value = source()
  await render(value, "claude", false)
  value.installations.install.mockResolvedValue({
    status: "failed",
    reason: "verification",
  })
  value.readiness.read.mockResolvedValue({ ok: false, reason: "unreachable" })
  await React.act(async () => button("Download Claude").click())
  expect(container.textContent).toContain("Retry")
  expect(container.textContent).not.toBe("")
})

it("does not publish an old installation to a newly selected agent", async () => {
  const value = source()
  let finish!: (value: InstallationOutcome) => void
  value.installations.install.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve
      }),
  )
  await render(value)
  await React.act(async () => button("Download Claude").click())
  await render(value, "codex")
  await React.act(async () => finish({ status: "installed", cleanupPending: false }))
  expect(container.textContent).toContain("Download Codex")
  expect(container.textContent).not.toContain("Installed.")
  expect(value.installations.install).toHaveBeenCalledExactlyOnceWith("claude")
})

it("does not hold a missing-runtime answer behind an unanswered login capability probe", async () => {
  const value = source()
  value.loginAvailable.mockImplementation(() => new Promise(() => {}))
  await render(value)
  expect(button("Download Claude").disabled).toBe(false)
  expect(container.textContent).not.toContain("Checking Claude")
})

async function saveKey(value: string) {
  const input = container.querySelector("input") as HTMLInputElement
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set
  await React.act(async () => {
    setter?.call(input, value)
    input.dispatchEvent(new Event("input", { bubbles: true }))
  })
  await React.act(async () => {
    container
      .querySelector("form")
      ?.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }))
  })
  return input
}

it.each([true, false])(
  "retains failed key audit evidence after readiness retires the form (recovery %s)",
  async (recovery) => {
    const value = source()
    value.readiness.read.mockResolvedValue({
      ok: true,
      agents: { opencode: "needs-authentication" },
    })
    const save = vi.fn(async () => ({ status: "saved-audit-failed" as const }))
    await render({ ...value, apiKeys: { save } }, "opencode", recovery)
    expect(container.querySelector("input")?.type).toBe("password")
    expect(container.textContent).toContain("messages may be metered")
    value.readiness.read.mockResolvedValue({ ok: true, agents: { opencode: "ready" } })
    await saveKey("fixture-private-key")
    expect(save).toHaveBeenCalledExactlyOnceWith("opencode", "fixture-private-key")
    expect(container.querySelector("input")).toBeNull()
    expect(container.textContent).toContain(
      "The key was saved, but Nessa could not record its security audit record",
    )
    expect(container.textContent).not.toContain("fixture-private-key")
    expect(value.installations.install).not.toHaveBeenCalled()
  },
)

it("keeps an uncertain OpenCode key and its independent failed audit fact", async () => {
  const value = source()
  value.readiness.read.mockResolvedValue({
    ok: true,
    agents: { opencode: "needs-authentication" },
  })
  const save = vi.fn(async () => {
    throw new AgentApiKeySaveUncertain("failed")
  })
  await render({ ...value, apiKeys: { save } }, "opencode")
  const input = await saveKey("fixture-private-key")
  expect(input.value).toBe("fixture-private-key")
  expect(container.textContent).toContain("could not confirm whether this key was saved")
  expect(container.textContent).toContain(
    "could not confirm the key save or record its security audit outcome",
  )
  expect(container.textContent).not.toContain("fixture-private-key")
})

it("gives simultaneous recovery forms distinct accessible labels and hints", async () => {
  const value = source()
  value.readiness.read.mockResolvedValue({
    ok: true,
    agents: { opencode: "needs-authentication" },
  })
  const setup = {
    ...value,
    apiKeys: { save: vi.fn(async () => ({ status: "saved" as const })) },
  }
  await React.act(async () =>
    root.render(
      <AgentSetupProvider source={setup}>
        <SelectedAgentSetup agent="opencode" recovery />
        <SelectedAgentSetup agent="opencode" recovery />
      </AgentSetupProvider>,
    ),
  )
  const inputs = [...container.querySelectorAll("input")]
  expect(inputs).toHaveLength(2)
  expect(new Set(inputs.map((input) => input.id)).size).toBe(2)
  for (const input of inputs) {
    expect(
      [...container.querySelectorAll("label")].some(
        (label) => label.htmlFor === input.id,
      ),
    ).toBe(true)
    expect(
      container.querySelector(`[id="${input.getAttribute("aria-describedby")}"]`),
    ).not.toBeNull()
  }
})

it("offers Claude key entry where this host cannot open Claude's sign-in", async () => {
  const value = source("needs-authentication")
  value.loginAvailable.mockResolvedValue(false)
  const save = vi.fn(async () => ({ status: "saved" as const }))
  await render({ ...value, apiKeys: { save } })
  expect(container.querySelector("input")?.type).toBe("password")
  expect(container.textContent).not.toContain("Sign in to Claude")
  await saveKey("fixture-private-key")
  expect(save).toHaveBeenCalledExactlyOnceWith("claude", "fixture-private-key")
})

it("offers Claude sign-in, not key entry, where the host can open it", async () => {
  const value = source("needs-authentication")
  await render({ ...value, apiKeys: { save: vi.fn() } })
  expect(container.textContent).toContain("Sign in to Claude")
  expect(container.querySelector("input")).toBeNull()
})
