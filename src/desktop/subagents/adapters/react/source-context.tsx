/**
 * The window's subagent source, provided once from composition. A view reads
 * it here; it does not look one up.
 */
import { createContext, useContext, type ReactNode } from "react"
import type { SubagentSource } from "../../application/ports"

const SubagentsContext = createContext<SubagentSource | null>(null)

export function SubagentsProvider({
  source,
  children,
}: {
  source: SubagentSource
  children: ReactNode
}) {
  return <SubagentsContext.Provider value={source}>{children}</SubagentsContext.Provider>
}

/** The window's source. Throws when composition did not provide one. */
export function useSubagentSource(): SubagentSource {
  const source = useContext(SubagentsContext)
  if (!source)
    throw new Error(
      "useSubagentSource needs a SubagentsProvider: the source comes from composition",
    )
  return source
}
