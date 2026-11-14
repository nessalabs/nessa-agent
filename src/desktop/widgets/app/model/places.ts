/**
 * How an app's display modes map onto the window's places (ADR 344): `inline`
 * is a card in the message; `fullscreen` is a tab in the conversation's panel,
 * and `pip` is never offered. The generic window-place mapping below is
 * fullscreen too; apps do not offer that destination (`offeredBy`).
 *
 * A reply describes its requesting view's mode. Before an inline request
 * opens a panel tab, the bridge posts that caller's inline answer (L17a).
 * The host then retires the inline copy: the app has one live view (#1220,
 * `host-table.test.ts`), not two copies of the app and its microphone.
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
 * opens anything — a panel tab — and only when the app said it can be shown so
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
    return { open: "panel", answer }
  return { answer }
}
