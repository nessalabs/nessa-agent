// @vitest-environment jsdom

import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import type { GatewayStartup } from "../../startup/application/ports"
import type { AgentReadinessSource } from "../application/ports"

const native = vi.hoisted(() => ({ handlers: [] as ((value: GatewayStartup) => void)[] }))
vi.mock("../../host", () => ({
  host: { kind: "browser" },
  loadShortcuts: async () => null,
  matchesAccelerator: () => false,
  onSummoned: async () => () => undefined,
  gatewayStartup: async () => ({ revision: 1, state: "ready" }),
  onGatewayStartup: async (handler: (value: GatewayStartup) => void) => {
    native.handlers.push(handler)
    return () => undefined
  },
  retryGatewayStartup: async () => undefined,
}))
vi.mock("./sound", () => ({ playCue: () => undefined }))

import { httpAgentReadiness } from "../adapters/agents"
import { beginOnboarding, startAgentChoice } from "../model/onboarding"
import { Onboarding } from "./onboarding"
import { useOnboarding } from "./use-onboarding"

let root: Root
let container: HTMLDivElement
function Surface({ source }: { source: AgentReadinessSource }) {
  const onboarding = useOnboarding(source, startAgentChoice(beginOnboarding()))
  return (
    <>
      <output>{`${onboarding.checking}:${onboarding.state.readiness?.claude ?? "unknown"}`}</output>
      <Onboarding
        state={onboarding.state}
        gatewayStartup={onboarding.gatewayStartup}
        checking={onboarding.checking}
        platform={onboarding.platform}
        onBegin={onboarding.begin}
        onChoose={onboarding.choose}
        onConfirm={onboarding.confirm}
        onFinish={onboarding.finish}
        onRecheck={onboarding.recheck}
        onRetryGateway={onboarding.retryGatewayStartup}
      />
    </>
  )
}
function recheck() {
  const button = [...container.querySelectorAll("button")].find((button) =>
    ["Checking…", "Check again"].includes(button.textContent?.trim() ?? ""),
  )
  if (!button) throw new Error("The readiness retry control is absent")
  return button
}

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  window.matchMedia ??= (() => ({
    matches: false,
    addEventListener: () => undefined,
    removeEventListener: () => undefined,
  })) as unknown as typeof window.matchMedia
  Element.prototype.animate ??= (() => ({
    finished: Promise.resolve(),
    cancel: () => undefined,
  })) as unknown as typeof Element.prototype.animate
  native.handlers = []
  container = document.createElement("div")
  document.body.append(container)
  root = createRoot(container)
  vi.useFakeTimers()
})
afterEach(async () => {
  await React.act(async () => root.unmount())
  container.remove()
  vi.useRealTimers()
})

it.each(["fetch", "body"] as const)(
  "releases Check again when %s stalls and retains the retry answer over a late old response",
  async (phase) => {
    let release!: (value: unknown) => void
    const held = new Promise<unknown>((resolve) => {
      release = resolve
    })
    const fetch = vi.fn<typeof globalThis.fetch>()
    if (phase === "fetch") fetch.mockReturnValueOnce(held as Promise<Response>)
    else fetch.mockResolvedValueOnce({ ok: true, json: () => held } as Response)
    fetch.mockResolvedValue({
      ok: true,
      json: async () => ({ agents: [{ id: "claude", readiness: "ready" }] }),
    } as Response)
    const source = httpAgentReadiness({ baseUrl: "", fetch })
    await React.act(async () => root.render(<Surface source={source} />))
    expect(fetch).toHaveBeenCalledTimes(1)
    expect(recheck().disabled).toBe(true)
    await React.act(async () => {
      await vi.advanceTimersByTimeAsync(10_000)
    })
    expect(recheck().textContent).toBe("Check again")
    expect(recheck().disabled).toBe(false)
    expect(fetch.mock.calls[0][1]?.signal?.aborted).toBe(true)
    await React.act(async () => recheck().click())
    expect(fetch).toHaveBeenCalledTimes(2)
    expect(container.querySelector("output")?.textContent).toBe("false:ready")
    await React.act(async () => {
      release(phase === "fetch" ? { ok: false } : { agents: [] })
      await held
    })
    expect(container.querySelector("output")?.textContent).toBe("false:ready")
  },
)

it("discards the deadline of an old gateway while its replacement remains busy", async () => {
  const fetch = vi.fn<typeof globalThis.fetch>(
    () => new Promise<Response>(() => undefined),
  )
  const source = httpAgentReadiness({ baseUrl: "", fetch })
  await React.act(async () => root.render(<Surface source={source} />))
  await React.act(async () => {
    await vi.advanceTimersByTimeAsync(5_000)
  })
  await React.act(async () => {
    native.handlers.at(-1)?.({ revision: 2, state: "ready" })
  })
  expect(fetch).toHaveBeenCalledTimes(2)
  await React.act(async () => {
    await vi.advanceTimersByTimeAsync(5_000)
  })
  expect(fetch.mock.calls[0][1]?.signal?.aborted).toBe(true)
  expect(fetch.mock.calls[1][1]?.signal?.aborted).toBe(false)
  expect(recheck().disabled).toBe(true)
  await React.act(async () => {
    await vi.advanceTimersByTimeAsync(5_000)
  })
  expect(fetch.mock.calls[1][1]?.signal?.aborted).toBe(true)
  expect(recheck().disabled).toBe(false)
})
