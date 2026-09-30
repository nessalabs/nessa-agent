import {
  createContext,
  useCallback,
  useContext,
  useSyncExternalStore,
  type ReactNode,
} from "react"
import type { ExperimentSource } from "../../application/ports"
import type { Experiment } from "../../model/experiment"

const SourceContext = createContext<ExperimentSource | null>(null)

/** The window's experiment source, from composition (`src/desktop/main.tsx`). */
export function ExperimentsProvider({
  source,
  children,
}: {
  source: ExperimentSource
  children: ReactNode
}) {
  return <SourceContext.Provider value={source}>{children}</SourceContext.Provider>
}

/** Opens a run's change, or one of its files, in the person's editor. */
export function useOpenInEditor(experimentId: string) {
  const source = useContext(SourceContext)
  return useCallback(
    (target: { runId: string; path?: string }) =>
      source?.openInEditor({ experimentId, ...target }),
    [source, experimentId],
  )
}

/** An experiment's title, for the header of a pane it fills. */
export function useExperimentTitle(id: string): string | undefined {
  return useExperiment(id)?.title
}

/** The conversation an experiment belongs to, for a link back to it. */
export function useExperimentSession(id: string): string | undefined {
  return useExperiment(id)?.sessionId
}

/** One experiment as the source holds it now, followed while mounted. */
export function useExperiment(id: string): Experiment | undefined {
  const source = useContext(SourceContext)
  if (!source) throw new Error("useExperiment needs an ExperimentsProvider")
  const subscribe = useCallback(
    (listener: () => void) => source.subscribe(listener),
    [source],
  )
  return useSyncExternalStore(subscribe, () => source.get(id))
}
