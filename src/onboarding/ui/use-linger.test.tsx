// @vitest-environment jsdom

import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { AgentReadinessSource } from "../application/ports"
import type { LingerSource } from "../adapters/linger"
import {
  beginOnboarding,
  chooseAgent,
  confirmAgent,
  recordReadiness,
  startAgentChoice,
} from "../model/onboarding"
import { Onboarding } from "./onboarding"
import { useOnboarding } from "./use-onboarding"

vi.mock("../../host", () => ({
  host: { kind: "browser" },
  loadShortcuts: async () => null,
  matchesAccelerator: () => false,
  onSummoned: async () => () => undefined,
  gatewayStartup: async () => ({ revision: 1, state: "unmanaged" }),
  onGatewayStartup: async () => () => undefined,
  retryGatewayStartup: async () => undefined,
}))
vi.mock("./sound", () => ({ playCue: () => undefined }))

const agents: AgentReadinessSource = {
  read: async () => ({ ok: true, agents: {} }),
}

const atSummon = confirmAgent(
  chooseAgent(
    startAgentChoice(recordReadiness(beginOnboarding(), { claude: "ready" })),
    "claude",
  ),
)

let root: Root
let container: HTMLDivElement

function button(name: string) {
  const found = [...container.querySelectorAll("button")].find(
    (candidate) => candidate.textContent?.trim() === name,
  )
  if (!found) throw new Error(`missing button ${name}`)
  return found
}

function Surface({ linger }: { linger: LingerSource }) {
  const onboarding = useOnboarding(agents, atSummon, linger)
  return (
    <div data-step={onboarding.state.step}>
      <Onboarding
        state={onboarding.state}
        gatewayStartup={onboarding.gatewayStartup}
        platform={onboarding.platform}
        accelerator={onboarding.accelerator}
        lingerPending={onboarding.lingerPending}
        onBegin={onboarding.begin}
        onChoose={onboarding.choose}
        onConfirm={onboarding.confirm}
        onFinish={onboarding.finish}
        onAcceptLinger={onboarding.acceptLinger}
        onRecheck={onboarding.recheck}
        onRetryGateway={onboarding.retryGatewayStartup}
      />
    </div>
  )
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
  container = document.createElement("div")
  document.body.append(container)
  root = createRoot(container)
})

afterEach(async () => {
  await React.act(async () => root.unmount())
  container.remove()
})

describe("linger at the end of setup", () => {
  it("finishes when this host has no logind question", async () => {
    const linger: LingerSource = {
      status: async () => ({ shown: "not-applicable" }),
      accept: async () => {
        throw new Error("accept")
      },
    }
    await React.act(async () => {
      root.render(<Surface linger={linger} />)
    })
    await React.act(async () => {
      button("Skip this step").click()
    })
    expect(container.querySelector("[data-step]")?.getAttribute("data-step")).toBe("done")
  })

  it("offers the choice and finishes without enabling when the person declines", async () => {
    let accepts = 0
    const linger: LingerSource = {
      status: async () => ({ shown: "offer" }),
      accept: async () => {
        accepts += 1
        return { shown: "enabled" }
      },
    }
    await React.act(async () => {
      root.render(<Surface linger={linger} />)
    })
    await React.act(async () => {
      button("Skip this step").click()
    })
    expect(container.textContent).toContain("Keep Nessa running when you log out?")
    await React.act(async () => {
      button("Only while I’m signed in").click()
    })
    expect(accepts).toBe(0)
    expect(container.querySelector("[data-step]")?.getAttribute("data-step")).toBe("done")
    expect(container.textContent).not.toContain("keeps running")
  })

  it("shows the next read when the enable reply is lost", async () => {
    let statuses = 0
    const linger: LingerSource = {
      status: async () => {
        statuses += 1
        return statuses === 1 ? { shown: "offer" } : { shown: "enabled" }
      },
      accept: async () => {
        throw new Error("lost")
      },
    }
    await React.act(async () => {
      root.render(<Surface linger={linger} />)
    })
    await React.act(async () => {
      button("Skip this step").click()
    })
    await React.act(async () => {
      button("Keep it running").click()
    })
    expect(statuses).toBe(2)
    expect(container.textContent).toContain("Nessa keeps running when you log out")
  })

  it("keeps the last screen when the repeated read also fails", async () => {
    let statuses = 0
    const linger: LingerSource = {
      status: async () => {
        statuses += 1
        if (statuses === 1) return { shown: "offer" }
        throw new Error("unreadable")
      },
      accept: async () => {
        throw new Error("lost")
      },
    }
    await React.act(async () => {
      root.render(<Surface linger={linger} />)
    })
    await React.act(async () => {
      button("Skip this step").click()
    })
    await React.act(async () => {
      button("Keep it running").click()
    })
    expect(container.querySelector("[data-linger]")?.getAttribute("data-linger")).toBe(
      "offer",
    )
    expect(container.textContent).not.toContain("keeps running")
  })

  it("finishes when the ask fails", async () => {
    const linger: LingerSource = {
      status: async () => {
        throw new Error("unreachable")
      },
      accept: async () => ({ shown: "enabled" }),
    }
    await React.act(async () => {
      root.render(<Surface linger={linger} />)
    })
    await React.act(async () => {
      button("Skip this step").click()
    })
    expect(container.querySelector("[data-step]")?.getAttribute("data-step")).toBe("done")
    expect(container.textContent).not.toContain("keeps running")
  })

  it("sends one enable for two clicks in the same turn", async () => {
    let accepts = 0
    let release: (() => void) | undefined
    const gate = new Promise<void>((resolve) => {
      release = resolve
    })
    const linger: LingerSource = {
      status: async () => ({ shown: "offer" }),
      accept: async () => {
        accepts += 1
        await gate
        return { shown: "enabled" }
      },
    }
    const calls: { accept?: () => void } = {}
    function Harness() {
      const onboarding = useOnboarding(agents, atSummon, linger)
      calls.accept = onboarding.acceptLinger
      return (
        <div data-step={onboarding.state.step}>
          <Onboarding
            state={onboarding.state}
            gatewayStartup={onboarding.gatewayStartup}
            platform={onboarding.platform}
            accelerator={onboarding.accelerator}
            lingerPending={onboarding.lingerPending}
            onBegin={onboarding.begin}
            onChoose={onboarding.choose}
            onConfirm={onboarding.confirm}
            onFinish={onboarding.finish}
            onAcceptLinger={onboarding.acceptLinger}
            onRecheck={onboarding.recheck}
            onRetryGateway={onboarding.retryGatewayStartup}
          />
        </div>
      )
    }
    await React.act(async () => {
      root.render(<Harness />)
    })
    await React.act(async () => {
      button("Skip this step").click()
    })
    await React.act(async () => {
      calls.accept?.()
      calls.accept?.()
    })
    expect(accepts).toBe(1)
    expect(container.textContent).toContain("Keep Nessa running when you log out?")
    await React.act(async () => {
      release?.()
    })
    expect(container.textContent).toContain("Nessa keeps running when you log out")
  })
})
