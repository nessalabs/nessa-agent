import { describe, expect, it } from "vitest"
import type { ShortcutsDocument } from "@nessa/client"
import defaults from "../../../protocol/defaults/shortcuts.v1.json"
import {
  acceleratorKeys,
  formatAccelerator,
  heldAcceleratorKeys,
  NOTHING_HELD,
  summonAccelerator,
  type HeldKeys,
} from "./shortcut-display"

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

  /**
   * An accelerator is whatever the host's shortcut document wrote, and the
   * spelling tables are objects, so they answer to names nobody put in them.
   * `toLowerCase` hides most of those — `toString` becomes `tostring`, which
   * is nothing — but `constructor` and `__proto__` are already lowercase and
   * came back as a function and as `Object.prototype`. Both went into a
   * declared `string[]`: a keycap React will not draw, and an accessible name
   * reading `⌘function Object() { [native code] }`.
   */
  it("does not take a keycap off Object.prototype", () => {
    for (const token of ["constructor", "__proto__"]) {
      for (const platform of ["apple", "windows", "linux"] as const) {
        const caps = acceleratorKeys(`CmdOrCtrl+${token}`, platform)
        expect(caps.every((cap) => typeof cap === "string")).toBe(true)
        expect(caps).toEqual([platform === "apple" ? "⌘" : "Ctrl", token])
      }
      expect(formatAccelerator(token, "apple")).toBe(token)
      expect(formatAccelerator(`CmdOrCtrl+${token}`, "linux")).toBe(`Ctrl+${token}`)
    }
  })
})

describe("answering a chord pressed one key at a time", () => {
  const held = (over: Partial<HeldKeys>): HeldKeys => ({ ...NOTHING_HELD, ...over })

  it("lights each cap as its own key goes down", () => {
    const keys = "CmdOrCtrl+Shift+D"
    expect(heldAcceleratorKeys(keys, "apple", NOTHING_HELD)).toEqual([
      false,
      false,
      false,
    ])
    expect(heldAcceleratorKeys(keys, "apple", held({ meta: true }))).toEqual([
      true,
      false,
      false,
    ])
    expect(heldAcceleratorKeys(keys, "apple", held({ meta: true, shift: true }))).toEqual(
      [true, true, false],
    )
    expect(
      heldAcceleratorKeys(keys, "apple", held({ meta: true, shift: true, key: "d" })),
    ).toEqual([true, true, true])
  })

  it("reads CmdOrCtrl as the key that keyboard actually has", () => {
    const keys = "CmdOrCtrl+Shift+D"
    // Command on an Apple keyboard is Control everywhere else, and the cap
    // that lights has to be the one the person is holding.
    expect(heldAcceleratorKeys(keys, "windows", held({ meta: true }))[0]).toBe(false)
    expect(heldAcceleratorKeys(keys, "windows", held({ ctrl: true }))[0]).toBe(true)
    expect(heldAcceleratorKeys(keys, "apple", held({ ctrl: true }))[0]).toBe(false)
  })

  it("does not light a cap for a different key", () => {
    expect(
      heldAcceleratorKeys("CmdOrCtrl+Shift+D", "apple", held({ meta: true, key: "f" })),
    ).toEqual([true, false, false])
  })
})
