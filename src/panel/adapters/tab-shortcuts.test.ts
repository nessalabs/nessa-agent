import defaults from "../../../protocol/defaults/shortcuts.v1.json"
import { describe, expect, it } from "vitest"

import type { ShortcutsDocument } from "@nessa/client"

import { matchFocusedShortcut, parseAccelerator } from "./tab-shortcuts"

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
      surface: "desktop",
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

describe("relative tab navigation", () => {
  const navigation: ShortcutsDocument = {
    version: 1,
    bindings: [
      {
        keys: "CmdOrCtrl+Shift+H",
        action: "panel.previousTab",
        scope: "focused",
        surface: "desktop",
      },
      {
        keys: "CmdOrCtrl+Shift+L",
        action: "panel.nextTab",
        scope: "focused",
        surface: "desktop",
      },
    ],
  }
  it.each(["desktop"] as const)("matches both directions on %s", (surface) => {
    expect(
      matchFocusedShortcut(
        chord({ key: "H", metaKey: true, shiftKey: true }),
        navigation,
        surface,
      ),
    ).toEqual({ action: "panel.previousTab" })
    expect(
      matchFocusedShortcut(
        chord({ key: "L", metaKey: true, shiftKey: true }),
        navigation,
        surface,
      ),
    ).toEqual({ action: "panel.nextTab" })
    expect(
      matchFocusedShortcut(chord({ key: "h", metaKey: true }), navigation, surface),
    ).toBeNull()
  })
  it("uses configured chords instead of hardcoded keys", () => {
    const custom: ShortcutsDocument = {
      version: 1,
      bindings: [{ ...navigation.bindings[0]!, keys: "Cmd+Alt+J" }],
    }
    expect(
      matchFocusedShortcut(
        chord({ key: "H", metaKey: true, shiftKey: true }),
        custom,
        "desktop",
      ),
    ).toBeNull()
    expect(
      matchFocusedShortcut(
        chord({ key: "j", metaKey: true, altKey: true }),
        custom,
        "desktop",
      ),
    ).toEqual({ action: "panel.previousTab" })
  })
})

it("does not capture the desktop navigation chords in browsers", () => {
  for (const key of ["H", "L"]) {
    expect(
      matchFocusedShortcut(
        chord({ key, metaKey: true, shiftKey: true }),
        defaults as ShortcutsDocument,
        "browser",
      ),
    ).toBeNull()
  }
})
