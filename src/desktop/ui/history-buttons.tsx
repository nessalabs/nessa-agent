import { isMac } from "../adapters/platform"
import { chordLabel, type Chord } from "../model/keyboard"
import { IconButton, type IconButtonSize } from "./icon-button"

/**
 * Back and Forward, beside the sidebar toggle in every surface's titlebar —
 * the classic shell, both workspace layouts, Settings — in the same place
 * whether the sidebar is drawn, revealed or hidden. The window keeps no
 * history yet, so they are shown and rest: this is the one seam history will
 * be wired to (`canGoBack`, `onBack`, …), and until it is, neither does
 * anything nor looks as if it could. ⌘[ and ⌘] are theirs; ⇧⌘[ and ⇧⌘] step
 * between panes.
 */
export interface History {
  readonly canGoBack?: boolean
  readonly canGoForward?: boolean
  readonly onBack?: () => void
  readonly onForward?: () => void
}

/** What the two say, and the chords reserved for them. */
export const historyActions = {
  back: { label: "Go Back", chord: { code: "BracketLeft", command: true } },
  forward: { label: "Go Forward", chord: { code: "BracketRight", command: true } },
} as const satisfies Record<string, { label: string; chord: Chord }>

export function HistoryButtons({
  size,
  canGoBack = false,
  canGoForward = false,
  onBack,
  onForward,
}: History & { size?: IconButtonSize }) {
  const button = (
    direction: "back" | "forward",
    enabled: boolean,
    go: (() => void) | undefined,
  ) => {
    const { label, chord } = historyActions[direction]
    const usable = enabled && go !== undefined
    return (
      <IconButton
        icon={direction}
        label={label}
        shortcut={chordLabel(chord, isMac)}
        size={size}
        // Resting, not disabled: its tooltip still says what it will do.
        aria-disabled={!usable || undefined}
        data-history={direction}
        onClick={usable ? go : undefined}
      />
    )
  }
  return (
    <>
      {button("back", canGoBack, onBack)}
      {button("forward", canGoForward, onForward)}
    </>
  )
}
