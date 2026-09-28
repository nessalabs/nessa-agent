/**
 * The agents overview's keyboard, as data: which chord asks for what. The
 * key handlers and every tooltip read these, so a tooltip never names a
 * chord that does something else.
 *
 * ⌘↩ is the Mac's "do the default thing" and ⌘⌫ its "delete, decline"; the
 * arrows walk the list, ↩ opens, ⌘R writes a reply in the peek, and Escape
 * goes back to the panes. ⌘0, which opens the overview from anywhere in the
 * window as its sidebar entry does, is the window's (`ui/layouts/shortcuts.ts`).
 */
import type { Binding, Chord } from "../../adapters/dom/shortcuts"

/** The overview's commands; the three answers are an approval card's own (`ApprovalChoice`). */
export type OverviewCommand =
  | "once"
  | "always"
  | "deny"
  | "open"
  | "next"
  | "previous"
  | "first"
  | "last"
  | "leave"
  | "reply"

const bind = (chord: Chord, command: OverviewCommand): Binding<OverviewCommand> => ({
  chord,
  command,
})

/** The keys inside the overview. */
export const overviewKeys: readonly Binding<OverviewCommand>[] = [
  bind({ code: "Enter", command: true }, "once"),
  bind({ code: "NumpadEnter", command: true }, "once"),
  bind({ code: "Enter", command: true, alt: true }, "always"),
  bind({ code: "Backspace", command: true }, "deny"),
  bind({ code: "Enter" }, "open"),
  bind({ code: "NumpadEnter" }, "open"),
  bind({ code: "ArrowDown" }, "next"),
  bind({ code: "ArrowUp" }, "previous"),
  bind({ code: "Home" }, "first"),
  bind({ code: "End" }, "last"),
  bind({ code: "Escape" }, "leave"),
  bind({ code: "KeyR", command: true }, "reply"),
]
