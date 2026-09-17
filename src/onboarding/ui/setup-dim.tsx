import * as React from "react"

import { closeSetupDim, openSetupWindow } from "../../host/window"
import { AgentBloom } from "./agent-bloom"

/**
 * The opening: a darkened screen, a light, and the window the light becomes.
 *
 * This is its own window covering the screen, because none of it can happen
 * inside the window it produces — a window exists the instant it is created,
 * frame and corners and controls and all, and a box sitting there while a
 * light is supposed to be turning into one gives the whole thing away.
 *
 * So the light grows here, to exactly the size and place the window will
 * occupy, and the window is opened at the moment it has filled. What is on
 * screen does not change at that instant; what changes is which process is
 * drawing it.
 */
export function SetupDim() {
  React.useEffect(() => {
    const opened = window.setTimeout(() => void openSetupWindow(), LIGHT_FILLED_MS)
    const closed = window.setTimeout(() => void closeSetupDim(), DIM_LIFETIME_MS)
    return () => {
      window.clearTimeout(opened)
      window.clearTimeout(closed)
    }
  }, [])
  return (
    <>
      <div aria-hidden="true" className="nessa-setup-dim" />
      <div aria-hidden="true" className="nessa-setup-reveal">
        <AgentBloom />
      </div>
    </>
  )
}

/**
 * When the light has reached the window's own bounds, and the window is opened
 * behind it.
 *
 * Written here as well as in the stylesheet because a window has to be told to
 * open; a CSS animation reaching its last frame is not something the window
 * system notices. The two have to agree, and the stylesheet says so where they
 * are defined.
 */
const LIGHT_FILLED_MS = 3360 // --nessa-reveal-seed + --nessa-reveal-bloom

/**
 * When the dim goes. It closes rather than merely finishing its fade: a
 * transparent window that has faded out is still a window over the screen.
 */
const DIM_LIFETIME_MS = 3900 // after --nessa-setup-dim has finished fading
