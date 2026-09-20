// @vitest-environment jsdom
/**
 * What a surface with no second window does with the agent setup finished on.
 *
 * The desktop hands the choice to its host and a different window reads it
 * back. A browser has neither: the same component becomes the panel in place,
 * in this page, so what it was told is the only record the choice will ever
 * get. Without this the picker is a control whose selection goes nowhere — the
 * user is shown Codex, picks it, finishes setup, and every conversation runs on
 * the gateway's default with nothing on screen saying so.
 *
 * Driven through the real steps rather than a stand-in state, because the claim
 * is about the whole handover: what the picker recorded has to be what arrives.
 */
import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { SetupHandoff } from "../../host"
import type { AgentReadinessSource } from "../application/ports"

// Only the window calls are stood in for; everything else about the host —
// which platform this is, what the summon chord is — is the real thing, because
// the step the choice has to survive is the one that teaches that chord.
const host = vi.hoisted(() => ({
  revealSetupWindow: vi.fn(async () => {}),
  // What the host answers a page it is not running: the browser case this
  // whole file is about.
  finishSetupWindow: vi.fn(async (): Promise<SetupHandoff> => ({
    outcome: "no-native-host",
  })),
}))
vi.mock("../../host", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../host")>()),
  ...host,
}))
// The opening sound is not what is under test, and jsdom has no audio. Only
// the two calls that would reach for it are stood in for; the mute control the
// setup chrome reads is the real one.
vi.mock("./sound", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./sound")>()),
  playCue: vi.fn(),
  stopCue: vi.fn(),
}))

import { SetupGate } from "./setup-gate"

/** Both agents ready, so the choice is a real one between two options. */
const bothReady: AgentReadinessSource = {
  read: async () => ({ ok: true, agents: { claude: "ready", codex: "ready" } }),
}

let container: HTMLDivElement
let root: Root

async function flush() {
  await React.act(async () => {
    await Promise.resolve()
  })
}

function control(name: string): HTMLButtonElement {
  const found = Array.from(container.querySelectorAll("button")).find(
    (candidate) =>
      candidate.textContent === name || candidate.getAttribute("aria-label") === name,
  )
  expect(found, `no ${name} control`).toBeDefined()
  return found!
}

async function press(name: string) {
  const target = control(name)
  await React.act(async () => {
    target.click()
  })
  await flush()
}

beforeEach(() => {
  // jsdom has no media queries, and the setup light behind the steps asks for
  // one. Answered as "no preference", which is the case that draws everything.
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
  // And no Web Animations either. The steps animate their arrival; nothing
  // here waits on one, so a handle that is already finished is enough.
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
  vi.clearAllMocks()
})

describe("setup on a surface that becomes the panel in place", () => {
  it("hands the agent it finished on to whoever takes over", async () => {
    const handedOver = vi.fn()
    await React.act(async () => {
      root.render(
        React.createElement(
          SetupGate,
          { agents: bothReady, onHandOver: handedOver },
          React.createElement("p", null, "the panel"),
        ),
      )
    })
    await flush()

    await press("Get started")
    await press("Codex")
    await press("Continue")
    expect(
      handedOver,
      "a choice made on the way out is not a decision",
    ).not.toHaveBeenCalled()

    // The summon step's way out. It finishes setup exactly as the primary
    // control does, without this test having to hold a chord.
    await press("Skip this step")

    expect(handedOver).toHaveBeenCalledWith("codex")
    // And the panel is what is on screen now, which is the whole reason this
    // surface has no window to read the choice back in.
    expect(container.textContent).toContain("the panel")
  })

  it("hands over nothing when setup was left rather than finished", async () => {
    const handedOver = vi.fn()
    await React.act(async () => {
      root.render(
        React.createElement(
          SetupGate,
          { agents: bothReady, onHandOver: handedOver },
          React.createElement("p", null, "the panel"),
        ),
      )
    })
    await flush()

    await press("Get started")
    await press("Codex")
    // Leaving from the choice step. The agent is in the state and must not
    // travel: the same rule the host handoff applies.
    await React.act(async () => {
      document.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
      )
    })
    await flush()

    expect(handedOver).not.toHaveBeenCalled()
  })
})
