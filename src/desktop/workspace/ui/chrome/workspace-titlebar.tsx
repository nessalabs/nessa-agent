import type { ReactNode } from "react"

/** The window-wide drag strip, above the rail and workspace columns. */
export function WorkspaceTitlebar({ children }: { children: ReactNode }) {
  return (
    <div className="workspace-titlebar" data-tauri-drag-region>
      <div className="workspace-titlebar-controls" data-tauri-drag-region>
        {children}
      </div>
    </div>
  )
}
