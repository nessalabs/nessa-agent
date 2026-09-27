import { useEffect, useState } from "react"

/**
 * Spike only: which workspace layout the window shows, chosen in the
 * Appearance menu and remembered. Every reader follows a change at once.
 */
export type WorkspaceLayout = "columns" | "sidebar" | "classic"

export const workspaceLayouts: readonly { id: WorkspaceLayout; label: string }[] = [
  { id: "columns", label: "Three columns" },
  { id: "sidebar", label: "Sessions in sidebar" },
  { id: "classic", label: "Classic" },
]

const storageKey = "nessa.desktop.workspace-layout"
const changeEvent = "nessa:workspace-layout"

export function parseWorkspaceLayout(value: string | null): WorkspaceLayout {
  return value === "sidebar" || value === "classic" ? value : "columns"
}

function read(): WorkspaceLayout {
  try {
    return parseWorkspaceLayout(localStorage.getItem(storageKey))
  } catch {
    return "columns"
  }
}

export function useWorkspaceLayout(): [WorkspaceLayout, (next: WorkspaceLayout) => void] {
  const [layout, setLayout] = useState(read)
  useEffect(() => {
    const follow = () => setLayout(read())
    window.addEventListener(changeEvent, follow)
    window.addEventListener("storage", follow)
    return () => {
      window.removeEventListener(changeEvent, follow)
      window.removeEventListener("storage", follow)
    }
  }, [])
  const choose = (next: WorkspaceLayout) => {
    try {
      localStorage.setItem(storageKey, next)
    } catch {
      // Remembering is a convenience; this window still switches.
    }
    setLayout(next)
    window.dispatchEvent(new Event(changeEvent))
  }
  return [layout, choose]
}
