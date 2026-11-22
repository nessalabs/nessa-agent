/**
 * The window's terminals, for the views that show one: a pane, or a Run
 * block's output. Absent beside no gateway, and then nothing offers one.
 */
import { createContext, useContext, type ReactNode } from "react"
import type { TerminalSource } from "../../application/ports"

const TerminalSourceContext = createContext<TerminalSource | undefined>(undefined)

export function TerminalSourceProvider({
  source,
  children,
}: {
  source: TerminalSource | undefined
  children: ReactNode
}) {
  return (
    <TerminalSourceContext.Provider value={source}>
      {children}
    </TerminalSourceContext.Provider>
  )
}

export function useTerminalSource(): TerminalSource | undefined {
  return useContext(TerminalSourceContext)
}
