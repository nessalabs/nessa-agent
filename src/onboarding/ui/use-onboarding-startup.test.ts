// @vitest-environment jsdom
import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { GatewayStartup } from "../../startup/application/ports"

const native = vi.hoisted(() => ({
  handlers: [] as ((startup: GatewayStartup) => void)[],
  gatewayStartup: vi.fn<() => Promise<GatewayStartup>>(),
  onGatewayStartup:
    vi.fn<(handler: (startup: GatewayStartup) => void) => Promise<() => void>>(),
  retryGatewayStartup: vi.fn<() => Promise<void>>(),
  stop: vi.fn(),
}))

vi.mock("../../host", () => ({
  host: { kind: "browser" },
  loadShortcuts: async () => null,
  matchesAccelerator: () => false,
  onSummoned: async () => () => undefined,
  gatewayStartup: native.gatewayStartup,
  onGatewayStartup: native.onGatewayStartup,
  retryGatewayStartup: native.retryGatewayStartup,
}))
vi.mock("./sound", () => ({ playCue: () => undefined }))

import type { AgentReadinessSource } from "../application/ports"
import { useOnboarding } from "./use-onboarding"

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((res) => {
    resolve = res
  })
  return { promise, resolve }
}

function Surface({ agents }: { agents: AgentReadinessSource }) {
  const onboarding = useOnboarding(agents)
  return React.createElement(
    React.Fragment,
    null,
    React.createElement(
      "output",
      null,
      `${onboarding.gatewayStartup.state}:${onboarding.checking}`,
    ),
    React.createElement(
      "output",
      { "data-readiness": true },
      `${onboarding.state.readiness?.claude ?? "unknown"}:${onboarding.state.readinessFailure ?? "-"}`,
    ),
    React.createElement("button", { onClick: onboarding.begin }, "Begin"),
    React.createElement("button", { onClick: onboarding.retryGatewayStartup }, "Retry"),
  )
}

let container: HTMLDivElement
let root: Root

async function render(agents: AgentReadinessSource) {
  await React.act(async () => {
    root.render(
      React.createElement(
        React.StrictMode,
        null,
        React.createElement(Surface, { agents }),
      ),
    )
  })
}

async function flush() {
  await React.act(async () => {
    await Promise.resolve()
    await Promise.resolve()
  })
}

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  native.handlers.length = 0
  native.gatewayStartup.mockReset()
  native.onGatewayStartup.mockReset().mockImplementation(async (handler) => {
    native.handlers.push(handler)
    return native.stop
  })
  native.retryGatewayStartup.mockReset().mockResolvedValue(undefined)
  native.stop.mockReset()
  container = document.createElement("div")
  document.body.appendChild(container)
  root = createRoot(container)
})

afterEach(async () => {
  await React.act(async () => root.unmount())
  container.remove()
})

describe("the setup owner under Strict Mode", () => {
  it("asks readiness once after a newer ready event beats the startup snapshot", async () => {
    const snapshot = deferred<GatewayStartup>()
    native.gatewayStartup.mockReturnValue(snapshot.promise)
    const agents = { read: vi.fn(async () => ({ ok: true as const, agents: {} })) }

    await render(agents)
    await flush()
    expect(native.handlers.length).toBeGreaterThan(0)
    await React.act(async () => {
      native.handlers.at(-1)!({ revision: 2, state: "ready" })
      snapshot.resolve({ revision: 1, state: "starting", step: "preparing" })
      await snapshot.promise
    })
    await flush()

    expect(container.querySelector("output")?.textContent).toBe("ready:false")
    expect(agents.read).toHaveBeenCalledTimes(1)
  })

  it.each([
    {
      stale: { ok: false as const, reason: "unreachable" as const },
      name: "unreachable failure",
    },
    {
      stale: { ok: true as const, agents: { claude: "ready" as const } },
      name: "ready answer",
    },
  ])(
    "replaces a pending $name when a newer ready identity arrives without starting",
    async ({ stale }) => {
      native.gatewayStartup.mockResolvedValue({ revision: 2, state: "ready" })
      const first = deferred<Awaited<ReturnType<AgentReadinessSource["read"]>>>()
      const second = deferred<Awaited<ReturnType<AgentReadinessSource["read"]>>>()
      const agents = {
        read: vi
          .fn()
          .mockReturnValueOnce(first.promise)
          .mockReturnValueOnce(second.promise),
      }

      await render(agents)
      await flush()
      expect(agents.read).toHaveBeenCalledTimes(1)

      await React.act(async () => {
        native.handlers.at(-1)!({ revision: 4, state: "ready" })
      })
      await flush()
      expect(agents.read).toHaveBeenCalledTimes(2)
      expect(container.querySelector("[data-readiness]")?.textContent).toBe("unknown:-")

      await React.act(async () => first.resolve(stale))
      await flush()
      expect(container.querySelector("[data-readiness]")?.textContent).toBe("unknown:-")

      await React.act(async () =>
        second.resolve({ ok: true, agents: { claude: "not-installed" } }),
      )
      await flush()
      expect(container.querySelector("[data-readiness]")?.textContent).toBe(
        "not-installed:-",
      )
    },
  )

  it("preserves the browser asks when the native lifecycle is unmanaged", async () => {
    native.gatewayStartup.mockResolvedValue({ revision: 0, state: "unmanaged" })
    const agents = { read: vi.fn(async () => ({ ok: true as const, agents: {} })) }

    await render(agents)
    await flush()
    expect(agents.read).toHaveBeenCalledTimes(1)

    await React.act(async () => {
      container.querySelector("button")?.click()
    })
    await flush()

    expect(agents.read).toHaveBeenCalledTimes(2)
  })

  it("sends an explicit retry to the host owner and refreshes its snapshot", async () => {
    native.gatewayStartup.mockResolvedValue({
      revision: 1,
      state: "failed",
      message: "startup failed",
    })
    const agents = { read: vi.fn(async () => ({ ok: true as const, agents: {} })) }
    await render(agents)
    await flush()

    await React.act(async () => {
      Array.from(container.querySelectorAll("button"))
        .find((button) => button.textContent === "Retry")
        ?.click()
    })
    await flush()

    expect(native.retryGatewayStartup).toHaveBeenCalledOnce()
    expect(native.gatewayStartup).toHaveBeenCalledTimes(2)
  })

  it("removes the live subscription and ignores events after unmount", async () => {
    native.gatewayStartup.mockResolvedValue({ revision: 1, state: "starting", step: "preparing" })
    const agents = { read: vi.fn(async () => ({ ok: true as const, agents: {} })) }
    await render(agents)
    await flush()
    const publish = native.handlers.at(-1)!

    await React.act(async () => root.unmount())
    publish({ revision: 2, state: "ready" })
    await flush()

    expect(native.stop).toHaveBeenCalled()
    expect(agents.read).not.toHaveBeenCalled()
    expect(container.innerHTML).toBe("")
  })
})
