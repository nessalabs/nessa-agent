import { createContext, useContext, type ReactNode } from "react"

/**
 * One haptic tick, for a gesture that changed something by one step. The
 * haptics are the host's (a call out of the page), so composition supplies
 * it; with none composed nothing is felt.
 */
export type Haptic = () => void

const none: Haptic = () => {}

const HapticContext = createContext<Haptic>(none)

export function HapticProvider({
  tick,
  children,
}: {
  tick: Haptic
  children: ReactNode
}) {
  return <HapticContext.Provider value={tick}>{children}</HapticContext.Provider>
}

/** The composed tick, or one that does nothing. */
export function useHaptic(): Haptic {
  return useContext(HapticContext)
}
