import * as React from "react"

import { closeSetupDim } from "../../host/window"

/**
 * The sheet that takes the desktop away while setup opens.
 *
 * It is its own window, covering the screen and ignoring the mouse, so that
 * setup itself can be an ordinary app window — with a frame the system draws,
 * a titlebar to drag it by, and its own real controls. One window cannot be
 * both a screen-covering takeover and a window you can move.
 *
 * It leaves on its own once the opening is over, and closes rather than merely
 * fading: a transparent window that has finished fading is still a window over
 * the screen.
 */
export function SetupDim() {
  React.useEffect(() => {
    const timer = window.setTimeout(() => void closeSetupDim(), DIM_LIFETIME_MS)
    return () => window.clearTimeout(timer)
  }, [])
  return <div aria-hidden="true" className="nessa-setup-dim" />
}

/**
 * How long the dim is on screen, matching the CSS that fades it.
 *
 * It is written here as well as there because a window has to be told to go;
 * the fade reaching zero is not something the window system notices. The
 * fade ends first, so what closes is already invisible.
 */
const DIM_LIFETIME_MS = 3900
