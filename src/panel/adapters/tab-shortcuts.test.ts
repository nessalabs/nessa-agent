import { describe, expect, it } from "vitest"

import type { ShortcutsDocument } from "@nessa/client"

import {
  matchFocusedShortcut,
  parseAccelerator,
} from "./tab-shortcuts"

const sample: ShortcutsDocument = {
  version: 1,
  bindings: [
    {
      keys: "CmdOrCtrl+T",
      action: "panel.newTab",
      scope: "focused",
      surface: "desktop",
    },
    {
      keys: "CmdOrCtrl+N",
      action: "panel.newTab",
      scope: "focused",
      surface: "desktop",
    },
    {
      keys: "CmdOrCtrl+W",
      action: "panel.closeTab",
      scope: "focused",
      surface: "desktop",
    },
    {
      keys: "CmdOrCtrl+1",
      action: "panel.activateTab",
      args: { index: 0 },
      scope: "focused",
      surface: "desktop",
    },
    {
      keys: "CmdOrCtrl+Shift+T",
      action: "panel.newTab",
      scope: "focused",
      surface: "browser",
    },
    {
      keys: "CmdOrCtrl+Shift+W",
      action: "panel.closeTab",
      scope: "focused",
      surface: "browser",
    },
    {
      keys: "CmdOrCtrl+Shift+1",
      action: "panel.activateTab",
      args: { index: 0 },
      scope: "focused",
      surface: "browser",
    },
    {
      keys: "CmdOrCtrl+Shift+D",
      action: "panel.summon",
      scope: "global",
      surface: "*",
    },
  ],
}

function chord(
  partial: Partial<{
    key: string
    code: string
    metaKey: boolean
    ctrlKey: boolean
    altKey: boolean
    shiftKey: boolean
    repeat: boolean
  }>,
) {
  return {
    key: "a",
    code: "KeyA",
    metaKey: false,
    ctrlKey: false,
    altKey: false,
    shiftKey: false,
    repeat: false,
    ...partial,
  }
}

describe("parseAccelerator", () => {
  it("parses CmdOrCtrl+Shift+D", () => {
    expect(parseAccelerator("CmdOrCtrl+Shift+D")).toEqual({
      cmdOrCtrl: true,
      meta: false,
      ctrl: false,
      alt: false,
      shift: true,
      key: "d",
    })
  })
})

describe("matchFocusedShortcut", () => {
  it("opens and closes on desktop without Shift", () => {
    expect(
      matchFocusedShortcut(
        chord({ key: "t", code: "KeyT", metaKey: true }),
        sample,
        "desktop",
      ),
    ).toEqual({ action: "panel.newTab" })

    expect(
      matchFocusedShortcut(
        chord({ key: "w", code: "KeyW", ctrlKey: true }),
        sample,
        "desktop",
      ),
    ).toEqual({ action: "panel.closeTab" })
  })

  it("requires Shift on the browser surface", () => {
    expect(
      matchFocusedShortcut(
        chord({ key: "t", code: "KeyT", metaKey: true }),
        sample,
        "browser",
      ),
    ).toBeNull()

    expect(
      matchFocusedShortcut(
        chord({ key: "t", code: "KeyT", metaKey: true, shiftKey: true }),
        sample,
        "browser",
      ),
    ).toEqual({ action: "panel.newTab" })
  })

  it("activates by index and ignores summon / repeat", () => {
    expect(
      matchFocusedShortcut(
        chord({ key: "1", code: "Digit1", metaKey: true }),
        sample,
        "desktop",
      ),
    ).toEqual({ action: "panel.activateTab", index: 0, conversationId: undefined })

    expect(
      matchFocusedShortcut(
        chord({ key: "d", code: "KeyD", metaKey: true, shiftKey: true }),
        sample,
        "desktop",
      ),
    ).toBeNull()

    expect(
      matchFocusedShortcut(
        chord({ key: "t", code: "KeyT", metaKey: true, repeat: true }),
        sample,
        "desktop",
      ),
    ).toBeNull()
  })
})
