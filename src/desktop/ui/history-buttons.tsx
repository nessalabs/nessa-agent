import { DesktopIcon } from "./icons"
import { tooltip } from "./tooltip"

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
  back: { label: "Go Back", shortcut: "⌘[" },
  forward: { label: "Go Forward", shortcut: "⌘]" },
} as const

export function HistoryButtons({
  className,
  canGoBack = false,
  canGoForward = false,
  onBack,
  onForward,
}: History & { className: string }) {
  const button = (
    direction: "back" | "forward",
    enabled: boolean,
    go: (() => void) | undefined,
  ) => {
    const { label, shortcut } = historyActions[direction]
    const usable = enabled && go !== undefined
    return (
      <button
        type="button"
        className={className}
        aria-label={label}
        // Resting, not disabled: its tooltip still says what it will do.
        aria-disabled={!usable || undefined}
        data-history={direction}
        {...tooltip(label, { shortcut })}
        onClick={usable ? go : undefined}
      >
        <DesktopIcon name={direction} />
      </button>
    )
  }
  return (
    <>
      {button("back", canGoBack, onBack)}
      {button("forward", canGoForward, onForward)}
    </>
  )
}
