import * as React from "react"

import { closeSetupDim, openSetupWindow } from "../../host/window"
import { AgentBloom } from "./agent-bloom"
import { useIntroSound } from "./use-intro-sound"

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
  // The chime belongs to the opening, and the opening is here. It was playing
  // from the setup window, which does not exist until the opening is over —
  // so it was either arriving after the thing it accompanies or not at all.
  useIntroSound(true)

  React.useEffect(() => {
    // Opening setup is what takes this window away: the host closes the dim as
    // soon as the window it produced is up. Nothing here has to close itself,
    // which is the one thing a window covering the screen must not get wrong.
    const opened = window.setTimeout(() => void openSetupWindow(), LIGHT_FILLED_MS)
    // The host has already hidden this window by now, so closing it is only
    // housekeeping — nothing anyone can see depends on it happening.
    const closed = window.setTimeout(() => void closeSetupDim(), CHIME_ENDED_MS)
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
 * When the opening's chime has finished and this window has nothing left to
 * do. It is longer than the opening, which is why the window is hidden at the
 * handover rather than closed: closing it would cut the sound off.
 */
const CHIME_ENDED_MS = 4800
