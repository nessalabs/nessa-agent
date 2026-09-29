import type { ReactNode } from "react"

/**
 * The window's drag strip and its stationary controls, after the native
 * traffic lights. It sits over the columns and never moves when they fold.
 */
export function WorkspaceTitlebar({ children }: { children: ReactNode }) {
  return (
    <div className="workspace-titlebar" data-tauri-drag-region>
      {children}
    </div>
  )
}
