import { useLayoutEffect } from "react"
import type { Chord } from "../model/keyboard"
import {
  DEFAULT_ZOOM,
  parseZoom,
  zoomedIn,
  zoomedOut,
  type WindowZoom,
} from "../model/window-zoom"
import { useKeyBindings, type Binding } from "./key-bindings"
import { storedPreference } from "./stored-preference"

/**
 * The window's zoom (`model/window-zoom.ts`): remembered like the window's
 * other preferences, and put into effect by the host, which zooms the whole
 * page as a browser does. The page never scales itself: a CSS `zoom` or a
 * transform would leave every measured length (a drag, a FLIP, a resize) in
 * a different unit from the styles written back.
 *
 * Only a native host can zoom its webview. In a browser the browser's own
 * zoom is the person's, so nothing is bound there and no key is taken
 * (`zoomAvailable`).
 *
 * The zoom in effect is carried on the page's root as `--desktop-zoom` and
 * `data-zoom`, once the host has applied it. The stylesheet divides by it
 * the few lengths the host owns in screen points, which the page's zoom does
 * not scale (the room for the traffic lights, `styles.css`).
 */
export interface WindowZoomHost {
  /** Zooms this window's page to `zoom`, and everything of the host's that lines up with it. */
  apply(zoom: WindowZoom): Promise<void>
}

let hostNow: WindowZoomHost | null = null

/** Point the window's zoom at the host that applies it. Composition binds it where there is one. */
export function bindWindowZoom(host: WindowZoomHost | null): void {
  hostNow = host
}

const zoomPreference = storedPreference({
  key: "nessa.desktop.zoom",
  event: "nessa:desktop-zoom",
  parse: parseZoom,
})

export type ZoomCommand = "zoomIn" | "zoomOut" | "zoomReset"

const bind = (chord: Chord, command: ZoomCommand): Binding<ZoomCommand> => ({
  chord,
  command,
})

/**
 * The zoom's keys, the first of each command the one its labels show. ⌘= is
 * ⌘+ without Shift, and both are taken, as are the number pad's. ⌘0 goes
 * back to 100%, as it does in a browser.
 */
export const zoomShortcuts: readonly Binding<ZoomCommand>[] = [
  bind({ code: "Equal", command: true }, "zoomIn"),
  bind({ code: "Equal", command: true, shift: true }, "zoomIn"),
  bind({ code: "NumpadAdd", command: true }, "zoomIn"),
  bind({ code: "Minus", command: true }, "zoomOut"),
  bind({ code: "NumpadSubtract", command: true }, "zoomOut"),
  bind({ code: "Digit0", command: true }, "zoomReset"),
  bind({ code: "Numpad0", command: true }, "zoomReset"),
]

/** Whether this window zooms itself: only under a native host. Settings lists the keys only then. */
export function zoomAvailable(): boolean {
  return hostNow !== null
}

/** What each command is called where the keyboard is listed (Settings › Workspace › Keyboard). */
export const zoomNames: Readonly<Record<ZoomCommand, string>> = {
  zoomIn: "Zoom in",
  zoomOut: "Zoom out",
  zoomReset: "Actual size",
}

const steps: Readonly<Record<ZoomCommand, (zoom: WindowZoom) => WindowZoom>> = {
  zoomIn: zoomedIn,
  zoomOut: zoomedOut,
  zoomReset: () => DEFAULT_ZOOM,
}

/** Nothing bound: a window with no host leaves these keys to the browser. */
const none: readonly Binding<ZoomCommand>[] = []

/**
 * Keeps the host's zoom to the one chosen, from before the first paint, and
 * runs the zoom's keys. Mounted once, by the window, above Settings and the
 * workspace alike, so the keys work wherever the person is.
 */
export function useWindowZoomInEffect(): void {
  const [zoom, choose] = zoomPreference.usePreference()
  // Read once: composition binds the host before the window mounts.
  const host = hostNow
  useKeyBindings(host ? zoomShortcuts : none, (command) => choose(steps[command](zoom)))
  useLayoutEffect(() => {
    if (!host) return
    let latest = true
    host.apply(zoom).then(
      () => {
        // The root says only what the host has done, and only the newest ask.
        if (!latest) return
        const root = document.documentElement
        root.style.setProperty("--desktop-zoom", String(zoom))
        root.dataset.zoom = String(zoom)
      },
      (error: unknown) => console.warn("The window did not zoom", error),
    )
    return () => {
      latest = false
    }
  }, [host, zoom])
}
