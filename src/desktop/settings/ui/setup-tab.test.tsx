// @vitest-environment jsdom
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

const host = vi.hoisted(() => ({
  native: true,
  replayOnboarding: vi.fn<() => Promise<void>>(),
}))
vi.mock("../../../host", async (original) => ({
  ...(await original<typeof import("../../../host")>()),
  hasNativeHost: () => host.native,
  replayOnboarding: host.replayOnboarding,
}))

import { settingsTabPages } from "./settings-tabs"

let container: HTMLDivElement
let root: Root

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  host.native = true
  host.replayOnboarding.mockReset()
  container = document.createElement("div")
  document.body.append(container)
  root = createRoot(container)
})

afterEach(() => {
  act(() => root.unmount())
  container.remove()
})

async function mount() {
  const Page = settingsTabPages.setup
  await act(async () => root.render(<Page />))
}

const replay = () =>
  container.querySelector<HTMLButtonElement>('[data-setting="replay-onboarding"] button')

describe("Settings › Advanced › Setup", () => {
  it("asks the host to show setup again, once per press", async () => {
    host.replayOnboarding.mockResolvedValue(undefined)
    await mount()
    expect(container.textContent).toContain("Replay onboarding")
    expect(replay()?.textContent).toBe("Replay")
    await act(async () => replay()?.click())
    expect(host.replayOnboarding).toHaveBeenCalledTimes(1)
    expect(replay()?.disabled).toBe(false)
  })

  it("rests while the ask is open, and says so when setup did not open", async () => {
    let refuse: (error: Error) => void = () => undefined
    host.replayOnboarding.mockReturnValue(new Promise((_, reject) => (refuse = reject)))
    await mount()
    await act(async () => replay()?.click())
    expect(replay()?.disabled).toBe(true)
    await act(async () => refuse(new Error("no window")))
    expect(replay()?.disabled).toBe(false)
    expect(container.textContent).toContain("Setup did not open.")
  })

  it("offers no button where there is no setup window to open", async () => {
    host.native = false
    await mount()
    expect(replay()).toBeNull()
    expect(container.textContent).toContain("Setup can be replayed in the Nessa app.")
  })
})
