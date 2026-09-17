import { describe, expect, it } from "vitest"
import type { ShortcutsDocument } from "@nessa/client"
import defaults from "../../../protocol/defaults/shortcuts.v1.json"
import { formatAccelerator, summonAccelerator } from "./shortcut-display"

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
  it("uses Apple glyphs on macOS and words elsewhere", () => {
    expect(formatAccelerator("CmdOrCtrl+Shift+D", "macos")).toBe("⌘⇧D")
    expect(formatAccelerator("CmdOrCtrl+Shift+D", "linux")).toBe("Ctrl+Shift+D")
    expect(formatAccelerator("CmdOrCtrl+Shift+D", "browser")).toBe("Ctrl+Shift+D")
  })

  it("keeps a token it does not recognise instead of dropping it", () => {
    expect(formatAccelerator("CmdOrCtrl+Space", "macos")).toBe("⌘Space")
    expect(formatAccelerator("Hyper+K", "linux")).toBe("Hyper+K")
  })

  it("survives empty and untidy input without inventing keys", () => {
    expect(formatAccelerator("", "macos")).toBe("")
    expect(formatAccelerator(" Cmd + Shift + a ", "macos")).toBe("⌘⇧A")
  })
})
