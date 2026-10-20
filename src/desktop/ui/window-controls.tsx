/**
 * The window's own controls (the macOS traffic lights) sit in the corner of
 * the page's first column. When that column moves away from the window's left
 * edge — the side rail opening beside it — the host is told, so the controls
 * move with it. The host's mover is injected from `main.tsx`; without one
 * (tests, the browser preview) nothing is told.
 *
 * Only the surface that has the window's corner moves them. One covered by
 * another (the workspace under Settings, or under the startup screen) asks
 * for the window's edge, where the surface over it starts.
 */
import { createContext, useContext, useEffect, type ReactNode } from "react"

type Lead = (by: number) => Promise<void>

const WindowControlsLead = createContext<Lead | null>(null)
const Covered = createContext(false)

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

/** Marks what it holds as covered: another surface has the window's corner. */
export function WindowControlsCovered({
  covered,
  children,
}: {
  covered: boolean
  children: ReactNode
}) {
  const outer = useContext(Covered)
  return <Covered.Provider value={outer || covered}>{children}</Covered.Provider>
}

/**
 * Keeps the window's controls `by` pixels in from the window's left edge while
 * the calling view is mounted and uncovered, and back at the edge once it is
 * covered or gone.
 */
export function useWindowControlsLead(by: number) {
  const lead = useContext(WindowControlsLead)
  const at = useContext(Covered) ? 0 : by
  useEffect(() => {
    if (lead) tell(lead, at)
  }, [lead, at])
  useEffect(() => {
    if (lead) return () => tell(lead, 0)
  }, [lead])
}

function tell(lead: Lead, by: number) {
  lead(by).catch((error: unknown) => {
    console.error("[nessa] could not move the window's controls", error)
  })
}
