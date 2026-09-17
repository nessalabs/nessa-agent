import { describe, expect, it } from "vitest"
import type { ShortcutsDocument } from "@nessa/client"
import defaults from "../../../protocol/defaults/shortcuts.v1.json"
import { acceleratorKeys, formatAccelerator, summonAccelerator } from "./shortcut-display"

const bundled = defaults as ShortcutsDocument

function document(bindings: ShortcutsDocument["bindings"]): ShortcutsDocument {
  return { version: 1, bindings }
}

describe("summon accelerator", () => {
  it("reads the binding the bundled defaults actually ship", () => {
    expect(summonAccelerator(bundled)).toBe("CmdOrCtrl+Shift+D")
  })

  it("takes the first global summon binding, like the host does", () => {
    expect(
      summonAccelerator(
        document([
          {
            keys: "CmdOrCtrl+Shift+T",
            action: "panel.newTab",
            scope: "focused",
            surface: "*",
          },
          {
            keys: "CmdOrCtrl+Shift+A",
            action: "panel.summon",
            scope: "global",
            surface: "*",
          },
          {
            keys: "CmdOrCtrl+Shift+D",
            action: "panel.summon",
            scope: "global",
            surface: "*",
          },
        ]),
      ),
    ).toBe("CmdOrCtrl+Shift+A")
  })

  it("ignores a summon binding that is not global, and reports none", () => {
    expect(
      summonAccelerator(
        document([
          {
            keys: "CmdOrCtrl+Shift+A",
            action: "panel.summon",
            scope: "focused",
            surface: "*",
          },
        ]),
      ),
    ).toBeUndefined()
    expect(summonAccelerator(document([]))).toBeUndefined()
  })
})

describe("writing an accelerator", () => {
  it("writes the same binding the way each platform writes it", () => {
    expect(formatAccelerator("CmdOrCtrl+Shift+D", "apple")).toBe("⌘⇧D")
    expect(formatAccelerator("CmdOrCtrl+Shift+D", "windows")).toBe("Ctrl+Shift+D")
    expect(formatAccelerator("CmdOrCtrl+Shift+D", "linux")).toBe("Ctrl+Shift+D")
  })

  it("splits a shortcut into one key per cap", () => {
    expect(acceleratorKeys("CmdOrCtrl+Shift+D", "apple")).toEqual(["⌘", "⇧", "D"])
    expect(acceleratorKeys("CmdOrCtrl+Shift+D", "windows")).toEqual([
      "Ctrl",
      "Shift",
      "D",
    ])
  })

  it("writes the same modifier as each platform names it", () => {
    expect(acceleratorKeys("Super+K", "apple")).toEqual(["⌘", "K"])
    expect(acceleratorKeys("Super+K", "windows")).toEqual(["Win", "K"])
    expect(acceleratorKeys("Super+K", "linux")).toEqual(["Super", "K"])
    expect(acceleratorKeys("Alt+Escape", "apple")).toEqual(["⌥", "esc"])
    expect(acceleratorKeys("Alt+Escape", "linux")).toEqual(["Alt", "Esc"])
  })

  it("keeps a token it does not recognise instead of dropping it", () => {
    expect(acceleratorKeys("CmdOrCtrl+Space", "apple")).toEqual(["⌘", "Space"])
    expect(formatAccelerator("Hyper+K", "linux")).toBe("Hyper+K")
  })

  it("survives empty and untidy input without inventing keys", () => {
    expect(acceleratorKeys("", "apple")).toEqual([])
    expect(formatAccelerator(" Cmd + Shift + a ", "apple")).toBe("⌘⇧A")
  })
})
