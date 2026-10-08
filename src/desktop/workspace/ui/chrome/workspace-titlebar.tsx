import type { ReactNode } from "react"

/**
 * The window's drag strip and its stationary controls, after the native
 * traffic lights. It sits over the columns and never moves when they fold.
 *
 * The controls slide as one (FlipScope) from where they were drawn to where
 * they are: the strip's own box moves with the workspace while its inset
 * changes with it, so sliding the strip would carry them from a place they
 * never stood — under the traffic lights, on a side rail toggle.
 */
export function WorkspaceTitlebar({ children }: { children: ReactNode }) {
  return (
    <div className="workspace-titlebar" data-tauri-drag-region>
      <div
        className="workspace-titlebar-controls"
        data-tauri-drag-region
        data-flip="slide"
        data-flip-id="titlebar"
      >
        {children}
      </div>
    </div>
  )
}
