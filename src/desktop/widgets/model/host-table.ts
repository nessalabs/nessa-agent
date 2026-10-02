/**
 * What a host draws for a widget in each place (ADR 326's table), and what
 * its chrome calls it. The one statement of that table: every host — the
 * inline card, the widget pane's body, the window — asks it, and draws the
 * answer. A ready app's view has rows of its own beneath it
 * (`app/model/app-view.ts`), which keep this table's rule for close.
 *
 * Nothing here retries or pretends to be live (gates 7 and 16): a source
 * that reads again answers `useWidget` again, and the table is asked again.
 */
import type { OfferedPlaces, WidgetAnswer, WidgetPlace } from "./widget-state"

/**
 * What a host draws: the plugin's own view for the place; a row with the
 * widget's title and Open (inline, ready, with no inline view); a quiet
 * placeholder with the plugin's name while it is not read; or one line,
 * with close where the place has one.
 */
export type HostDraws =
  | { readonly kind: "view" }
  | { readonly kind: "row"; readonly title: string }
  | { readonly kind: "waiting"; readonly name: string }
  | { readonly kind: "line"; readonly text: string; readonly closes: boolean }

/** The lines a host says for a widget it cannot draw, by why. */
export const hostLines = {
  missing: "This is no longer available",
  off: "Turned off in Settings › Advanced › Experimental",
  unshowable: "Can't show this here",
} as const

/** Whether a line drawn in `place` offers close: a place with chrome of its own does, a card in a message does not. */
export function closesIn(place: WidgetPlace): boolean {
  return place !== "inline"
}

/** What a host in `place` draws for `answer`, given the places its plugin offers. */
export function hostDraws(
  place: WidgetPlace,
  answer: WidgetAnswer,
  offered: OfferedPlaces,
): HostDraws {
  const closes = closesIn(place)
  if (!answer.registered) return { kind: "line", text: hostLines.unshowable, closes }
  const { state } = answer
  switch (state.kind) {
    case "ready":
      if (place === "inline")
        return offered.inline ? { kind: "view" } : { kind: "row", title: state.title }
      // Every plugin has a pane view; a window it does not offer is never
      // offered, and asked for all the same, it says so.
      return place === "pane" || offered.window
        ? { kind: "view" }
        : { kind: "line", text: hostLines.unshowable, closes }
    case "unread":
      return { kind: "waiting", name: answer.name }
    case "missing":
    case "off":
    case "unshowable":
      return { kind: "line", text: hostLines[state.kind], closes }
  }
}

/**
 * What a widget's chrome calls it: its title once read, else its plugin's
 * name, and a plain word for a plugin this window does not have.
 */
export function widgetTitle(answer: WidgetAnswer): string {
  if (!answer.registered) return "Unavailable"
  return answer.state.kind === "ready" ? answer.state.title : answer.name
}

/** The session a ready widget belongs to, if any: where its chrome's way back leads. */
export function widgetOrigin(answer: WidgetAnswer): string | undefined {
  return answer.registered && answer.state.kind === "ready"
    ? answer.state.origin
    : undefined
}

/** Whether the window is offered for a widget: ready, and its plugin draws one. */
export function offersWindow(answer: WidgetAnswer, offered: OfferedPlaces): boolean {
  return answer.registered && answer.state.kind === "ready" && offered.window
}
