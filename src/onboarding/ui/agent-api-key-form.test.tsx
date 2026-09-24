// @vitest-environment jsdom

import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { AgentApiKeySaveRejected, AgentApiKeySaveUncertain } from "../application/ports"
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
    const save = vi.fn(async () => ({ status: "saved" as const }))
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

  it("retains the key when the native effect is uncertain without telling the user to retry", async () => {
    const save = vi.fn(async () => {
      throw new AgentApiKeySaveUncertain("unknown")
    })
    await React.act(async () => {
      root.render(
        <AgentApiKeyForm
          agent="claude"
          agentName="Claude"
          onSave={save}
          onSaved={() => {}}
        />,
      )
    })

    const input = await enterAndSubmit("sk-private-value")

    expect(input.value).toBe("sk-private-value")
    expect(container.textContent).toContain(
      "could not confirm whether this key was saved",
    )
    expect(container.textContent).toContain("Check Claude sign-in")
    expect(container.textContent).not.toContain("Try again")
    expect(container.textContent).not.toContain("sk-private-value")
  })

  it("keeps a definite refusal separate from its failed outcome audit", async () => {
    const save = vi.fn(async () => {
      throw new AgentApiKeySaveRejected({ reason: "refused", auditStatus: "failed" })
    })
    await React.act(async () => {
      root.render(
        <AgentApiKeyForm
          agent="claude"
          agentName="Claude"
          onSave={save}
          onSaved={() => {}}
        />,
      )
    })

    const input = await enterAndSubmit("sk-private-value")

    expect(input.value).toBe("sk-private-value")
    expect(container.textContent).toContain("key was not saved")
    expect(container.textContent).toContain("could not record")
    expect(container.textContent).not.toContain("sk-private-value")
  })

  it("reports a refresh failure without relabeling the persisted save", async () => {
    const save = vi.fn(async () => ({ status: "saved" as const }))
    const saved = vi.fn(async () => {
      throw new Error("refresh leaked sk-private-value")
    })
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

    expect(save).toHaveBeenCalledOnce()
    expect(saved).toHaveBeenCalledOnce()
    expect(input.value).toBe("")
    expect(container.textContent).toContain("Key saved")
    expect(container.textContent).toContain("could not check Claude again")
    expect(container.textContent).not.toContain("could not save")
    expect(container.textContent).not.toContain("refresh leaked")
    expect(container.textContent).not.toContain("sk-private-value")
  })

  it("treats an audit failure as a confirmed save for its caller to report", async () => {
    const save = vi.fn(async () => ({ status: "saved-audit-failed" as const }))
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

    expect(saved).toHaveBeenCalledOnce()
    expect(input.value).toBe("")
    expect(container.textContent).toContain("Key saved")
    expect(container.textContent).not.toContain("could not save")
    expect(container.textContent).not.toContain("sk-private-value")
  })
})
