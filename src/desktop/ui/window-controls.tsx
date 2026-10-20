/**
 * The window's own controls (the macOS traffic lights) sit in the corner of
 * the page's first column. When that column moves away from the window's left
 * edge — the side rail opening beside it — the host is told, so the controls
 * move with it. The host's mover is injected from `main.tsx`; without one
 * (tests, the browser preview) nothing is told.
 */
import { createContext, useContext, useEffect, type ReactNode } from "react"

type Lead = (by: number) => Promise<void>

const WindowControlsLead = createContext<Lead | null>(null)

export function WindowControlsProvider({
  lead,
  children,
}: {
  lead: Lead
  children: ReactNode
}) {
  return (
    <WindowControlsLead.Provider value={lead}>{children}</WindowControlsLead.Provider>
  )
}

/**
 * Keeps the window's controls `by` pixels in from the window's left edge while
 * the calling view is mounted, and back at the edge once it is gone.
 */
export function useWindowControlsLead(by: number) {
  const lead = useContext(WindowControlsLead)
  useEffect(() => {
    if (lead) tell(lead, by)
  }, [lead, by])
  useEffect(() => {
    if (lead) return () => tell(lead, 0)
  }, [lead])
}

function tell(lead: Lead, by: number) {
  lead(by).catch((error: unknown) => {
    console.error("[nessa] could not move the window's controls", error)
  })
}
