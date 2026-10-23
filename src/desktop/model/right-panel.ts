/**
 * The panel on the window's right: the one Classic has, in every layout.
 * Whether it is open and how wide the person dragged it are remembered
 * between launches (`adapters/window-preferences.ts`); maximized is not —
 * it is a moment, which Escape ends.
 */

export interface PanelPlace {
  readonly open: boolean
  /** The width the person dragged it to, before the window's room is applied. */
  readonly width: number
}

export const PANEL_MIN_WIDTH = 260
export const PANEL_MAX_WIDTH = 560
export const PANEL_DEFAULT_WIDTH = 360
/** The most of the window it takes, however wide it was dragged. */
export const PANEL_WINDOW_SHARE = 0.34

export const closedPanel: PanelPlace = { open: false, width: PANEL_DEFAULT_WIDTH }

/** Holds a width within the panel's limits. */
export function clampPanel(width: number): number {
  return Math.round(Math.min(Math.max(width, PANEL_MIN_WIDTH), PANEL_MAX_WIDTH))
}

/** The width the panel is drawn at in a window `windowWidth` wide. */
export function panelWidth(asked: number, windowWidth: number): number {
  return Math.min(
    clampPanel(asked),
    Math.max(PANEL_MIN_WIDTH, Math.floor(windowWidth * PANEL_WINDOW_SHARE)),
  )
}

/** Reads a remembered place; anything this build does not know is a closed panel. */
export function parsePanel(stored: unknown): PanelPlace {
  if (typeof stored !== "string") return closedPanel
  try {
    const value = JSON.parse(stored) as Partial<PanelPlace> | null
    const width =
      typeof value?.width === "number" && Number.isFinite(value.width)
        ? clampPanel(value.width)
        : PANEL_DEFAULT_WIDTH
    return { open: value?.open === true, width }
  } catch {
    return closedPanel
  }
}

export const serializePanel = (place: PanelPlace) => JSON.stringify(place)
