/**
 * The agents overview's keyboard, as data: which chord asks for what. The
 * key handlers and every tooltip read these, so a tooltip never names a
 * chord that does something else.
 *
 * ⌘↩ is the Mac's "do the default thing" and ⌘⌫ its "delete, decline"; the
 * arrows walk the list, ↩ opens, and Escape goes back to the workspace. ⌘0
 * opens and closes the overview from anywhere in the window.
 */
import type { Binding, Chord } from "../../../workspace/adapters/dom/shortcuts"

export type OverviewCommand =
  "allow" | "always" | "deny" | "open" | "next" | "previous" | "first" | "last" | "leave"

const bind = (chord: Chord, command: OverviewCommand): Binding<OverviewCommand> => ({
  chord,
  command,
})

/** The keys inside the overview. */
export const overviewKeys: readonly Binding<OverviewCommand>[] = [
  bind({ code: "Enter", command: true }, "allow"),
  bind({ code: "NumpadEnter", command: true }, "allow"),
  bind({ code: "Enter", command: true, alt: true }, "always"),
  bind({ code: "Backspace", command: true }, "deny"),
  bind({ code: "Enter" }, "open"),
  bind({ code: "NumpadEnter" }, "open"),
  bind({ code: "ArrowDown" }, "next"),
  bind({ code: "ArrowUp" }, "previous"),
  bind({ code: "Home" }, "first"),
  bind({ code: "End" }, "last"),
  bind({ code: "Escape" }, "leave"),
]

/** The window-wide key that opens and closes the overview. */
export const toggleKeys: readonly Binding<"toggle">[] = [
  { chord: { code: "Digit0", command: true }, command: "toggle" },
]
