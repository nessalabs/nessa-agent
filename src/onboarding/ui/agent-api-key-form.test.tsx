// @vitest-environment jsdom

import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { AgentApiKeyForm } from "./agent-api-key-form"

let container: HTMLDivElement
let root: Root

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  container = document.createElement("div")
  document.body.append(container)
  root = createRoot(container)
})

afterEach(async () => {
  await React.act(async () => root.unmount())
  container.remove()
})

async function enterAndSubmit(value: string) {
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

describe("agent API-key entry", () => {
  it("clears a saved key and refreshes readiness without rendering the secret", async () => {
    const save = vi.fn(async () => {})
    const saved = vi.fn()
    await React.act(async () => {
      root.render(
        <AgentApiKeyForm
          agent="claude"
          agentName="Claude"
          onSave={save}
          onSaved={saved}
        />,
      )
    })

    const input = await enterAndSubmit("sk-private-value")

    expect(save).toHaveBeenCalledWith("claude", "sk-private-value")
    expect(saved).toHaveBeenCalledOnce()
    expect(input.value).toBe("")
    expect(container.textContent).toContain("Key saved. Checking Claude again")
    expect(container.textContent).not.toContain("sk-private-value")
  })

  it("shows a fixed safe failure and retains the key for retry", async () => {
    const save = vi.fn(async () => {
      throw new Error("provider leaked sk-private-value")
    })
    await React.act(async () => {
      root.render(
        <AgentApiKeyForm
          agent="opencode"
          agentName="OpenCode"
          onSave={save}
          onSaved={() => {}}
        />,
      )
    })

    const input = await enterAndSubmit("sk-private-value")

    expect(input.value).toBe("sk-private-value")
    expect(container.textContent).toContain("Nessa could not save this key")
    expect(container.textContent).not.toContain("provider leaked")
    expect(container.textContent).not.toContain("sk-private-value")
  })
})
