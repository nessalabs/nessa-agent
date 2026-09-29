import { afterEach, describe, expect, it, vi } from "vitest"
import { workspaceShortcuts } from "../../ui/layouts/shortcuts"
import { labelOf } from "./shortcuts"

// The platform the window read, as each test says: a Mac unless it says otherwise.
const platform = vi.hoisted(() => ({ mac: true }))
vi.mock("../../../adapters/platform", () => ({
  get isMac() {
    return platform.mac
  },
}))

afterEach(() => {
  platform.mac = true
})

describe("labelling bound commands", () => {
  it("labels a command by the first chord bound to it", () => {
    const bindings = [
      { chord: { code: "KeyK", command: true }, command: "search" },
      { chord: { code: "KeyF", command: true }, command: "search" },
    ] as const
    expect(labelOf(bindings, "search")?.endsWith("K")).toBe(true)
    expect(labelOf(bindings, "other" as "search")).toBeUndefined()
  })

  for (const [mac, label] of [
    [true, "⌥⌘S"],
    [false, "Ctrl+Alt+S"],
  ] as const)
    it(`writes the workspace's keys as ${mac ? "a Mac" : "elsewhere"} does`, () => {
      platform.mac = mac
      expect(labelOf(workspaceShortcuts, "toggleSessionList")).toBe(label)
    })
})
