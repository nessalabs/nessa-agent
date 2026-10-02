/**
 * How an app's display modes map onto the window's places (ADR 344): `inline`
 * is a card in the message; `fullscreen` is a pane beside the conversation,
 * and the window — where a sidebar entry opens an app — is drawn as
 * fullscreen too; `pip` is never offered.
 *
 * Each place draws its own view, so a request changes where *another* view
 * is drawn, never this one: an inline view asking for fullscreen opens a
 * pane and stays inline, and says so in its answer, as the spec asks
 * ("Host MUST return the resulting mode").
 */
import type { OpenPlace, WidgetPlace } from "../../model/widget-state"
import type { DisplayMode } from "./messages"

/** The modes the host offers any app (`hostContext.availableDisplayModes`). */
export const offeredModes: readonly DisplayMode[] = ["inline", "fullscreen"]

/** The mode a view drawn in `place` is in. */
export function modeOf(place: WidgetPlace): DisplayMode {
  return place === "inline" ? "inline" : "fullscreen"
}

/** What the host does for a view in `place` asking for `mode`, and what it answers. */
export interface ModeDecision {
  /** The place the host opens for the widget, if any. */
  readonly open?: OpenPlace
  /** The view's mode afterwards: always its own, as views do not move. */
  readonly answer: DisplayMode
}

/**
 * The decision for a request. Only an inline view asking for fullscreen
 * opens anything — a pane — and only when the app said it can be shown so
 * (`declared`, its `appCapabilities.availableDisplayModes`, when it gave
 * any): a mode it did not declare is never switched to.
 */
export function requestMode(
  place: WidgetPlace,
  mode: DisplayMode,
  declared: readonly DisplayMode[] | undefined,
): ModeDecision {
  const answer = modeOf(place)
  const allowed = offeredModes.includes(mode) && (declared?.includes(mode) ?? true)
  if (place === "inline" && mode === "fullscreen" && allowed)
    return { open: "pane", answer }
  return { answer }
}
