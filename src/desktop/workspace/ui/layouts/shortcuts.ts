/**
 * The window's keyboard, as data: which chord asks for which command. The
 * keyboard hook runs these and every menu and tooltip labels itself from
 * them, so a chord is named only where it works.
 */
import type { Chord } from "../../../model/keyboard"
import type { Binding } from "../../adapters/dom/shortcuts"
import type { ShortcutCommand } from "../workspace-frame"

const bind = (chord: Chord, command: ShortcutCommand): Binding<ShortcutCommand> => ({
  chord,
  command,
})

/**
 * The window's keyboard, the same in both layouts: the only difference
 * between them is the sidebar's composition, never a key. ⌘K jumps anywhere
 * and ⌘\\ picks what opens beside (the quick switcher, in both); ⌘F searches
 * the session list where there is one, and jumps where there is not.
 */
export const workspaceShortcuts: readonly Binding<ShortcutCommand>[] = [
  bind({ code: "KeyB", command: true }, "toggleSidebar"),
  bind({ code: "KeyS", command: true, alt: true }, "toggleSessionList"),
  bind({ code: "KeyN", command: true }, "newSession"),
  bind({ code: "KeyN", command: true, shift: true }, "newSessionBeside"),
  bind({ code: "KeyW", command: true }, "closePane"),
  bind({ code: "Backslash", command: true }, "openBeside"),
  bind({ code: "Backslash", command: true, shift: true }, "splitDown"),
  bind({ code: "KeyK", command: true }, "switcher"),
  bind({ code: "KeyF", command: true }, "search"),
  bind({ code: "Digit1", command: true }, "focusPane1"),
  bind({ code: "Digit2", command: true }, "focusPane2"),
  bind({ code: "Digit3", command: true }, "focusPane3"),
  bind({ code: "Digit4", command: true }, "focusPane4"),
  // ⇧⌘[ and ⇧⌘], as tabs step elsewhere; ⌘[ and ⌘] are Back and Forward's.
  bind({ code: "BracketLeft", command: true, shift: true }, "focusPrevious"),
  bind({ code: "BracketRight", command: true, shift: true }, "focusNext"),
  // ⌃⌥ and an arrow moves the pane itself.
  bind({ code: "ArrowLeft", control: true, alt: true }, "moveLeft"),
  bind({ code: "ArrowRight", control: true, alt: true }, "moveRight"),
  bind({ code: "ArrowUp", control: true, alt: true }, "moveUp"),
  bind({ code: "ArrowDown", control: true, alt: true }, "moveDown"),
  // The Agents overview, as its sidebar entry opens it: a place, so ⌘0 again keeps it.
  bind({ code: "Digit0", command: true }, "showOverview"),
]

/** What each command is called where the keyboard is listed (Settings › Workspace › Keyboard). */
export const shortcutNames: Readonly<Record<ShortcutCommand, string>> = {
  toggleSidebar: "Show or hide the sidebar",
  toggleSessionList: "Show or hide the session list",
  newSession: "New session",
  newSessionBeside: "New session beside",
  splitRight: "Split right",
  splitDown: "Split down",
  closePane: "Close pane",
  search: "Search the session list",
  switcher: "Jump to a session or channel",
  openBeside: "Open a session beside",
  focusPane1: "Go to pane 1",
  focusPane2: "Go to pane 2",
  focusPane3: "Go to pane 3",
  focusPane4: "Go to pane 4",
  focusPrevious: "Previous pane",
  focusNext: "Next pane",
  moveLeft: "Move pane left",
  moveRight: "Move pane right",
  moveUp: "Move pane up",
  moveDown: "Move pane down",
  showOverview: "Show every agent at a glance",
}
