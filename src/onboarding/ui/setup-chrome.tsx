import * as React from "react"
import { Volume2, VolumeX, X } from "lucide-react"

import { setSoundMuted, soundMuted, subscribeSoundMuted } from "./sound"

/**
 * Setup's own chrome: the way out, and the way to make it be quiet.
 *
 * It belongs to the window rather than to the step, for two reasons. A control
 * that is always there should not be re-mounted by every step, and — the one
 * that matters — the wash the steps are painted on fades in behind the whole
 * opening, which took the only visible exit with it. Sitting on the window
 * instead, these are on screen as soon as the window is, which is what a
 * full-screen surface covering the menu bar owes the person looking at it.
 *
 * Drawn window controls were never convincing — they have to reimplement every
 * state the system already draws, and the one that could not do anything had to
 * either look broken or lie. Two quiet marks ask for none of that.
 */
export function SetupChrome({ onClose }: { onClose?: () => void }) {
  const muted = React.useSyncExternalStore(subscribeSoundMuted, soundMuted, () => true)
  return (
    <div className="nessa-setup-chrome">
      <button
        type="button"
        aria-label={muted ? "Turn setup sounds on" : "Turn setup sounds off"}
        onClick={() => setSoundMuted(!muted)}
        className="nessa-setup-control"
      >
        {muted ? (
          <VolumeX aria-hidden className="size-4" />
        ) : (
          <Volume2 aria-hidden className="size-4" />
        )}
      </button>
      {onClose ? (
        <button
          type="button"
          aria-label="Close setup"
          onClick={onClose}
          className="nessa-setup-control"
        >
          <X aria-hidden className="size-4" />
        </button>
      ) : null}
    </div>
  )
}
