// @vitest-environment jsdom

import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { AgentApiKeySaveUncertain, type AgentApiKeySink } from "../application/ports"
import {
  beginOnboarding,
  recordReadiness,
  startAgentChoice,
  type OnboardingState,
} from "../model/onboarding"
import { Onboarding } from "./onboarding"

let container: HTMLDivElement
let root: Root

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  window.matchMedia ??= ((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    dispatchEvent: () => false,
  })) as unknown as typeof window.matchMedia
  Element.prototype.animate ??= (() => ({
    finished: Promise.resolve(),
    cancel: () => {},
    finish: () => {},
    addEventListener: () => {},
    removeEventListener: () => {},
  })) as unknown as typeof Element.prototype.animate
  container = document.createElement("div")
  document.body.append(container)
  root = createRoot(container)
})

afterEach(async () => {
  await React.act(async () => root.unmount())
  container.remove()
})

describe("onboarding credential-save outcomes", () => {
  it("retains an audit warning after readiness removes the cleared key form", async () => {
    const asking = startAgentChoice(beginOnboarding())
    let state: OnboardingState = recordReadiness(asking, {
      claude: "needs-authentication",
    })
    const save = vi.fn(async () => ({ status: "saved-audit-failed" as const }))
    const apiKeys: AgentApiKeySink = { save }
    let releaseRefresh = () => {}
    const refreshReleased = new Promise<void>((resolve) => {
      releaseRefresh = resolve
    })
    let reportRefreshStarted = () => {}
    const refreshStarted = new Promise<void>((resolve) => {
      reportRefreshStarted = resolve
    })
    const onRecheck = vi.fn(async () => {
      reportRefreshStarted()
      await refreshReleased
      state = recordReadiness(asking, { claude: "ready" })
      root.render(view())
    })
    const view = () => (
      <Onboarding
        state={state}
        gatewayStartup={{ revision: 1, state: "ready" }}
        apiKeys={apiKeys}
        platform="apple"
        onBegin={() => {}}
        onChoose={() => {}}
        onConfirm={() => {}}
        onFinish={() => {}}
        onRecheck={onRecheck}
        onRetryGateway={() => {}}
      />
    )

    await React.act(async () => root.render(view()))
    const input = container.querySelector("input") as HTMLInputElement
    const setter = Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )?.set
    await React.act(async () => {
      setter?.call(input, "sk-private-value")
      input.dispatchEvent(new Event("input", { bubbles: true }))
      container
        .querySelector("form")
        ?.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }))
      await refreshStarted
    })

    expect(save).toHaveBeenCalledOnce()
    expect(onRecheck).toHaveBeenCalledOnce()
    expect(input.value).toBe("")

    await React.act(async () => {
      releaseRefresh()
      await refreshReleased
    })

    expect(container.querySelector("input")).toBeNull()
    expect(container.textContent).toContain(
      "The key was saved, but Nessa could not record its security audit record.",
    )
    expect(container.textContent).not.toContain("sk-private-value")
  })

  it("keeps uncertain effect and failed audit facts separate without clearing the key", async () => {
    const state = recordReadiness(startAgentChoice(beginOnboarding()), {
      claude: "needs-authentication",
    })
    const save = vi.fn(async () => {
      throw new AgentApiKeySaveUncertain("failed")
    })
    const onRecheck = vi.fn()
    await React.act(async () => {
      root.render(
        <Onboarding
          state={state}
          gatewayStartup={{ revision: 1, state: "ready" }}
          apiKeys={{ save }}
          platform="apple"
          onBegin={() => {}}
          onChoose={() => {}}
          onConfirm={() => {}}
          onFinish={() => {}}
          onRecheck={onRecheck}
          onRetryGateway={() => {}}
        />,
      )
    })
    const input = container.querySelector("input") as HTMLInputElement
    const setter = Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )?.set
    await React.act(async () => {
      setter?.call(input, "sk-private-value")
      input.dispatchEvent(new Event("input", { bubbles: true }))
      container
        .querySelector("form")
        ?.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }))
    })

    expect(save).toHaveBeenCalledOnce()
    expect(onRecheck).not.toHaveBeenCalled()
    expect(input.value).toBe("sk-private-value")
    expect(container.textContent).toContain(
      "could not confirm whether this key was saved",
    )
    expect(container.textContent).toContain(
      "Nessa could not confirm the key save or record its security audit outcome.",
    )
    expect(container.textContent).not.toContain("sk-private-value")
  })
})
