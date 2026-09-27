/**
 * Each layout's keyboard, as data: which chord asks for which command. The
 * keyboard hook runs these and every menu and tooltip labels itself from
 * them, so a chord is named only where it works.
 */
import type { Binding, Chord } from "../../adapters/dom/shortcuts"
import type { ShortcutCommand } from "../workspace-frame"

const bind = (chord: Chord, command: ShortcutCommand): Binding<ShortcutCommand> => ({
  chord,
  command,
})

/** What both layouts share: panes, sessions and the sidebar. */
const shared: readonly Binding<ShortcutCommand>[] = [
  bind({ code: "KeyB", command: true }, "toggleSidebar"),
  bind({ code: "KeyN", command: true }, "newSession"),
  bind({ code: "KeyW", command: true }, "closePane"),
  bind({ code: "Backslash", command: true, shift: true }, "splitDown"),
  bind({ code: "Digit1", command: true }, "focusPane1"),
  bind({ code: "Digit2", command: true }, "focusPane2"),
  bind({ code: "Digit3", command: true }, "focusPane3"),
  bind({ code: "Digit4", command: true }, "focusPane4"),
  bind({ code: "ArrowLeft", control: true, alt: true }, "moveLeft"),
  bind({ code: "ArrowRight", control: true, alt: true }, "moveRight"),
  bind({ code: "ArrowUp", control: true, alt: true }, "moveUp"),
  bind({ code: "ArrowDown", control: true, alt: true }, "moveDown"),
]

/** Three columns: the session list has its own toggle and search, and ⌘\ splits. */
export const threeColumnsShortcuts: readonly Binding<ShortcutCommand>[] = [
  ...shared,
  bind({ code: "KeyS", command: true, alt: true }, "toggleSessionList"),
  bind({ code: "KeyN", command: true, shift: true }, "newSessionBeside"),
  bind({ code: "Backslash", command: true }, "splitRight"),
  bind({ code: "KeyF", command: true }, "search"),
  bind({ code: "KeyK", command: true }, "search"),
]

/** Sessions in the sidebar: ⌘K jumps anywhere, and ⌘\ picks what opens beside. */
export const sessionsInSidebarShortcuts: readonly Binding<ShortcutCommand>[] = [
  ...shared,
  bind({ code: "KeyK", command: true }, "switcher"),
  bind({ code: "Backslash", command: true }, "openBeside"),
  // ⇧⌘[ and ⇧⌘], as tabs step elsewhere; ⌃⌥ and an arrow moves the pane itself.
  bind({ code: "BracketLeft", command: true, shift: true }, "focusPrevious"),
  bind({ code: "BracketRight", command: true, shift: true }, "focusNext"),
]
